// Human-only commands: they show values or replace the vault. Agents are refused (SPEC §5).
use super::vars::finish;
use super::{Ctx, one, require_human, target_for};
use crate::args::{Opt, flag, parse, string};
use crate::errors::{PoenvError, Res, code};
use crate::output::{bold, dim, emit_json, ok, say, show_phrase};
use crate::project::{find_project, require_project};
use crate::prompt::{ask_hidden, confirm};
use crate::store::model::{Store, Target, Var, Variable, resolve_env, rollback_var, var_at};
use crate::store::{export_store, import_store, open_store, paths, phrase_matches, recover_store, rekey_store};
use crate::util::{exists, hex, js_len, now_iso};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::io::Write;

pub const GET_HELP: &str = "poenv get KEY [--shared] [--env ENV]

Print a value. Human-only: refused for agents, scripts and pipes.";

pub const HISTORY_HELP: &str = "poenv history KEY [--shared] [--env ENV] [--show]

List a key's previous values (last 10), newest first. Values are masked
unless --show. Human-only.";

pub const ROLLBACK_HELP: &str = "poenv rollback KEY [N] [--shared] [--env ENV]

Restore version N from poenv history (default 0, the previous value).
The current value stays in history, so a rollback can be undone. Human-only.";

pub const BACKUP_HELP: &str = "poenv backup [FILE] [--force]

Write an encrypted backup of your whole vault (default: poenv-backup-DATE.enc).
It opens only with your recovery phrase. Human-only.";

pub const RESTORE_HELP: &str = "poenv restore FILE

Restore a backup as this machine's vault. On a new machine you'll be asked
for the backup's recovery phrase. Your current vault is kept as a .bak file.
Human-only.";

pub const RECOVER_HELP: &str = "poenv recover [--check | --new-phrase]

Type your 24-word recovery phrase to rebuild this machine's key for the vault
(when keys.json is lost or unreadable). Human-only.
  --check       type your phrase to confirm the copy you saved is right
  --new-phrase  lost your phrase? make a new one (re-encrypts the vault)";

const KEY_OPTS: [Opt; 2] = [flag("shared"), string("env")];

const DIFFERENT_VAULT: &str =
    "That is a valid recovery phrase, but for a different vault (did poenv init run more than once?).";

fn target(ctx: &Ctx, shared: bool, env: Option<String>) -> Res<Target> {
    if shared {
        return Ok(Target::Shared);
    }
    let project = require_project(&ctx.cwd)?;
    target_for(false, env, Some(&project))
}

pub fn get(ctx: &Ctx, args: &[String]) -> Res<i32> {
    require_human(ctx, "get", "prints secret values")?;
    let p = parse("get", args, &KEY_OPTS)?;
    let key = one(&p.positionals, "get", "KEY")?;
    let store = open_store()?.data;
    let t = target(ctx, p.flag("shared"), p.str("env"))?;
    let value = match &t {
        Target::Shared => store.shared.get(&key).map(|v| v.value.clone()),
        Target::App { app, env } => resolve_env(&store, app, env)?.get(&key).cloned(),
    };
    let Some(value) = value else {
        return Err(PoenvError::fix(format!("No key \"{key}\" in {}.", t.describe()), code::UNKNOWN, "poenv ls"));
    };
    if ctx.json {
        emit_json(&json!({ "ok": true, "key": key, "value": value }));
    } else {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(format!("{value}\n").as_bytes());
    }
    Ok(code::OK)
}

