// Commands that change variables: add, set, import, rm. None of them ever print a value.
use super::{Ctx, guard_change, guard_protected, one, refresh_file, require_app, target_for};
use crate::args::{flag, parse, string};
use crate::envfile::{ApplyResult, absorb_outside, parse_env_file};
use crate::errors::{PoenvError, Res, code};
use crate::output::{emit_json, ok, print_apply, print_others, warn};
use crate::project::{Project, current_env, find_project, has_block, require_project, write_env_file};
use crate::prompt::{ask_hidden, pick, read_stdin};
use crate::repos::refresh_other_repos;
use crate::store::model::{
    RemoveMode, Store, Target, get_env, remove_shared, remove_var, resolve_env, set_ref, set_var, shared_uses, valid_key,
};
use crate::store::open_store;
use crate::util::{exists, file_mode, join_display, plural, write_file_atomic};
use serde_json::{Map, Value, json};
use std::fs;
use std::io::IsTerminal;

pub const ADD_HELP: &str = "poenv add KEY [--as NAME] [--from APP/ENV] [--env ENV]

Add one key to this repo's current env (or --env). By default it's a live
reference to the shared key KEY, so rotating the shared value updates every app.
--from copies the value from another app's env once instead. --as renames it.";

pub const SET_HELP: &str = "poenv set KEY [--shared] [--env ENV] [--empty]

Set a value. You'll be asked for it (hidden), or pipe it in:
  printf '%s' \"$VALUE\" | poenv set KEY
  poenv set TLS_KEY < key.pem
Values are never accepted as arguments, so they stay out of shell history.
--empty sets an empty value.";

pub const IMPORT_HELP: &str = "poenv import [--env ENV]

Save the keys you pasted into this repo's env file (outside the poenv block)
to the current env (or --env). Keys that already exist are overwritten; their
old values stay in poenv history. The pasted lines are then removed from the file.";

pub const RM_HELP: &str = "poenv rm KEY [--shared [--keep-copies | --everywhere]] [--env ENV]

Remove a key from this repo's current env (or --env), or from the shared pool.
If apps still use a shared key, say what happens to them (a person is asked):
  --keep-copies   each app keeps its own copy of the value (this undoes sharing)
  --everywhere    remove it from those apps too";

pub fn add(ctx: &Ctx, args: &[String]) -> Res<i32> {
    let p = parse("add", args, &[string("as"), string("from"), string("env")])?;
    let key = one(&p.positionals, "add", "KEY")?;
    let project = require_project(&ctx.cwd)?;
    let mut store = open_store()?;
    require_app(&store.data, &project)?;
    let env = match p.str("env") {
        Some(e) => e,
        None => current_env(&project)?,
    };
    let target = Target::app(&project.app, &env);
    let name = p.str("as").unwrap_or_else(|| key.clone());
    let command = format!("poenv add {}", args.join(" "));
    let from = p.str("from");
    let source = match &from {
        Some(f) => {
            let mut parts = f.split('/');
            match (parts.next().filter(|s| !s.is_empty()), parts.next().filter(|s| !s.is_empty())) {
                (Some(a), Some(e)) => Some((a.to_string(), e.to_string())),
                _ => return Err(PoenvError::new(format!("--from must look like APP/ENV (got \"{f}\")."))),
            }
        }
        None => None,
    };

    guard_change(ctx, &store.data, &target, &name, &command)?;
    // Reading a protected env's values into an unprotected one would be a way around its protection.
    if let Some((a, e)) = &source {
        guard_protected(ctx, &store.data, a, e, "copy from it", &command)?;
    } else if !get_env(&store.data, &project.app, &env)?.protected {
        let uses = shared_uses(&store.data, &key);
        let all_protected = uses.iter().all(|u| get_env(&store.data, &u.app, &u.env).is_ok_and(|e| e.protected));
        if !uses.is_empty() && all_protected {
            let doing = format!("link its shared {key} into other envs");
            guard_protected(ctx, &store.data, &uses[0].app, &uses[0].env, &doing, &command)?;
        }
    }

    store.update(|s| match &source {
        None => set_ref(s, &project.app, &env, &name, &key),
        Some((a, e)) => {
            let Some(value) = resolve_env(s, a, e)?.get(&key).cloned() else {
                return Err(PoenvError::fix(format!("{a}/{e} has no key \"{key}\"."), code::UNKNOWN, format!("poenv ls {a}")));
            };
            set_var(s, &target, &name, &value)
        }
    })?;

    let how = match &from {
        Some(f) => format!("copied from {f}"),
        None => format!("shared reference to {key}"),
    };
    let mut extra = Map::new();
    extra.insert("key".into(), name.clone().into());
    extra.insert("app".into(), project.app.clone().into());
    extra.insert("env".into(), env.clone().into());
    extra.insert("ref".into(), if from.is_some() { Value::Null } else { key.clone().into() });
    finish(ctx, Some(&project), &store.data, &format!("Added {name} to {}/{env} ({how})", project.app), extra)
}

