use super::{Ctx, guard_switch, merge, require_app};
use crate::args::parse;
use crate::errors::{Res, code};
use crate::output::{emit_json, print_apply};
use crate::project::{current_env, require_project, write_env_file};
use crate::prompt::pick;
use crate::store::model::{get_env, resolve_env};
use crate::store::open_store;
use serde_json::json;

pub const HELP: &str = "poenv use [ENV]

Switch this repo's env file to ENV. With no ENV: a picker (in a terminal),
otherwise the default env from .poenv. Protected envs need you to type their name.";

pub fn run(ctx: &Ctx, args: &[String]) -> Res<i32> {
    let p = parse("use", args, &[])?;
    let project = require_project(&ctx.cwd)?;
    let store = open_store()?;
    let app = require_app(&store.data, &project)?;
    let current = current_env(&project)?;

    let env = match p.positionals.first() {
        Some(e) => e.clone(),
        None if ctx.human && !ctx.json => {
            let envs: Vec<String> = app.envs.keys().cloned().collect();
            pick("Environment", &envs, Some(&current))?
        }
        None => project.env.clone(),
    };
    get_env(&store.data, &project.app, &env)?; // unknown env → clear error before the guard
    guard_switch(ctx, &store.data, &project.app, &env)?;

    let vars = resolve_env(&store.data, &project.app, &env)?;
    let result = write_env_file(&project, &env, &vars)?;
    if ctx.json {
        emit_json(&merge(json!({ "ok": true, "app": project.app, "env": env, "file": project.file }), result.json_fields()));
    } else {
        print_apply(&project.app, &env, &project.file, project.format.name(), &result);
    }
    Ok(code::OK)
}
