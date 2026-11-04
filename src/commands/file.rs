// poenv file: show or change which env file this repo's poenv block lives in. Never prints a value.
use super::Ctx;
use crate::args::parse;
use crate::envfile::{parse_env_file, remove_block, write_block};
use crate::errors::{PoenvError, Res, code};
use crate::output::{dim, emit_json, ok, say, warn};
use crate::project::{MARKER, require_project, write_marker};
use crate::repo::{Ignored, ensure_ignored, is_tracked};
use crate::util::{exists, file_mode, normalize, write_file_atomic};
use serde_json::json;
use std::fs;
use std::path::Path;

pub const HELP: &str = "poenv file [NAME]

Show the env file poenv manages in this repo, or switch it to NAME (a path
inside the repo, like .env or apps/web/.env.local). The poenv block moves to
NAME, your own lines stay where they are, and NAME is gitignored.";

pub fn run(ctx: &Ctx, args: &[String]) -> Res<i32> {
    let p = parse("file", args, &[])?;
    let project = require_project(&ctx.cwd)?;
    if p.positionals.len() > 1 {
        return Err(PoenvError::fix("Expected one file name.", code::ERROR, "poenv file --help"));
    }
    let Some(name) = p.positionals.first() else {
        if ctx.json {
            emit_json(&json!({ "ok": true, "file": project.file, "format": project.format.name() }));
        } else {
            say(&format!("{} {}", project.file, dim(&format!("(format: {})", project.format.name()))));
        }
        return Ok(code::OK);
    };

    let next = normalize(name);
    if Path::new(&next).is_absolute() || next.starts_with("..") || next == MARKER {
        return Err(PoenvError::new(format!("\"{name}\" must be a file inside this repo (and not {MARKER}).")));
    }
    // Gitignoring a tracked file doesn't stop git: the next commit would publish the values.
    if is_tracked(&project.root, &next) {
        return Err(PoenvError::fix(
            format!("git tracks {next}, so values written there would be committed."),
            code::ERROR,
            format!("pick an untracked file like .env, or untrack it first: git rm --cached {next}"),
        ));
    }
    let from = project.file.clone();
    if next == normalize(&from) {
        if ctx.json {
            emit_json(&json!({ "ok": true, "from": from, "to": from, "moved": false }));
        } else {
            ok(&format!("Already using {from}"));
        }
        return Ok(code::OK);
    }

    // Move the block as it is: no vault needed, and the new file stays on the same env.
    let old_path = project.env_file_path();
    let old_text = if exists(&old_path) { fs::read_to_string(&old_path)? } else { String::new() };
    let block = parse_env_file(&old_text, project.format)?.block.filter(|b| b.app == project.app);
    let mut target = project.clone();
    target.file = next.clone();
    if let Some(b) = &block {
        let new_path = target.env_file_path();
        let present = exists(&new_path);
        let text = if present { fs::read_to_string(&new_path)? } else { String::new() };
        let mode = if present { file_mode(&new_path) } else { 0o600 };
        write_file_atomic(&new_path, write_block(&text, &b.app, &b.env, &b.vars, project.format)?.as_bytes(), mode)?;
        let rest = remove_block(&old_text)?;
        if rest.trim().is_empty() {
            fs::remove_file(&old_path)?;
        } else {
            write_file_atomic(&old_path, rest.as_bytes(), file_mode(&old_path))?;
        }
    }
    write_marker(&target)?;
    let ignored = ensure_ignored(&project.root, &next)?;

    if ctx.json {
        emit_json(&json!({
            "ok": true,
            "from": from,
            "to": next,
            "moved": block.is_some(),
            "env": block.as_ref().map(|b| b.env.clone()),
            "gitignore": ignored.name(),
        }));
        return Ok(code::OK);
    }
    let moved = match &block {
        Some(b) => format!(" (moved the {} block from {from})", b.env),
        None => String::new(),
    };
    ok(&format!("poenv now manages {next}{moved}"));
    if ignored == Ignored::Added {
        ok(&format!("Added {next} to .gitignore"));
    }
    if ignored == Ignored::NotGit {
        warn(&format!("Not a git repo, so {next} isn't gitignored. Keep it out of version control."));
    }
    if block.is_none() {
        say(&dim("  Write it with: poenv use"));
    }
    Ok(code::OK)
}