pub fn set(ctx: &Ctx, args: &[String]) -> Res<i32> {
    let p = parse("set", args, &[flag("shared"), string("env"), flag("empty")])?;
    if p.positionals.len() > 1 {
        let k = &p.positionals[0];
        return Err(PoenvError::fix(
            "Values are never passed as arguments (they'd end up in shell history and agent logs).",
            code::ERROR,
            format!("poenv set {k}   (then type the value), or pipe it: printf '%s' \"$VALUE\" | poenv set {k}"),
        ));
    }
    let key = one(&p.positionals, "set", "KEY")?;
    let shared = p.flag("shared");
    let project = if shared { find_project(&ctx.cwd)? } else { Some(require_project(&ctx.cwd)?) };
    let target = target_for(shared, p.str("env"), project.as_ref())?;

    let mut store = open_store()?;
    if let (Some(pr), false) = (&project, shared) {
        require_app(&store.data, pr)?;
    }
    guard_change(ctx, &store.data, &target, &key, &format!("poenv set {}", args.join(" ")))?;

    let empty = p.flag("empty");
    let value = if empty {
        String::new()
    } else if std::io::stdin().is_terminal() {
        ask_hidden(&format!("Value for {key} (hidden): "))?
    } else {
        read_stdin()?
    };
    if value.is_empty() && !empty {
        return Err(PoenvError::fix(
            format!("No value given for {key}."),
            code::ERROR,
            format!("printf '%s' \"$VALUE\" | poenv set {key}   (or poenv set {key} --empty)"),
        ));
    }

    store.update(|s| set_var(s, &target, &key, &value))?;
    let mut extra = Map::new();
    extra.insert("key".into(), key.clone().into());
    extra.insert("target".into(), target.to_json());
    finish(ctx, project.as_ref(), &store.data, &format!("Set {key} in {}", target.describe()), extra)
}

/// Keys in an env file that poenv can't store (it accepts `.` and `-` in env files; the vault doesn't).
pub fn split_invalid(keys: Vec<String>) -> (Vec<String>, Vec<String>) {
    keys.into_iter().partition(|k| valid_key(k))
}

pub fn warn_invalid(invalid: &[String], file: &str) {
    for k in invalid {
        warn(&format!(
            "{k} can't be stored in poenv (key names use letters, digits and \"_\"), so it stays as-is in {file}."
        ));
    }
}

