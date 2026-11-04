use super::Ctx;
use crate::args::{flag, parse};
use crate::doctor::{Check, fix_all, run_checks, runner, summarize};
use crate::errors::{Res, code};
use crate::output::{dim, emit_json, green, red, say, yellow};
use serde_json::json;

pub const HELP: &str = "poenv doctor [--fix]

Check this machine's poenv setup and this repo. --fix does the safe fixes
(stale locks, gitignore, agent notes, refreshing an out-of-date env file).
Anything that needs you (creating the vault, your recovery phrase, sudo) is
listed with the exact command. Exit 5 means: only the user can fix what's left.";

pub fn run(ctx: &Ctx, args: &[String]) -> Res<i32> {
    let p = parse("doctor", args, &[flag("fix")])?;
    let (fixed, checks) = if p.flag("fix") { fix_all(&ctx.cwd) } else { (vec![], run_checks(&ctx.cwd)) };
    let (ok, waiting, only_user) = summarize(&checks);
    let exit = if ok {
        code::OK
    } else if only_user {
        code::WAITING_ON_USER
    } else {
        code::ERROR
    };

    if ctx.json {
        let list: Vec<_> = checks.iter().map(Check::to_json).collect();
        emit_json(&json!({
            "ok": ok, "waitingOnUser": waiting, "onlyUserCanFix": only_user, "fixed": fixed, "runner": runner(), "checks": list,
        }));
        return Ok(exit);
    }
    for c in &checks {
        say(&line(c));
    }
    say("");
    if !fixed.is_empty() {
        say(&green(&format!("Fixed: {}", fixed.join(", "))));
    }
    if ok && checks.iter().all(|c| c.status == "ok") {
        say(&green("All good."));
    } else if checks.iter().any(|c| c.status != "ok" && c.auto_fix.is_some()) {
        say(&dim("Run poenv doctor --fix for the ones marked (auto)."));
    }
    Ok(exit)
}

fn line(c: &Check) -> String {
    let icon = match c.status {
        "ok" => green("✓"),
        "warn" => yellow("!"),
        _ => red("✗"),
    };
    let mut out = format!("{icon} {}", c.message);
    if c.status != "ok" {
        if let Some(fix) = &c.fix {
            let tag = if c.auto_fix.is_some() {
                dim(" (auto)")
            } else if c.needs_user {
                dim(" (needs you)")
            } else {
                String::new()
            };
            out += &format!("\n    {} {fix}{tag}", dim("Fix:"));
        }
    }
    out
}
