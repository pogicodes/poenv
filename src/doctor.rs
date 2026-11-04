// poenv doctor: check the machine and this repo, and fix what's safe to fix. SPEC §9.
use crate::envfile::{apply_env, parse_env_file};
use crate::project::{MARKER, Project, current_env, file_vars, find_project};
use crate::repo::{SNIPPET_MARK, check_ignore, ensure_agent_notes, ensure_ignored, is_tracked};
use crate::store::lock::is_stale;
use crate::store::model::{Store, Var, resolve_env};
use crate::store::{RECOVER_FIX, open_store, paths};
use crate::util::{exists, which};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub enum Fix {
    RemoveFile(PathBuf),
    RemoveFiles(Vec<PathBuf>),
    Ignore(String, String),
    AgentNotes(String),
    Refresh(Project, String, crate::envfile::Vars),
}

#[derive(Clone)]
pub struct Check {
    pub id: &'static str,
    pub status: &'static str,
    pub message: String,
    /// The exact command (or step) that resolves it.
    pub fix: Option<String>,
    /// Only a person can do the fix.
    pub needs_user: bool,
    /// `doctor --fix` can do it safely.
    pub auto_fix: Option<Fix>,
}

fn check(id: &'static str, status: &'static str, message: impl Into<String>) -> Check {
    Check { id, status, message: message.into(), fix: None, needs_user: false, auto_fix: None }
}

impl Check {
    fn fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }
    fn user(mut self) -> Self {
        self.needs_user = true;
        self
    }
    fn auto(mut self, f: Fix) -> Self {
        self.auto_fix = Some(f);
        self
    }
    pub fn to_json(&self) -> Value {
        let mut m = serde_json::Map::new();
        m.insert("id".into(), self.id.into());
        m.insert("status".into(), self.status.into());
        m.insert("message".into(), self.message.clone().into());
        if let Some(f) = &self.fix {
            m.insert("fix".into(), f.clone().into());
        }
        if self.needs_user {
            m.insert("needsUser".into(), true.into());
        }
        m.insert("autoFixable".into(), self.auto_fix.is_some().into());
        Value::Object(m)
    }
}

pub fn run_checks(cwd: &Path) -> Vec<Check> {
    let p = paths();
    let mut checks = vec![check_binary()];
    let mut store: Option<Store> = None;
    if !exists(&p.store) {
        checks.push(
            check("vault", "fail", "No poenv vault on this machine yet.")
                .fix("poenv init (in a terminal: it shows your recovery phrase)")
                .user(),
        );
    } else {
        match open_store() {
            Ok(h) => {
                store = Some(h.data);
                checks.push(check("vault", "ok", "Vault opens."));
            }
            Err(e) => {
                let message =
                    if e.unexpected { format!("The vault can't be opened: {}", e.problem) } else { e.problem.clone() };
                let fix = if e.fix.as_deref() == Some(RECOVER_FIX) { RECOVER_FIX } else { "poenv restore <your latest backup>" };
                checks.push(check("vault", "fail", message).fix(fix).user());
            }
        }
    }
    if let Some(s) = &store {
        checks.push(if s.meta.recovery_confirmed {
            check("recovery-phrase", "ok", "Recovery phrase confirmed saved.")
        } else {
            check(
                "recovery-phrase",
                "warn",
                "You haven't confirmed your recovery phrase is saved. Without it, a lost key or machine means lost secrets.",
            )
            .fix("poenv recover --check (type the phrase once to confirm it's right)")
            .user()
        });
    }
    checks.extend(check_leftovers(&p.home, &p.lock));
    if let Ok(Some(project)) = find_project(cwd) {
        checks.extend(check_project(&project, store.as_ref()));
    }
    checks
}

/// Runs every safe fix, then checks again.
pub fn fix_all(cwd: &Path) -> (Vec<String>, Vec<Check>) {
    let mut fixed = vec![];
    for c in run_checks(cwd) {
        if c.status == "ok" {
            continue;
        }
        let Some(f) = c.auto_fix else { continue };
        if apply(f) {
            fixed.push(c.id.to_string());
        }
    }
    (fixed, run_checks(cwd))
}

fn apply(f: Fix) -> bool {
    match f {
        Fix::RemoveFile(p) => {
            let _ = fs::remove_file(p);
            true
        }
        Fix::RemoveFiles(ps) => {
            ps.into_iter().for_each(|p| {
                let _ = fs::remove_file(p);
            });
            true
        }
        Fix::Ignore(root, file) => ensure_ignored(&root, &file).is_ok(),
        Fix::AgentNotes(root) => ensure_agent_notes(&root).is_ok(),
        Fix::Refresh(project, env, want) => {
            apply_env(&project.env_file_path(), &project.app, &env, &want, project.format).is_ok()
        }
    }
}

pub fn summarize(checks: &[Check]) -> (bool, Vec<&'static str>, bool) {
    let failing: Vec<&Check> = checks.iter().filter(|c| c.status == "fail").collect();
    let waiting = checks.iter().filter(|c| c.status != "ok" && c.needs_user).map(|c| c.id).collect();
    (failing.is_empty(), waiting, !failing.is_empty() && failing.iter().all(|c| c.needs_user))
}

