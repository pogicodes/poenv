// Human-readable output. Never prints secret values.
use crate::envfile::ApplyResult;
use crate::repos::{Refreshed, Skipped};
use crate::util::join_display;
use std::io::{IsTerminal, Write};
use std::sync::OnceLock;

fn use_color() -> bool {
    static C: OnceLock<bool> = OnceLock::new();
    *C.get_or_init(|| std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none())
}

fn paint(code: u8, s: &str) -> String {
    if use_color() { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_string() }
}
pub fn green(s: &str) -> String {
    paint(32, s)
}
pub fn yellow(s: &str) -> String {
    paint(33, s)
}
pub fn red(s: &str) -> String {
    paint(31, s)
}
pub fn dim(s: &str) -> String {
    paint(2, s)
}
pub fn bold(s: &str) -> String {
    paint(1, s)
}

pub fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(line.as_bytes());
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}
pub fn ok(line: &str) {
    say(&format!("{} {line}", green("✓")));
}
pub fn warn(line: &str) {
    say(&format!("{} {line}", yellow("!")));
}
pub fn eprint(text: &str) {
    let mut err = std::io::stderr().lock();
    let _ = err.write_all(text.as_bytes());
    let _ = err.flush();
}

pub fn emit_json(value: &serde_json::Value) {
    say(&serde_json::to_string(value).unwrap_or_default());
}

/// The summary after an env file write: where it moved from/to, and which keys changed.
pub fn print_apply(app: &str, env: &str, file: &str, format: &str, r: &ApplyResult) {
    let from = match &r.from {
        Some(f) if f.app == app && f.env != env => format!("{} → ", f.env),
        _ => String::new(),
    };
    ok(&format!("{}: {from}{}  {}", bold(app), bold(env), dim(&format!("({file})"))));
    let mut lines: Vec<String> = Vec::new();
    lines.extend(r.diff.added.iter().map(|k| format!("  {} {k}", green("+"))));
    lines.extend(r.diff.changed.iter().map(|k| format!("  {} {k}", yellow("~"))));
    lines.extend(r.diff.removed.iter().map(|k| format!("  {} {k}", red("-"))));
    if lines.is_empty() {
        say(&dim("  (no changes)"));
    } else {
        lines.iter().for_each(|l| say(l));
    }
    for k in &r.conflicts {
        warn(&format!("{k} is also set below the poenv block in {file}; your value there wins."));
    }
    for k in &r.lossy {
        warn(&format!("{k} can't be written exactly for format \"{format}\". Use: poenv run {env} -- <command>"));
    }
}

/// Other repos' env files a change rewrote (keys only), and the ones it couldn't, with how to catch up.
pub fn print_others(updated: &[Refreshed], skipped: &[Skipped]) {
    if !updated.is_empty() {
        let n = updated.len();
        ok(&format!("Also updated {n} other env file{}:", if n == 1 { "" } else { "s" }));
        for o in updated {
            let keys: Vec<&str> = o
                .result
                .diff
                .added
                .iter()
                .chain(&o.result.diff.changed)
                .chain(&o.result.diff.removed)
                .map(|s| s.as_str())
                .collect();
            say(&format!(
                "  {} {}  {}",
                yellow("~"),
                join_display(&o.root, &o.file),
                dim(&format!("({}/{}: {})", o.app, o.env, keys.join(", ")))
            ));
        }
        say(&dim("  Restart apps that are already running there to pick up the change."));
    }
    for s in skipped {
        let where_ = match &s.file {
            Some(f) => join_display(&s.root, f),
            None => s.root.clone(),
        };
        warn(&format!("Couldn't update {where_} ({}). Run there: poenv use", s.reason));
    }
}

/// The recovery phrase on one plain line (no numbers, no line breaks), so it pastes into a one-line prompt as-is.
pub fn show_phrase(phrase: &str) {
    say("");
    say(&format!("{}{}", bold("Your poenv recovery phrase"), dim(" (shown once, never stored; copy the whole line):")));
    say("");
    say(&phrase.split_whitespace().collect::<Vec<_>>().join(" "));
    say("");
    say("Save it in your password manager. It opens your backups on any machine, and gets this");
    say(&format!("machine back into your vault if its key is ever lost ({}).", bold("poenv recover")));
}
