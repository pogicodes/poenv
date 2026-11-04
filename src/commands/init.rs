// poenv init: set up this repo (and, on first run, this machine's vault). SPEC §6.
use super::vars::{split_invalid, warn_invalid};
use super::{Ctx, guard_protected, merge};
use crate::args::{Opt, flag, parse, string};
use crate::envfile::{Dialect, absorb_outside, parse_env_file};
use crate::errors::{PoenvError, Res, code};
use crate::output::{bold, dim, emit_json, ok, say, show_phrase, warn};
use crate::project::{MARKER, Project, ValuesMode, read_marker, write_env_file, write_marker};
use crate::prompt::{ask, confirm, pick};
use crate::repo::{
    Ignored, default_app_name, detect_project, ensure_agent_notes, ensure_ignored, env_files_with_keys, format_for, is_tracked,
};
use crate::repos::remember_repo;
use crate::store::model::{DEFAULT_ENVS, Store, Target, Var, create_app, create_env, resolve_env, set_ref, set_var};
use crate::store::{StoreHandle, init_store, open_store};
use crate::util::{exists, file_mode, write_file_atomic};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

pub const HELP: &str = "poenv init [--app NAME] [--env ENV] [--file FILE] [--format dotenv|next|python|properties] [--no-import] [--yes]

Set up this repo: create the app in your vault, import the existing env file,
write .poenv, gitignore both and add poenv notes to AGENTS.md.
--env picks the env to import into and start on; it's created if it doesn't exist.
On first run it also creates your vault and shows its recovery phrase.
In a fresh clone that already has .poenv, it creates the app from .poenv.";

struct Duplicate {
    key: String,
    places: Vec<(String, String)>,
    shared_exists: bool,
}

