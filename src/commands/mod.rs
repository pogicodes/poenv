pub mod doctor;
pub mod envs;
pub mod file;
pub mod info;
pub mod init;
pub mod run;
pub mod secrets;
pub mod use_env;
pub mod values;
pub mod vars;

use crate::envfile::ApplyResult;
use crate::errors::{PoenvError, Res, code};
use crate::project::{Project, current_env, has_block, write_env_file};
use crate::prompt::ask;
use crate::store::model::{App, Store, Target, Use, get_env, resolve_env, shared_uses};
use std::path::PathBuf;

pub struct Ctx {
    pub cwd: PathBuf,
    /// --json: machine output on stdout, no prompts.
    pub json: bool,
    /// A person at a terminal (TTY and no agent markers). Prompts are allowed only then.
    pub human: bool,
}

/// Protected envs need a person to type the env's name. Agents and scripts are refused.
pub fn guard_protected(ctx: &Ctx, store: &Store, app: &str, env: &str, doing: &str, command: &str) -> Res<()> {
    if !get_env(store, app, env)?.protected {
        return Ok(());
    }
    if !ctx.human || ctx.json {
        return Err(PoenvError::fix(
            format!("{app}/{env} is protected. Only a person at a terminal can {doing}."),
            code::REFUSED,
            format!("ask the user to run \"{command}\" themselves"),
        ));
    }
    let typed = ask(&format!("{app}/{env} is protected. Type \"{env}\" to continue"), "")?;
    if typed != env {
        return Err(PoenvError::code("Cancelled.", code::REFUSED));
    }
    Ok(())
}

/// The default guard: switching to a protected env.
pub fn guard_switch(ctx: &Ctx, store: &Store, app: &str, env: &str) -> Res<()> {
    guard_protected(ctx, store, app, env, "switch to it", &format!("poenv use {env}"))
}

/// Changing a protected env needs a person, and so does changing a shared key that a protected env
/// uses: through the reference, it's that env's value too.
pub fn guard_change(ctx: &Ctx, store: &Store, target: &Target, key: &str, command: &str) -> Res<()> {
    match target {
        Target::App { app, env } => guard_protected(ctx, store, app, env, "change it", command),
        Target::Shared => match protected_use(store, key) {
            Some(u) => guard_protected(ctx, store, &u.app, &u.env, &format!("change shared {key}, which it uses"), command),
            None => Ok(()),
        },
    }
}

/// A protected env that references shared `key`, if any.
pub fn protected_use(store: &Store, key: &str) -> Option<Use> {
    shared_uses(store, key).into_iter().find(|u| get_env(store, &u.app, &u.env).is_ok_and(|e| e.protected))
}

/// After a store change, rewrite the env file if it holds this app's block.
pub fn refresh_file(project: &Project, store: &Store) -> Res<Option<(String, ApplyResult)>> {
    if !has_block(project)? {
        return Ok(None);
    }
    let env = current_env(project)?;
    let vars = resolve_env(store, &project.app, &env)?;
    Ok(Some((env.clone(), write_env_file(project, &env, &vars)?)))
}

pub fn require_app<'a>(store: &'a Store, project: &Project) -> Res<&'a App> {
    store.apps.get(&project.app).ok_or_else(|| {
        PoenvError::fix(
            format!("This repo's app \"{}\" (from .poenv) isn't in your poenv vault on this machine.", project.app),
            code::UNKNOWN,
            "poenv init",
        )
    })
}

/// --shared → the shared pool; otherwise this repo's app, at --env or the env it's on now.
pub fn target_for(shared: bool, env: Option<String>, project: Option<&Project>) -> Res<Target> {
    if shared {
        return Ok(Target::Shared);
    }
    let project = project.expect("project required when not --shared");
    Ok(Target::App { app: project.app.clone(), env: match env {
        Some(e) => e,
        None => current_env(project)?,
    } })
}

/// Commands that reveal values (or replace the vault) need a person at a terminal.
pub fn require_human(ctx: &Ctx, command: &str, why: &str) -> Res<()> {
    if ctx.human {
        return Ok(());
    }
    Err(PoenvError::fix(
        format!("\"poenv {command}\" {why}, so only a person at a terminal can run it."),
        code::REFUSED,
        format!("ask the user to run \"poenv {command}\" themselves"),
    ))
}

pub fn one(positionals: &[String], command: &str, what: &str) -> Res<String> {
    if positionals.len() != 1 {
        return Err(PoenvError::fix(format!("Expected one {what}."), code::ERROR, format!("poenv {command} --help")));
    }
    Ok(positionals[0].clone())
}

/// `{ ...a, ...b }` for JSON objects.
pub fn merge(mut a: serde_json::Value, b: serde_json::Map<String, serde_json::Value>) -> serde_json::Value {
    if let Some(m) = a.as_object_mut() {
        m.extend(b);
    }
    a
}
