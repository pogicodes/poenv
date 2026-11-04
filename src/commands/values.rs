// poenv values: show or change where this repo's values live. Never prints a value.
use super::{Ctx, refresh_file, require_app, require_human};
use crate::args::parse;
use crate::errors::{PoenvError, Res, code};
use crate::output::{dim, emit_json, ok, say};
use crate::project::{ValuesMode, require_project, write_marker};
use crate::store::open_store;
use serde_json::json;

pub const HELP: &str = "poenv values [file|run]

Show or change where this repo's values live.
  file   in the env file, for every tool that reads it (the default)
  run    only inside \"poenv run\"; the env file lists key names with empty
         values, so an agent or tool reading it finds nothing

Start tools that read the env file with: poenv run -- <command>
Switching back to file writes values where agents can read them, so it's
human-only.";

pub fn run(ctx: &Ctx, args: &[String]) -> Res<i32> {
    let p = parse("values", args, &[])?;
    let project = require_project(&ctx.cwd)?;
    if p.positionals.len() > 1 {
        return Err(PoenvError::fix("Expected file or run.", code::ERROR, "poenv values --help"));
    }
    let Some(mode_name) = p.positionals.first() else {
        let v = project.values.name();
        if ctx.json {
            emit_json(&json!({ "ok": true, "values": v }));
        } else {
            let note = if project.values == ValuesMode::Run { "(values only inside poenv run)" } else { "(values in the env file)" };
            say(&format!("{v} {}", dim(note)));
        }
        return Ok(code::OK);
    };
    let mode = match mode_name.as_str() {
        "file" => ValuesMode::File,
        "run" => ValuesMode::Run,
        other => {
            return Err(PoenvError::fix(
                format!("Unknown mode \"{other}\". Expected file or run."),
                code::ERROR,
                "poenv values --help",
            ));
        }
    };
    if mode == ValuesMode::File && project.values != ValuesMode::File {
        require_human(ctx, "values file", "writes values into the env file, where agents can read them")?;
    }

    let changed = mode != project.values;
    let mut target = project.clone();
    target.values = mode;
    if changed {
        write_marker(&target)?;
    }
    let store = open_store()?;
    require_app(&store.data, &target)?;
    let written = refresh_file(&target, &store.data)?;

    if ctx.json {
        emit_json(&json!({
            "ok": true,
            "values": mode.name(),
            "changed": changed,
            "file": written.as_ref().map(|_| target.file.clone()),
            "env": written.as_ref().map(|(e, _)| e.clone()),
        }));
        return Ok(code::OK);
    }
    if !changed {
        ok(&format!("Already {}", mode.name()));
    } else if mode == ValuesMode::Run {
        let note = if written.is_some() { format!(" ({} lists key names only)", target.file) } else { String::new() };
        ok(&format!("Values now live only inside poenv run{note}"));
    } else {
        let note = match &written {
            Some((e, _)) => format!(" (on {e})"),
            None => String::new(),
        };
        ok(&format!("Values are back in {}{note}", target.file));
    }
    if mode == ValuesMode::Run {
        say(&dim("  Start tools that read the env file with: poenv run -- <command>"));
    }
    Ok(code::OK)
}
