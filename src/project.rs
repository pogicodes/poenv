// The `.poenv` marker file: which app a repo is, its default env, target file and format.
use crate::envfile::{ApplyResult, Dialect, Vars, apply_env, js_ws, parse_env_file};
use crate::errors::{PoenvError, Res, code};
use crate::util::{exists, is_file};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const MARKER: &str = ".poenv";

/// Where values live. File: in the env file's block. Run: only inside `poenv run`; the block lists
/// key names with empty values.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ValuesMode {
    File,
    Run,
}

impl ValuesMode {
    pub fn name(self) -> &'static str {
        match self {
            ValuesMode::File => "file",
            ValuesMode::Run => "run",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Project {
    pub root: String,
    pub app: String,
    /// Default env, used when nothing else says which env this folder is on.
    pub env: String,
    pub file: String,
    pub format: Dialect,
    pub values: ValuesMode,
}

impl Project {
    pub fn env_file_path(&self) -> PathBuf {
        Path::new(&self.root).join(&self.file)
    }
}

/// Looks for a `.poenv` file in `cwd` and its parents. Only files count: the vault itself is a
/// `~/.poenv` folder, which would otherwise look like a repo for everything under your home folder.
pub fn find_project(cwd: &Path) -> Res<Option<Project>> {
    let mut dir = cwd.to_path_buf();
    loop {
        if is_file(&dir.join(MARKER)) {
            return read_marker(&dir.to_string_lossy()).map(Some);
        }
        if !dir.pop() {
            return Ok(None);
        }
    }
}

pub fn require_project(cwd: &Path) -> Res<Project> {
    find_project(cwd)?.ok_or_else(|| {
        PoenvError::fix("This folder isn't set up for poenv (no .poenv file).", code::NOT_SET_UP, "poenv init")
    })
}

/// `^\s*([a-z]+)\s*=\s*"(.*)"\s*$`
fn marker_line(line: &str) -> Option<(String, String)> {
    let s = line.trim_start_matches(js_ws);
    let key_end = s.find(|c: char| !c.is_ascii_lowercase()).unwrap_or(s.len());
    if key_end == 0 {
        return None;
    }
    let rest = s[key_end..].trim_start_matches(js_ws).strip_prefix('=')?;
    let rest = rest.trim_start_matches(js_ws).strip_prefix('"')?;
    let rest = rest.trim_end_matches(js_ws).strip_suffix('"')?;
    Some((s[..key_end].to_string(), rest.to_string()))
}

pub fn read_marker(root: &str) -> Res<Project> {
    let path = Path::new(root).join(MARKER);
    let shown = path.display().to_string();
    let mut fields: HashMap<String, String> = HashMap::new();
    for line in fs::read_to_string(&path)?.split('\n') {
        if let Some((k, v)) = marker_line(line.strip_suffix('\r').unwrap_or(line)) {
            fields.insert(k, v);
        }
    }
    let Some(app) = fields.get("app").filter(|a| !a.is_empty()).cloned() else {
        return Err(PoenvError::fix(format!("{shown} has no app = \"…\" line."), code::NOT_SET_UP, "poenv init"));
    };
    let format_name = fields.get("format").cloned().unwrap_or_else(|| "dotenv".into());
    let Some(format) = Dialect::parse(&format_name) else {
        return Err(PoenvError::new(format!(
            "{shown}: format must be one of dotenv, next, python, properties (got \"{format_name}\")."
        )));
    };
    let values_name = fields.get("values").cloned().unwrap_or_else(|| "file".into());
    let values = match values_name.as_str() {
        "file" => ValuesMode::File,
        "run" => ValuesMode::Run,
        other => {
            return Err(PoenvError::new(format!("{shown}: values must be one of file, run (got \"{other}\").")));
        }
    };
    let get = |k: &str, d: &str| fields.get(k).cloned().unwrap_or_else(|| d.to_string());
    Ok(Project { root: root.to_string(), app, env: get("env", "local"), file: get("file", ".env"), format, values })
}

pub fn write_marker(project: &Project) -> Res<()> {
    let mut lines = vec![
        "# poenv: names only, never secrets.".to_string(),
        format!("app = \"{}\"", project.app),
        format!("env = \"{}\"", project.env),
        format!("file = \"{}\"", project.file),
        format!("format = \"{}\"", project.format.name()),
    ];
    if project.values == ValuesMode::Run {
        lines.push("values = \"run\"".into());
    }
    lines.push(String::new());
    fs::write(Path::new(&project.root).join(MARKER), lines.join("\n"))?;
    Ok(())
}

/// The env this folder is on right now: the block header if it's ours, else the default.
pub fn current_env(project: &Project) -> Res<String> {
    let path = project.env_file_path();
    if !exists(&path) {
        return Ok(project.env.clone());
    }
    let parsed = parse_env_file(&fs::read_to_string(&path)?, project.format)?;
    Ok(match parsed.block {
        Some(b) if b.app == project.app && !b.env.is_empty() => b.env,
        _ => project.env.clone(),
    })
}

/// True when the env file currently holds a poenv block for this app.
pub fn has_block(project: &Project) -> Res<bool> {
    let path = project.env_file_path();
    if !exists(&path) {
        return Ok(false);
    }
    Ok(parse_env_file(&fs::read_to_string(&path)?, project.format)?.block.is_some_and(|b| b.app == project.app))
}

/// What the env file's block should hold for `vars`: the values, or in run mode the key names only.
pub fn file_vars(project: &Project, vars: &Vars) -> Vars {
    match project.values {
        ValuesMode::File => vars.clone(),
        ValuesMode::Run => vars.keys().map(|k| (k.clone(), String::new())).collect(),
    }
}

/// Writes `env`'s vars into this repo's env file, honouring the repo's values mode.
pub fn write_env_file(project: &Project, env: &str, vars: &Vars) -> Res<ApplyResult> {
    apply_env(&project.env_file_path(), &project.app, env, &file_vars(project, vars), project.format)
}