pub fn history(ctx: &Ctx, args: &[String]) -> Res<i32> {
    require_human(ctx, "history", "shows past secret values")?;
    let mut opts = KEY_OPTS.to_vec();
    opts.push(flag("show"));
    let p = parse("history", args, &opts)?;
    let key = one(&p.positionals, "history", "KEY")?;
    let store = open_store()?.data;
    let t = target(ctx, p.flag("shared"), p.str("env"))?;
    let v = literal(&store, &t, &key)?;
    let show_values = p.flag("show");
    let show = |s: &str| if show_values { s.to_string() } else { mask(s, true) };

    if ctx.json {
        let mut current = Map::new();
        current.insert("at".into(), v.updated_at.clone().into());
        if show_values {
            current.insert("value".into(), v.value.clone().into());
        }
        let history: Vec<Value> = v
            .history
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let mut m = Map::new();
                m.insert("index".into(), i.into());
                m.insert("at".into(), h.at.clone().into());
                if show_values {
                    m.insert("value".into(), h.value.clone().into());
                }
                Value::Object(m)
            })
            .collect();
        emit_json(&json!({ "ok": true, "key": key, "current": current, "history": history }));
        return Ok(code::OK);
    }
    say(&format!("{} {}", bold(&key), dim(&format!("in {}", t.describe()))));
    say(&format!("  {:<4} {}  {}", "now", when(&v.updated_at), show(&v.value)));
    for (i, h) in v.history.iter().enumerate() {
        say(&format!("  {:<4} {}  {}", i.to_string(), when(&h.at), show(&h.value)));
    }
    if v.history.is_empty() {
        say(&dim("  (no earlier versions)"));
    } else {
        let suffix = if p.flag("shared") { " --shared" } else { "" };
        say(&dim(&format!("  Restore one with: poenv rollback {key} <N>{suffix}")));
    }
    Ok(code::OK)
}

pub fn rollback(ctx: &Ctx, args: &[String]) -> Res<i32> {
    require_human(ctx, "rollback", "changes secret values from their history")?;
    let p = parse("rollback", args, &KEY_OPTS)?;
    if p.positionals.is_empty() || p.positionals.len() > 2 {
        return Err(PoenvError::fix("Expected: poenv rollback KEY [N]", code::ERROR, "poenv rollback --help"));
    }
    let key = p.positionals[0].clone();
    let raw = p.positionals.get(1).cloned();
    let index = match &raw {
        None => 0,
        Some(n) => match n.trim().parse::<usize>() {
            Ok(i) => i,
            Err(_) => return Err(PoenvError::new(format!("N must be 0 or more (got \"{n}\")."))),
        },
    };
    let shared = p.flag("shared");
    let project = if shared { find_project(&ctx.cwd)? } else { Some(require_project(&ctx.cwd)?) };
    let mut store = open_store()?;
    let t = target_for(shared, p.str("env"), project.as_ref())?;
    let Some(version) = literal(&store.data, &t, &key)?.history.get(index).cloned() else {
        return Err(PoenvError::fix(format!("{key} has no version {index}."), code::UNKNOWN, format!("poenv history {key}")));
    };
    if !confirm(&format!("Restore {key} in {} to the value set {}?", t.describe(), when(&version.at)), false)? {
        return Err(PoenvError::code("Cancelled.", code::REFUSED));
    }
    store.update(|s| rollback_var(s, &t, &key, index))?;
    let mut extra = Map::new();
    extra.insert("key".into(), key.clone().into());
    extra.insert("target".into(), t.to_json());
    extra.insert("index".into(), index.into());
    finish(ctx, project.as_ref(), &store.data, &format!("Restored {key} in {} (version {index})", t.describe()), extra)
}

pub fn backup(ctx: &Ctx, args: &[String]) -> Res<i32> {
    require_human(ctx, "backup", "writes a copy of your whole vault")?;
    let p = parse("backup", args, &[flag("force")])?;
    let date = &now_iso()[..10];
    let name = p.positionals.first().cloned().unwrap_or_else(|| format!("poenv-backup-{date}.enc"));
    let dest = ctx.cwd.join(&name);
    open_store()?;
    let apps = export_store(&dest, p.flag("force"))?;
    let shown = dest.display().to_string();
    if ctx.json {
        emit_json(&json!({ "ok": true, "file": shown, "apps": apps }));
        return Ok(code::OK);
    }
    ok(&format!("Backed up {} app{} to {shown}", apps.len(), if apps.len() == 1 { "" } else { "s" }));
    say(&dim("  It's encrypted and opens only with your recovery phrase. Restore with: poenv restore <file>"));
    Ok(code::OK)
}

pub fn restore(ctx: &Ctx, args: &[String]) -> Res<i32> {
    require_human(ctx, "restore", "replaces your vault")?;
    let p = parse("restore", args, &[])?;
    let src = ctx.cwd.join(one(&p.positionals, "restore", "FILE")?);
    let has_vault = exists(&paths().store);
    if has_vault
        && !confirm("Replace the vault on this machine with this backup? (the current one is kept as a .bak file)", false)?
    {
        return Err(PoenvError::code("Cancelled.", code::REFUSED));
    }
    let (previous, apps) = import_store(&src, || ask_hidden("Recovery phrase for this backup (hidden): "))?;
    let shown = src.display().to_string();
    if ctx.json {
        emit_json(&json!({ "ok": true, "apps": apps, "previous": previous }));
        return Ok(code::OK);
    }
    ok(&format!("Restored {} app{} from {shown}", apps.len(), if apps.len() == 1 { "" } else { "s" }));
    if let Some(prev) = previous {
        say(&dim(&format!("  Previous vault kept at {prev}")));
    }
    Ok(code::OK)
}

