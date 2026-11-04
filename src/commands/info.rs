// Read-only commands: ls and status. They show key names, never values.
use super::Ctx;
use crate::args::{flag, parse};
use crate::errors::{PoenvError, Res, code};
use crate::output::{bold, dim, emit_json, say, yellow};
use crate::project::{current_env, find_project, has_block, require_project};
use crate::store::model::Var;
use crate::store::open_store;
use crate::util::locale_cmp;
use serde_json::{Value, json};

pub const LS_HELP: &str = "poenv ls [APP]

List apps, environments and key names (never values). Inside a repo, shows
that repo's app. Shared keys are listed too.";

pub const STATUS_HELP: &str = "poenv status [--short]

Show this repo's app and the env its env file is on. --short prints
\"app:env\" for shell prompts. Doesn't open the vault, so it's instant
and works while poenv is locked.";

pub fn ls(ctx: &Ctx, args: &[String]) -> Res<i32> {
    let p = parse("ls", args, &[])?;
    let project = find_project(&ctx.cwd)?;
    let store = open_store()?.data;
    let name = p.positionals.first().cloned().or_else(|| project.as_ref().map(|p| p.app.clone()));
    let mut shared: Vec<String> = store.shared.keys().cloned().collect();
    shared.sort();

    let Some(name) = name else {
        let mut apps: Vec<(&String, Vec<String>)> =
            store.apps.iter().map(|(a, v)| (a, v.envs.keys().cloned().collect())).collect();
        apps.sort_by(|a, b| locale_cmp(a.0, b.0));
        if ctx.json {
            let list: Vec<Value> = apps.iter().map(|(a, e)| json!({ "app": a, "envs": e })).collect();
            emit_json(&json!({ "ok": true, "apps": list, "shared": shared }));
            return Ok(code::OK);
        }
        say(&bold("Apps"));
        if apps.is_empty() {
            say(&dim("  (none yet; run poenv init in a repo)"));
        }
        for (a, e) in &apps {
            say(&format!("  {a} {}", dim(&e.join(", "))));
        }
        print_shared(&shared);
        return Ok(code::OK);
    };

    let Some(app) = store.apps.get(&name) else {
        let mut known: Vec<&String> = store.apps.keys().collect();
        known.sort();
        let known = if known.is_empty() { "none".to_string() } else { known.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ") };
        return Err(PoenvError::code(format!("No app \"{name}\". Apps: {known}."), code::UNKNOWN));
    };
    let current = match &project {
        Some(p) if p.app == name && has_block(p)? => Some(current_env(p)?),
        _ => None,
    };
    let envs: Vec<Value> = app
        .envs
        .iter()
        .map(|(env, e)| {
            let mut keys: Vec<&String> = e.vars.keys().collect();
            keys.sort();
            let keys: Vec<Value> = keys
                .into_iter()
                .map(|k| match &e.vars[k] {
                    Var::Ref(r) => json!({ "key": k, "ref": r.target }),
                    Var::Lit(_) => json!({ "key": k, "ref": null }),
                })
                .collect();
            json!({ "env": env, "protected": e.protected, "current": Some(env) == current.as_ref(), "keys": keys })
        })
        .collect();
    if ctx.json {
        emit_json(&json!({ "ok": true, "app": name, "envs": envs, "shared": shared }));
        return Ok(code::OK);
    }
    say(&bold(&name));
    for e in &envs {
        let is_current = e["current"].as_bool().unwrap_or(false);
        let is_protected = e["protected"].as_bool().unwrap_or(false);
        let tags: Vec<&str> = [(is_current, "current"), (is_protected, "protected")]
            .into_iter()
            .filter(|(on, _)| *on)
            .map(|(_, t)| t)
            .collect();
        let tag = if tags.is_empty() { String::new() } else { dim(&format!(" ({})", tags.join(", "))) };
        say(&format!("  {} {}{tag}", if is_current { "▸" } else { " " }, bold(e["env"].as_str().unwrap_or(""))));
        let keys = e["keys"].as_array().cloned().unwrap_or_default();
        if keys.is_empty() {
            say(&dim("      (no keys)"));
        }
        for k in keys {
            let r = k["ref"].as_str().map(|r| dim(&format!(" → shared {r}"))).unwrap_or_default();
            say(&format!("      {}{r}", k["key"].as_str().unwrap_or("")));
        }
    }
    print_shared(&shared);
    Ok(code::OK)
}

pub fn status(ctx: &Ctx, args: &[String]) -> Res<i32> {
    let p = parse("status", args, &[flag("short")])?;
    let short = p.flag("short");
    let project = if short { find_project(&ctx.cwd)? } else { Some(require_project(&ctx.cwd)?) };
    let Some(project) = project else { return Ok(code::NOT_SET_UP) }; // --short: print nothing outside poenv repos
    let env = current_env(&project)?;
    let written = has_block(&project)?;
    if ctx.json {
        emit_json(&json!({
            "ok": true,
            "app": project.app,
            "env": env,
            "written": written,
            "file": project.file,
            "format": project.format.name(),
            "root": project.root,
        }));
    } else if short {
        say(&format!("{}:{env}", project.app));
    } else {
        say(&format!("{} {}", dim("app   "), bold(&project.app)));
        let note = if written { String::new() } else { yellow(&format!("  (not written yet: poenv use {env})")) };
        say(&format!("{} {}{note}", dim("env   "), bold(&env)));
        say(&format!("{} {} {}", dim("file  "), project.file, dim(&format!("(format: {})", project.format.name()))));
    }
    Ok(code::OK)
}

fn print_shared(shared: &[String]) {
    say(&bold("Shared"));
    if shared.is_empty() {
        say(&dim("  (none)"));
    }
    for k in shared {
        say(&format!("  {k}"));
    }
}