pub fn import_keys(ctx: &Ctx, args: &[String]) -> Res<i32> {
    let p = parse("import", args, &[string("env")])?;
    if !p.positionals.is_empty() {
        return Err(PoenvError::fix(
            "poenv import takes no file: it reads this repo's env file.",
            code::ERROR,
            "poenv import --help",
        ));
    }
    let project = require_project(&ctx.cwd)?;
    let path = project.env_file_path();
    let text = if exists(&path) { fs::read_to_string(&path)? } else { String::new() };
    let found = parse_env_file(&text, project.format)?.outside;
    let mut all: Vec<String> = found.keys().cloned().collect();
    all.sort();
    let (keys, invalid) = split_invalid(all);
    if keys.is_empty() {
        if !invalid.is_empty() && !ctx.json {
            warn_invalid(&invalid, &project.file);
        }
        return Err(PoenvError::fix(
            format!("{} has no keys outside the poenv block.", project.file),
            code::ERROR,
            format!("paste KEY=VALUE lines into {}, then run poenv import", project.file),
        ));
    }

    let mut store = open_store()?;
    require_app(&store.data, &project)?;
    let env = match p.str("env") {
        Some(e) => e,
        None => current_env(&project)?,
    };
    let target = Target::app(&project.app, &env);
    guard_protected(ctx, &store.data, &project.app, &env, "import into it", &format!("poenv import --env {env}"))?;
    let before = &get_env(&store.data, &project.app, &env)?.vars;
    let overwritten: Vec<String> = keys.iter().filter(|k| before.contains_key(*k)).cloned().collect();
    store.update(|s| {
        for k in &keys {
            set_var(s, &target, k, &found[k])?;
        }
        Ok(())
    })?;

    // The pasted lines now live in the vault; the block carries them from here on.
    write_file_atomic(&path, absorb_outside(&text, &keys, project.format)?.as_bytes(), file_mode(&path))?;
    if !has_block(&project)? {
        let cur = current_env(&project)?;
        write_env_file(&project, &cur, &resolve_env(&store.data, &project.app, &cur)?)?;
    }
    if !ctx.json {
        warn_invalid(&invalid, &project.file);
    }
    let replaced = if overwritten.is_empty() { String::new() } else { format!(", {} overwritten", overwritten.len()) };
    let mut extra = Map::new();
    extra.insert("app".into(), project.app.clone().into());
    extra.insert("env".into(), env.clone().into());
    extra.insert("imported".into(), keys.clone().into());
    extra.insert("overwritten".into(), overwritten.clone().into());
    extra.insert("invalid".into(), invalid.into());
    let message = format!("Imported {} into {}/{env}{replaced}", plural(keys.len(), "key"), project.app);
    finish(ctx, Some(&project), &store.data, &message, extra)
}