pub fn recover(ctx: &Ctx, args: &[String]) -> Res<i32> {
    require_human(ctx, "recover", "asks for your recovery phrase")?;
    let p = parse("recover", args, &[flag("check"), flag("new-phrase")])?;

    if p.flag("check") {
        let mut store = open_store()?;
        if !phrase_matches(&ask_hidden("Recovery phrase (24 words, hidden): ")?)? {
            return Err(PoenvError::new(DIFFERENT_VAULT));
        }
        store.update(|s| {
            s.meta.recovery_confirmed = true;
            Ok(())
        })?;
        ok("Your recovery phrase is right. Keep it safe.");
        return Ok(code::OK);
    }

    if p.flag("new-phrase") {
        open_store()?;
        say("This makes a new recovery phrase and re-encrypts your vault with it. The old phrase");
        say("stops working, and backups made before now still need the old one.");
        if !confirm("Make a new recovery phrase?", false)? {
            return Err(PoenvError::code("Cancelled.", code::REFUSED));
        }
        show_phrase(&rekey_store()?);
        ok("New recovery phrase set.");
        say(&dim("  Confirm you saved it with: poenv recover --check"));
        return Ok(code::OK);
    }

    let phrase = ask_hidden("Recovery phrase (24 words, hidden): ")?;
    if !phrase_matches(&phrase)? {
        return Err(PoenvError::new(DIFFERENT_VAULT));
    }
    recover_store(&phrase)?;
    open_store()?.update(|s| {
        s.meta.recovery_confirmed = true; // typing it just proved it's saved
        Ok(())
    })?;
    ok("Recovered. This machine opens your vault again.");
    Ok(code::OK)
}

/// The stored (non-reference) variable, or an error pointing at what to do instead.
fn literal(store: &Store, t: &Target, key: &str) -> Res<Variable> {
    match var_at(store, t, key)? {
        None => Err(PoenvError::fix(format!("No key \"{key}\" in {}.", t.describe()), code::UNKNOWN, "poenv ls")),
        Some(Var::Ref(r)) => Err(PoenvError::fix(
            format!("{key} in {} is a reference to shared {}; its history lives there.", t.describe(), r.target),
            code::UNKNOWN,
            format!("poenv history {} --shared", r.target),
        )),
        Some(Var::Lit(v)) => Ok(v),
    }
}

/// Enough to tell versions apart (a 4-hex fingerprint), not enough to leak a secret.
pub fn mask(value: &str, styled: bool) -> String {
    let style = |s: &str| if styled { dim(s) } else { s.to_string() };
    let fp = style(&format!("#{}", &hex(&Sha256::digest(value.as_bytes()))[..4]));
    let len = js_len(value);
    if len < 12 {
        return format!("{} {fp}", "•".repeat(len.max(3)));
    }
    let chars: Vec<char> = value.chars().collect();
    let head: String = chars.iter().take(3).collect();
    let tail: String = chars[chars.len().saturating_sub(2)..].iter().collect();
    format!("{head}…{tail} {fp} {}", style(&format!("({len} chars)")))
}

fn when(iso: &str) -> String {
    iso.replacen('T', " ", 1).chars().take(16).collect()
}

#[cfg(test)]
mod tests {
    use super::mask;

    #[test]
    fn mask_hides_short_values_entirely_and_long_ones_mostly() {
        let short = mask("short", false);
        assert!(short.starts_with("••••• #") && short.chars().count() == 11);
        let long = mask("sk-abcdefghijklmnop", false);
        assert!(long.starts_with("sk-…op #") && !long.contains("abcdef"));
        assert_ne!(mask("postgres://v1-db-host/app", false), mask("postgres://v2-db-host/app", false));
        assert_eq!(mask("same-value-here!", false), mask("same-value-here!", false));
    }
}