pub fn run(ctx: &Ctx, args: &[String]) -> Res<i32> {
    let opts: [Opt; 6] = [
        string("app"),
        string("env"),
        string("file"),
        string("format"),
        flag("no-import"),
        Opt { name: "yes", kind: crate::args::Kind::Bool, short: Some('y') },
    ];
    let p = parse("init", args, &opts)?;
    let interactive = ctx.human && !ctx.json && !p.flag("yes");
    let root = ctx.cwd.to_string_lossy().into_owned();
    // A clone with .poenv on a machine whose vault doesn't have the app yet: adopt its settings.
    let marker = if exists(&Path::new(&root).join(MARKER)) { Some(read_marker(&root)?) } else { None };

    let (mut handle, created) = open_or_create_vault(ctx)?;
    let (mut v_app, mut v_env, mut v_file, mut v_format) = (p.str("app"), p.str("env"), p.str("file"), p.str("format"));
    if let Some(m) = &marker {
        if handle.data.apps.contains_key(&m.app) {
            return Err(PoenvError::fix("This repo is already set up (.poenv exists).", code::ERROR, "poenv use"));
        }
        v_app.get_or_insert(m.app.clone());
        v_file.get_or_insert(m.file.clone());
        v_format.get_or_insert(m.format.name().to_string());
        v_env.get_or_insert(m.env.clone());
    }
    let detected = detect_project(&root);
    let format_name = v_format.clone().unwrap_or_else(|| detected.format.name().to_string());
    let Some(format) = Dialect::parse(&format_name) else {
        return Err(PoenvError::new(format!("--format must be dotenv, next, python or properties (got \"{format_name}\").")));
    };
    let file = match v_file.clone() {
        Some(f) => f,
        None => choose_file(&root, format, detected.file, interactive)?,
    };
    // A detected format follows the file: Spring's keys may already live in a .env (Docker Compose).
    let format = if v_format.is_some() { format } else { format_for(&file, format) };
    let mut app = v_app.clone().unwrap_or_else(|| default_app_name(&root));
    if interactive && v_app.is_none() {
        app = ask("App name", &app)?;
    }
    let mut project = Project {
        root: root.clone(),
        app: app.clone(),
        env: v_env.clone().unwrap_or_else(|| "local".into()),
        file: file.clone(),
        format,
        values: marker.as_ref().map(|m| m.values).unwrap_or(ValuesMode::File),
    };
    if is_tracked(&root, &file) {
        return Err(PoenvError::fix(
            format!("git tracks {file}, so values written there would be committed."),
            code::ERROR,
            format!("untrack it first (git rm --cached {file}), or pick another file with --file"),
        ));
    }

    // Keys already in the env file (outside any poenv block) are candidates for import.
    let file_path = Path::new(&root).join(&file);
    let file_text = if exists(&file_path) { fs::read_to_string(&file_path)? } else { String::new() };
    let found = parse_env_file(&file_text, format)?.outside;
    let (found_keys, invalid) = split_invalid(found.keys().cloned().collect());
    let app_existed = handle.data.apps.contains_key(&app);
    let mut envs: Vec<String> = if app_existed {
        handle.data.apps[&app].envs.keys().cloned().collect()
    } else {
        DEFAULT_ENVS.iter().map(|s| s.to_string()).collect()
    };
    // --env (or .poenv's env in a fresh clone) may name an env the app doesn't have yet: it's created.
    if let Some(e) = &v_env {
        if !envs.contains(e) {
            envs.push(e.clone());
        }
    }

    let mut import_into: Option<String> = None;
    if !found_keys.is_empty() && !p.flag("no-import") {
        import_into = Some(v_env.clone().unwrap_or_else(|| "local".into()));
        if interactive {
            let yes = confirm(&format!("Import {} keys from {file} into poenv?", found_keys.len()), true)?;
            import_into = if yes { Some(pick("Into which environment?", &envs, import_into.as_deref())?) } else { None };
        }
        if let Some(i) = &import_into {
            if !envs.contains(i) {
                return Err(PoenvError::code(
                    format!("No environment \"{i}\". Environments: {}.", envs.join(", ")),
                    code::UNKNOWN,
                ));
            }
        }
    }

    if let Some(i) = &import_into {
        if app_existed && handle.data.apps[&app].envs.get(i).is_some_and(|e| e.protected) {
            let command = format!("poenv init --app {app} --env {i}");
            guard_protected(ctx, &handle.data, &app, i, "import into it", &command)?;
        }
    }

    let start_env = project.env.clone();
    let duplicates = handle.update(|s| {
        if !app_existed {
            create_app(s, &app)?;
        }
        let env = import_into.clone().unwrap_or_else(|| start_env.clone());
        if !s.apps[&app].envs.contains_key(&env) {
            create_env(s, &app, &env)?;
        }
        let Some(into) = &import_into else { return Ok(vec![]) };
        let target = Target::app(&app, into);
        for k in &found_keys {
            set_var(s, &target, k, &found[k])?;
        }
        Ok(find_duplicates(s, &app, into, &found_keys))
    })?;

    let mut shared_now: Vec<String> = vec![];
    if !duplicates.is_empty() && interactive {
        for d in &duplicates {
            let where_: Vec<String> = d.places.iter().map(|(a, e)| format!("{a}/{e}")).collect();
            let what =
                if d.shared_exists { "matches the shared pool".to_string() } else { format!("has the same value in {}", where_.join(", ")) };
            if confirm(&format!("{} {what}. Use one shared value so they stay in sync?", d.key), true)? {
                let into = import_into.clone().unwrap();
                handle.update(|s| share_key(s, &app, &into, d))?;
                shared_now.push(d.key.clone());
            }
        }
    }

    // Move imported keys into the managed block, then write the block for the chosen env.
    if import_into.is_some() && !found_keys.is_empty() {
        write_file_atomic(&file_path, absorb_outside(&file_text, &found_keys, format)?.as_bytes(), file_mode(&file_path))?;
    }
    if let Some(i) = &import_into {
        project.env = i.clone();
    }
    write_marker(&project)?;
    remember_repo(&root);
    let applied = write_env_file(&project, &project.env, &resolve_env(&handle.data, &app, &project.env)?)?;
    let ignored = ensure_ignored(&root, &file)?;
    // .poenv is per-person, so it's gitignored too, unless the repo already commits it.
    let marker_ignored = if is_tracked(&root, MARKER) { None } else { Some(ensure_ignored(&root, MARKER)?) };
    let notes = ensure_agent_notes(&root)?;
    let left: Vec<&Duplicate> = duplicates.iter().filter(|d| !shared_now.contains(&d.key)).collect();

    if ctx.json {
        let mut imported = if import_into.is_some() { found_keys.clone() } else { vec![] };
        imported.sort();
        let dups: Vec<Value> = left
            .iter()
            .map(|d| {
                let places: Vec<Value> = d.places.iter().map(|(a, e)| json!({ "app": a, "env": e })).collect();
                json!({ "key": d.key, "places": places, "sharedExists": d.shared_exists })
            })
            .collect();
        let notes_json: Vec<Value> = notes.iter().map(|(f, a)| json!({ "file": f, "added": a })).collect();
        emit_json(&merge(
            json!({
                "ok": true,
                "vaultCreated": created,
                "app": app,
                "appExisted": app_existed,
                "env": project.env,
                "file": file,
                "format": format.name(),
                "imported": imported,
                "invalid": invalid,
                "shared": shared_now,
                "duplicates": dups,
                "gitignore": ignored.name(),
                "markerGitignore": marker_ignored.map(|m| m.name()).unwrap_or("tracked"),
                "agentNotes": notes_json,
            }),
            applied.json_fields(),
        ));
        return Ok(code::OK);
    }

    let verb = if app_existed { "Linked to existing app" } else { "Created app" };
    ok(&format!("{verb} {} {}", bold(&app), dim(&format!("(file: {file}, format: {})", format.name()))));
    if let Some(i) = &import_into {
        ok(&format!("Imported {} keys into {app}/{i}", found_keys.len()));
    }
    warn_invalid(&invalid, &file);
    for k in &shared_now {
        ok(&format!("{k} is now a shared key"));
    }
    for d in &left {
        warn(&format!("{} has the same value elsewhere. To share it: poenv set {} --shared, then poenv add {}", d.key, d.key, d.key));
    }
    ok(&format!("Wrote {MARKER}"));
    let mut added = vec![];
    if ignored == Ignored::Added {
        added.push(file.clone());
    }
    if marker_ignored == Some(Ignored::Added) {
        added.push(MARKER.to_string());
    }
    if !added.is_empty() {
        ok(&format!("Added {} to .gitignore", added.join(" and ")));
    }
    if ignored == Ignored::NotGit {
        warn(&format!("Not a git repo, so {file} isn't gitignored. Keep it out of version control."));
    }
    for (f, a) in &notes {
        if *a {
            ok(&format!("Added poenv notes to {f}"));
        }
    }
    say(&dim(&format!("  {file} is on {}. Switch with: poenv use <env>", project.env)));
    Ok(code::OK)
}