fn check_binary() -> Check {
    match which("poenv") {
        Some(path) => check("binary", "ok", format!("poenv is on PATH ({}).", path.display())),
        None => check("binary", "warn", "poenv isn't on your PATH (you're running it some other way).").fix(
            "curl -fsSL https://poenv.pogi.codes/install.sh | sh   (see https://github.com/pogicodes/poenv#install)",
        ),
    }
}

fn check_leftovers(home: &Path, lock: &Path) -> Vec<Check> {
    let mut checks = vec![];
    if exists(lock) && is_stale(lock) {
        checks.push(
            check("stale-lock", "warn", "A lock left by a crashed poenv command is still there.")
                .fix("poenv doctor --fix")
                .auto(Fix::RemoveFile(lock.to_path_buf())),
        );
    }
    let temps: Vec<PathBuf> = fs::read_dir(home)
        .map(|d| {
            d.filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().starts_with("store.enc.tmp-"))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    if !temps.is_empty() {
        checks.push(
            check("temp-files", "warn", format!("{} leftover temp file(s) from an interrupted write.", temps.len()))
                .fix("poenv doctor --fix")
                .auto(Fix::RemoveFiles(temps)),
        );
    }
    checks
}

fn check_project(project: &Project, store: Option<&Store>) -> Vec<Check> {
    let mut checks = vec![];
    let (root, file, app) = (&project.root, &project.file, &project.app);

    match check_ignore(root, file) {
        Some(1) => checks.push(
            check("gitignore", "warn", format!("{file} isn't gitignored, so secrets could be committed."))
                .fix("poenv doctor --fix")
                .auto(Fix::Ignore(root.clone(), file.clone())),
        ),
        Some(0) => checks.push(check("gitignore", "ok", format!("{file} is gitignored."))),
        _ => {}
    }

    // .poenv is per-person: gitignored by default, but a repo may choose to commit it.
    if !is_tracked(root, MARKER) {
        match check_ignore(root, MARKER) {
            Some(1) => checks.push(
                check("marker-gitignore", "warn", format!("{MARKER} isn't gitignored or committed."))
                    .fix("poenv doctor --fix")
                    .auto(Fix::Ignore(root.clone(), MARKER.into())),
            ),
            Some(0) => checks.push(check("marker-gitignore", "ok", format!("{MARKER} is gitignored."))),
            _ => {}
        }
    }

    let notes = ["AGENTS.md", "CLAUDE.md"]
        .iter()
        .any(|f| fs::read_to_string(Path::new(root).join(f)).is_ok_and(|t| t.contains(SNIPPET_MARK)));
    checks.push(if notes {
        check("agent-notes", "ok", "AGENTS.md/CLAUDE.md mention poenv.")
    } else {
        check("agent-notes", "warn", "No poenv notes for agents in AGENTS.md / CLAUDE.md.")
            .fix("poenv doctor --fix")
            .auto(Fix::AgentNotes(root.clone()))
    });

    let Some(store) = store else { return checks };
    let Some(a) = store.apps.get(app) else {
        checks.push(
            check(
                "app",
                "fail",
                format!("This repo's app \"{app}\" isn't in the vault on this machine (new machine or fresh clone?)."),
            )
            .fix("poenv init (creates it from .poenv)"),
        );
        return checks;
    };
    checks.push(check("app", "ok", format!("App \"{app}\" is in the vault.")));

    let mut dangling = vec![];
    for (env, e) in &a.envs {
        for (key, v) in &e.vars {
            if let Var::Ref(r) = v {
                if !store.shared.contains_key(&r.target) {
                    dangling.push((env.clone(), key.clone(), r.target.clone()));
                }
            }
        }
    }
    for (env, key, target) in &dangling {
        checks.push(
            check(
                "dangling-ref",
                "fail",
                format!("{key} in {app}/{env} points to shared {target}, which no longer exists."),
            )
            .fix(format!("poenv set {target} --shared   (or: poenv rm {key} --env {env})")),
        );
    }

    let path = project.env_file_path();
    let env = current_env(project).unwrap_or_else(|_| project.env.clone());
    let block = fs::read_to_string(&path).ok().and_then(|t| parse_env_file(&t, project.format).ok()).and_then(|p| p.block);
    match block {
        Some(b) if b.app == *app => {
            if !dangling.iter().any(|(e, _, _)| *e == env) {
                if let Ok(resolved) = resolve_env(store, app, &env) {
                    let want = file_vars(project, &resolved);
                    checks.push(if want == b.vars {
                        check("env-file", "ok", format!("{file} is on {env} and up to date."))
                    } else {
                        check(
                            "env-file",
                            "warn",
                            format!("{file} is on {env} but out of date (a value changed since it was written)."),
                        )
                        .fix("poenv doctor --fix")
                        .auto(Fix::Refresh(project.clone(), env.clone(), want))
                    });
                }
            }
        }
        _ => checks.push(check("env-file", "warn", format!("{file} has no poenv block yet.")).fix(format!("poenv use {env}"))),
    }
    checks
}

/// For --json: who's running doctor, so an agent knows which fixes to hand to the user.
pub fn runner() -> Value {
    json!({ "agent": crate::human::agent_marker() })
}
