// poenv env: create and rename this repo's environments. Never prints a value.
use super::{Ctx, guard_protected, require_app};
use crate::args::{parse, string};
use crate::errors::{PoenvError, Res, code};
use crate::output::{emit_json, ok, say};
use crate::project::{current_env, has_block, require_project, write_env_file, write_marker};
use crate::store::model::{copy_env, create_env, rename_env, resolve_env};
use crate::store::open_store;
use serde_json::json;

pub const HELP: &str = "poenv env new NAME [--from ENV]
poenv env rename OLD NEW

Manage this repo's environments (any name: letters, digits, \".\", \"_\" or \"-\").
  new      create an empty env, or a copy of ENV with --from (shared
           references stay references; values are copied once)
  rename   rename an env, keeping its keys, history and protection; the env
           file and .poenv follow if they were on it
New envs aren't protected. Copying or renaming a protected env needs a person.";

pub fn run(ctx: &Ctx, args: &[String]) -> Res<i32> {
    let p = parse("env", args, &[string("from")])?;
    let from = p.str("from");
    let (sub, names) = match p.positionals.split_first() {
        Some((s, n)) => (s.as_str(), n),
        None => ("", &p.positionals[..]),
    };
    if sub == "new" && names.len() == 1 {
        return create(ctx, &names[0], from);
    }
    if sub == "rename" && names.len() == 2 && from.is_none() {
        return rename(ctx, &names[0], &names[1]);
    }
    let problem = if sub == "new" || sub == "rename" {
        format!("Wrong arguments for \"poenv env {sub}\".")
    } else {
        "Expected new or rename.".to_string()
    };
    Err(PoenvError::fix(problem, code::ERROR, "poenv env --help"))
}

fn create(ctx: &Ctx, name: &str, from: Option<String>) -> Res<i32> {
    let project = require_project(&ctx.cwd)?;
    let mut store = open_store()?;
    require_app(&store.data, &project)?;
    let app = project.app.clone();
    // A copy of a protected env would be an unprotected way into its values.
    if let Some(f) = &from {
        guard_protected(ctx, &store.data, &app, f, "copy it", &format!("poenv env new {name} --from {f}"))?;
    }
    store.update(|s| match &from {
        Some(f) => copy_env(s, &app, f, name),
        None => create_env(s, &app, name),
    })?;

    let keys = store.data.apps[&app].envs[name].vars.len();
    if ctx.json {
        emit_json(&json!({ "ok": true, "app": app, "env": name, "from": from, "keys": keys }));
        return Ok(code::OK);
    }
    let detail = match &from {
        Some(f) => format!(" from {f} ({keys} keys)"),
        None => String::new(),
    };
    ok(&format!("Created {app}/{name}{detail}"));
    say(&format!("  Switch to it with: poenv use {name}"));
    Ok(code::OK)
}

fn rename(ctx: &Ctx, from: &str, to: &str) -> Res<i32> {
    let project = require_project(&ctx.cwd)?;
    let mut store = open_store()?;
    require_app(&store.data, &project)?;
    let app = project.app.clone();
    guard_protected(ctx, &store.data, &app, from, "rename it", &format!("poenv env rename {from} {to}"))?;
    let file_was_on = has_block(&project)? && current_env(&project)? == from;
    store.update(|s| rename_env(s, &app, from, to))?;

    // Point the env file's block header and the .poenv default at the new name.
    if file_was_on {
        write_env_file(&project, to, &resolve_env(&store.data, &app, to)?)?;
    }
    let marker_updated = project.env == from;
    if marker_updated {
        let mut p = project.clone();
        p.env = to.to_string();
        write_marker(&p)?;
    }

    if ctx.json {
        emit_json(&json!({
            "ok": true, "app": app, "from": from, "to": to, "fileUpdated": file_was_on, "markerUpdated": marker_updated,
        }));
        return Ok(code::OK);
    }
    ok(&format!("Renamed {app}/{from} to {to}"));
    if file_was_on {
        ok(&format!("{} is now on {to}", project.file));
    }
    if marker_updated {
        ok(&format!(".poenv's default env is now {to}"));
    }
    Ok(code::OK)
}
