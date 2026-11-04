// The repos on this machine that use poenv, so a change in one place can refresh every env file it
// affects. Paths only, never values: kept beside the vault in plain JSON, and left out of backups.
use crate::envfile::{ApplyResult, parse_env_file};
use crate::errors::{PoenvError, Res};
use crate::project::{MARKER, Project, file_vars, read_marker, write_env_file};
use crate::store::model::{Store, resolve_env};
use crate::util::{exists, poenv_home, write_file_atomic};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

pub struct Refreshed {
    pub root: String,
    pub file: String,
    pub app: String,
    pub env: String,
    pub result: ApplyResult,
}

/// A repo whose env file may be stale because it couldn't be checked or written.
pub struct Skipped {
    pub root: String,
    pub file: Option<String>,
    pub reason: String,
}

fn registry_path() -> PathBuf {
    poenv_home().join("repos.json")
}

pub fn known_repos() -> Vec<String> {
    let Ok(text) = fs::read_to_string(registry_path()) else { return vec![] };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return vec![] };
    match v.get("repos").and_then(|r| r.as_array()) {
        Some(a) => a.iter().filter_map(|r| r.as_str().map(String::from)).collect(),
        None => vec![],
    }
}

fn save(roots: &[String]) -> Res<()> {
    let text = serde_json::to_string_pretty(&serde_json::json!({ "repos": roots }))? + "\n";
    write_file_atomic(&registry_path(), text.as_bytes(), 0o600)
}

/// The real path, without Windows' `\\?\` prefix.
pub fn realpath(path: &str) -> std::io::Result<String> {
    let real = fs::canonicalize(path)?.to_string_lossy().into_owned();
    Ok(match real.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => rest.to_string(),
        _ => real,
    })
}

/// Remembers a repo root. Never fails a command: before the vault exists there's nowhere to write.
pub fn remember_repo(root: &str) {
    if !exists(&poenv_home()) {
        return;
    }
    let Ok(real) = realpath(root) else { return };
    let mut roots = known_repos();
    if !roots.contains(&real) {
        roots.push(real);
        let _ = save(&roots);
    }
}

/// The known repos on this machine set up as `app` (clones and worktrees each count).
pub fn repos_for_app(app: &str) -> Vec<Project> {
    known_repos().into_iter().filter_map(|r| read_marker(&r).ok()).filter(|p| p.app == app).collect()
}

fn is_missing(path: &Path) -> bool {
    matches!(fs::metadata(path), Err(e) if e.kind() == ErrorKind::NotFound)
}

fn reason_of(err: &PoenvError) -> String {
    match err.io_kind {
        Some(ErrorKind::PermissionDenied) => "permission denied".into(),
        Some(ErrorKind::ReadOnlyFilesystem) => "read-only file system".into(),
        _ => err.problem.clone(),
    }
}

/// Rewrites every known repo's env file whose block no longer matches the vault, skipping `except`
/// (the repo the command ran in, refreshed by the caller). Repos whose `.poenv` is gone are forgotten;
/// repos that can't be read or written are reported as skipped, never failing the command.
pub fn refresh_other_repos(store: &Store, except: Option<&str>) -> (Vec<Refreshed>, Vec<Skipped>) {
    let skip = except.map(|e| realpath(e).unwrap_or_else(|_| e.to_string()));
    let roots = known_repos();
    // Only a missing .poenv means the repo is gone. A folder we may not read (a sandbox) is kept.
    let alive: Vec<String> = roots.iter().filter(|r| !is_missing(&Path::new(r).join(MARKER))).cloned().collect();
    if alive.len() != roots.len() {
        let _ = save(&alive);
    }
    let mut updated = vec![];
    let mut skipped = vec![];
    for root in alive {
        if Some(&root) == skip.as_ref() {
            continue;
        }
        let mut file = None;
        let attempt = (|| -> Res<Option<Refreshed>> {
            let project = read_marker(&root)?;
            file = Some(project.file.clone());
            let path = project.env_file_path();
            if is_missing(&path) {
                return Ok(None);
            }
            let Some(block) = parse_env_file(&fs::read_to_string(&path)?, project.format)?.block else {
                return Ok(None);
            };
            if block.app != project.app || block.env.is_empty() {
                return Ok(None);
            }
            if !store.apps.get(&project.app).is_some_and(|a| a.envs.contains_key(&block.env)) {
                return Ok(None);
            }
            let want = resolve_env(store, &project.app, &block.env)?;
            if block.vars == file_vars(&project, &want) {
                return Ok(None);
            }
            let result = write_env_file(&project, &block.env, &want)?;
            Ok(Some(Refreshed { root: root.clone(), file: project.file.clone(), app: project.app, env: block.env, result }))
        })();
        match attempt {
            Ok(Some(r)) => updated.push(r),
            Ok(None) => {}
            Err(e) => skipped.push(Skipped { root: root.clone(), file, reason: reason_of(&e) }),
        }
    }
    (updated, skipped)
}