pub fn rm(ctx: &Ctx, args: &[String]) -> Res<i32> {
    let p = parse("rm", args, &[flag("shared"), string("env"), flag("keep-copies"), flag("everywhere")])?;
    let key = one(&p.positionals, "rm", "KEY")?;
    let shared = p.flag("shared");
    let project = if shared { find_project(&ctx.cwd)? } else { Some(require_project(&ctx.cwd)?) };
    let target = target_for(shared, p.str("env"), project.as_ref())?;
    let (keep, everywhere) = (p.flag("keep-copies"), p.flag("everywhere"));
    if !shared && (keep || everywhere) {
        return Err(PoenvError::fix(
            "--keep-copies and --everywhere only apply with --shared.",
            code::ERROR,
            "poenv rm --help",
        ));
    }
    if keep && everywhere {
        return Err(PoenvError::fix("Choose one of --keep-copies or --everywhere.", code::ERROR, "poenv rm --help"));
    }
    let mut store = open_store()?;
    guard_change(ctx, &store.data, &target, &key, &format!("poenv rm {}", args.join(" ")))?;
    let uses = if shared { shared_uses(&store.data, &key) } else { vec![] };
    if uses.is_empty() {
        store.update(|s| remove_var(s, &target, &key))?;
        let mut extra = Map::new();
        extra.insert("key".into(), key.clone().into());
        extra.insert("target".into(), target.to_json());
        return finish(ctx, project.as_ref(), &store.data, &format!("Removed {key} from {}", target.describe()), extra);
    }

    // Apps still use it: deleting it alone would leave them pointing at nothing.
    let places: Vec<String> = uses.iter().map(|u| format!("{}/{}", u.app, u.env)).collect();
    let places = places.join(", ");
    let mode = if keep {
        RemoveMode::KeepCopies
    } else if everywhere {
        RemoveMode::Everywhere
    } else {
        if !ctx.human || ctx.json {
            return Err(PoenvError::fix(
                format!("Shared {key} is used in {places}."),
                code::ERROR,
                format!("poenv rm {key} --shared --keep-copies   (or --everywhere to remove it from those apps too)"),
            ));
        }
        let keep_label = "Keep a copy in each (they stop following the shared value)".to_string();
        let choice = pick(
            &format!("Shared {key} is used in {places}. Delete it and:"),
            &[keep_label.clone(), "Delete it everywhere".into()],
            None,
        )?;
        if choice == keep_label { RemoveMode::KeepCopies } else { RemoveMode::Everywhere }
    };
    store.update(|s| remove_shared(s, &key, mode))?;
    let message = match mode {
        RemoveMode::KeepCopies => format!("{key} is no longer shared; {places} keep their own copy"),
        RemoveMode::Everywhere => format!("Removed {key} from the shared pool and {places}"),
    };
    let mut extra = Map::new();
    extra.insert("key".into(), key.clone().into());
    extra.insert("target".into(), target.to_json());
    let field = if mode == RemoveMode::KeepCopies { "keptCopies" } else { "removedFrom" };
    extra.insert(field.into(), uses.iter().map(|u| u.to_json()).collect::<Vec<_>>().into());
    finish(ctx, project.as_ref(), &store.data, &message, extra)
}

/// Report, and rewrite the env files the change affects: this repo's, and any other repo's on this machine.
pub fn finish(ctx: &Ctx, project: Option<&Project>, store: &Store, message: &str, extra: Map<String, Value>) -> Res<i32> {
    let applied = match project {
        Some(p) => safe_refresh(p, store)?,
        None => None,
    };
    let (updated, skipped) = refresh_other_repos(store, project.map(|p| p.root.as_str()));
    if ctx.json {
        let mut out = json!({ "ok": true });
        let m = out.as_object_mut().unwrap();
        m.extend(extra);
        m.insert(
            "file".into(),
            match (&applied, project) {
                (Some((_, r)), Some(p)) => {
                    let mut f = Map::new();
                    f.insert("path".into(), p.file.clone().into());
                    f.extend(r.json_fields());
                    Value::Object(f)
                }
                _ => Value::Null,
            },
        );
        m.insert(
            "otherFiles".into(),
            updated
                .iter()
                .map(|o| json!({ "path": join_display(&o.root, &o.file), "app": o.app, "env": o.env, "diff": o.result.diff.to_json() }))
                .collect::<Vec<_>>()
                .into(),
        );
        m.insert(
            "skippedFiles".into(),
            skipped
                .iter()
                .map(|s| {
                    let path = match &s.file {
                        Some(f) => join_display(&s.root, f),
                        None => s.root.clone(),
                    };
                    json!({ "path": path, "reason": s.reason })
                })
                .collect::<Vec<_>>()
                .into(),
        );
        emit_json(&out);
        return Ok(code::OK);
    }
    ok(message);
    if let (Some((env, r)), Some(p)) = (&applied, project) {
        if r.diff.count() > 0 {
            print_apply(&p.app, env, &p.file, p.format.name(), r);
        }
    }
    print_others(&updated, &skipped);
    Ok(code::OK)
}

/// A dangling reference elsewhere in the env shouldn't fail an unrelated change.
fn safe_refresh(project: &Project, store: &Store) -> Res<Option<(String, ApplyResult)>> {
    match refresh_file(project, store) {
        Ok(r) => Ok(r),
        Err(e) if !e.unexpected => Ok(None),
        Err(e) => Err(e),
    }
}