/// The env file to manage: the framework's usual one, unless the keys live elsewhere.
/// Several files with keys: people pick one; scripts get the first.
fn choose_file(root: &str, format: Dialect, usual: &str, interactive: bool) -> Res<String> {
    let with_keys = env_files_with_keys(root, format, usual);
    if with_keys.len() <= 1 || !interactive {
        return Ok(with_keys.first().cloned().unwrap_or_else(|| usual.to_string()));
    }
    pick("Which env file should poenv manage?", &with_keys, None)
}

/// Opens the vault, creating it on first run. Creating shows the recovery phrase, so it needs a person.
fn open_or_create_vault(ctx: &Ctx) -> Res<(StoreHandle, bool)> {
    match open_store() {
        Ok(h) => return Ok((h, false)),
        Err(e) if e.fix.as_deref() == Some("poenv init") && !e.unexpected => {}
        Err(e) => return Err(e),
    }
    if !ctx.human || ctx.json {
        return Err(PoenvError::fix(
            "poenv's vault hasn't been created on this machine yet. Creating it shows a recovery phrase, so a person has to do it.",
            code::WAITING_ON_USER,
            "ask the user to run \"poenv init\" in a terminal",
        ));
    }
    say(&bold("Welcome to poenv."));
    let phrase = init_store()?;
    show_phrase(&phrase);
    let answer = ask("Type \"saved\" when it's stored (Enter to skip; poenv doctor will remind you)", "")?;
    let mut handle = open_store()?;
    if answer.to_lowercase() == "saved" {
        handle.update(|s| {
            s.meta.recovery_confirmed = true;
            Ok(())
        })?;
    }
    say("");
    ok("Created your poenv vault");
    Ok((handle, true))
}

/// Imported keys whose exact value already lives elsewhere (other apps or the shared pool).
fn find_duplicates(s: &Store, app: &str, env: &str, keys: &[String]) -> Vec<Duplicate> {
    let mut out = vec![];
    for key in keys {
        let Some(Var::Lit(mine)) = s.apps[app].envs[env].vars.get(key) else { continue };
        let mut places = vec![];
        for (a, ap) in &s.apps {
            for (e, en) in &ap.envs {
                if a == app && e == env {
                    continue;
                }
                if let Some(Var::Lit(v)) = en.vars.get(key) {
                    if v.value == mine.value {
                        places.push((a.clone(), e.clone()));
                    }
                }
            }
        }
        let shared = s.shared.get(key);
        let shared_exists = shared.is_some_and(|v| v.value == mine.value);
        if shared.is_some() && !shared_exists {
            continue; // a different shared value already owns the name
        }
        if !places.is_empty() || shared_exists {
            out.push(Duplicate { key: key.clone(), places, shared_exists });
        }
    }
    out
}

/// Puts the value in the shared pool and turns every copy into a reference.
fn share_key(s: &mut Store, app: &str, env: &str, d: &Duplicate) -> Res<()> {
    let Some(Var::Lit(mine)) = s.apps[app].envs[env].vars.get(&d.key).cloned() else { return Ok(()) };
    if !d.shared_exists {
        set_var(s, &Target::Shared, &d.key, &mine.value)?;
    }
    set_ref(s, app, env, &d.key, &d.key)?;
    for (a, e) in &d.places {
        set_ref(s, a, e, &d.key, &d.key)?;
    }
    Ok(())
}
