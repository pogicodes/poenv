// The command line: parse argv, dispatch, report errors with stable exit codes.
use crate::commands::Ctx;
use crate::{human, project, repos, tui};
use crate::errors::{PoenvError, Res, code};
use crate::output::{bold, dim, emit_json, eprint, red, say};
use serde_json::json;

const VERSION: &str = env!("CARGO_PKG_VERSION");

type Run = fn(&Ctx, &[String]) -> Res<i32>;

fn command(name: &str) -> Option<(Run, &'static str)> {
    use crate::commands::*;
    Some(match name {
        "init" => (init::run, init::HELP),
        "use" => (use_env::run, use_env::HELP),
        "add" => (vars::add, vars::ADD_HELP),
        "set" => (vars::set, vars::SET_HELP),
        "import" => (vars::import_keys, vars::IMPORT_HELP),
        "rm" => (vars::rm, vars::RM_HELP),
        "env" => (envs::run, envs::HELP),
        "file" => (file::run, file::HELP),
        "values" => (values::run, values::HELP),
        "ls" => (info::ls, info::LS_HELP),
        "status" => (info::status, info::STATUS_HELP),
        "run" => (run::run, run::HELP),
        "get" => (secrets::get, secrets::GET_HELP),
        "history" => (secrets::history, secrets::HISTORY_HELP),
        "rollback" => (secrets::rollback, secrets::ROLLBACK_HELP),
        "backup" => (secrets::backup, secrets::BACKUP_HELP),
        "restore" => (secrets::restore, secrets::RESTORE_HELP),
        "recover" => (secrets::recover, secrets::RECOVER_HELP),
        "doctor" => (doctor::run, doctor::HELP),
        _ => return None,
    })
}

fn usage() -> String {
    format!(
        "{}: your configs and secrets, one command away.
Run {} on its own in a terminal to browse and edit everything.

{}
  poenv use [ENV]          switch this repo's env file to ENV
  poenv add KEY            add a shared key to this repo
  poenv status             which app/env this repo is on

{}
  poenv init               set up this repo
  poenv set KEY            set a value (asked for, or piped in)
  poenv import             save keys pasted into the env file
  poenv rm KEY             remove a key
  poenv env new NAME       add an env (--from ENV copies one)
  poenv env rename A B     rename an env
  poenv file [NAME]        show or change the env file (e.g. .env)
  poenv values [file|run]  keep values in the env file, or only in poenv run
  poenv ls [APP]           list apps, envs and key names
  poenv run [ENV] -- CMD   run CMD with ENV injected
  poenv doctor [--fix]     check (and fix) your setup

{} {}
  poenv get KEY            print a value
  poenv history KEY        previous values (masked)
  poenv rollback KEY [N]   restore a previous value
  poenv backup [FILE]      encrypted backup of your vault
  poenv restore FILE       restore a backup
  poenv recover            your recovery phrase: check it, or rebuild this machine's key

Add --json to any command for machine-readable output. {}",
        bold("poenv"),
        bold("poenv"),
        bold("Daily"),
        bold("Setup & management"),
        bold("Just for you"),
        dim("(refused for agents and scripts)"),
        dim("poenv <command> --help for details."),
    )
}

/// Runs the CLI and returns the exit code.
pub fn run(argv: Vec<String>) -> i32 {
    // Flags after `--` belong to the command `poenv run` starts, not to poenv.
    let sep = argv.iter().position(|a| a == "--");
    let own = &argv[..sep.unwrap_or(argv.len())];
    let json = own.iter().any(|a| a == "--json");
    let args: Vec<String> = argv
        .iter()
        .enumerate()
        .filter(|(i, a)| !(*a == "--json" && sep.is_none_or(|s| *i < s)))
        .map(|(_, a)| a.clone())
        .collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());

    // Every repo poenv runs in is remembered, so rotating a shared key can refresh all of them.
    if let Ok(Some(here)) = project::find_project(&cwd) {
        repos::remember_repo(&here.root);
    }

    let name = args.first().cloned();
    let rest: Vec<String> = args.iter().skip(1).cloned().collect();

    // No command: people get the TUI; agents and scripts get the help text.
    if name.is_none() || name.as_deref() == Some("ui") {
        if !json && human::is_interactive_human() {
            let ctx = Ctx { cwd, json: false, human: true };
            return match tui::run(&ctx) {
                Ok(c) => c,
                Err(e) => report(&e, json),
            };
        }
        if name.as_deref() == Some("ui") {
            return report(&PoenvError::code("The TUI needs a person at a terminal.", code::REFUSED), json);
        }
        say(&usage());
        return code::OK;
    }
    let name = name.unwrap();
    if matches!(name.as_str(), "help" | "--help" | "-h") {
        say(&usage());
        return code::OK;
    }
    if matches!(name.as_str(), "--version" | "-v") {
        say(VERSION);
        return code::OK;
    }

    let ctx = Ctx { cwd, json, human: human::is_interactive_human() };
    let Some((run, help)) = command(&name) else {
        return report(&PoenvError::fix(format!("Unknown command \"{name}\"."), code::ERROR, "poenv --help"), json);
    };
    let own_rest = match rest.iter().position(|a| a == "--") {
        Some(i) if sep.is_some() => &rest[..i],
        _ => &rest[..],
    };
    if own_rest.iter().any(|a| a == "--help" || a == "-h") {
        say(help);
        return code::OK;
    }
    match run(&ctx, &rest) {
        Ok(c) => c,
        Err(e) => report(&e, json),
    }
}

fn report(err: &PoenvError, json: bool) -> i32 {
    if err.unexpected {
        let message = format!("Unexpected error: {}", err.problem);
        if json {
            emit_json(&json!({ "ok": false, "error": message, "code": code::ERROR, "fix": null }));
        } else {
            eprint(&format!("{} {message}\n", red("✗")));
        }
        return code::ERROR;
    }
    if json {
        emit_json(&json!({ "ok": false, "error": err.problem, "code": err.code, "fix": err.fix }));
    } else {
        eprint(&format!("{} {}\n", red("✗"), err.problem));
        if let Some(fix) = &err.fix {
            eprint(&format!("  {} {fix}\n", dim("Run:")));
        }
    }
    err.code
}
