// Shared test harness: temp folders, a vault per test, and the real binary run the way an agent would.
#![allow(dead_code)]
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};

pub const OK: i32 = 0;
pub const ERROR: i32 = 1;
pub const NOT_SET_UP: i32 = 2;
pub const UNKNOWN: i32 = 3;
pub const REFUSED: i32 = 4;
pub const WAITING_ON_USER: i32 = 5;

// POENV_HOME is process-wide (the library reads it), so tests in one file take turns.
static LOCK: Mutex<()> = Mutex::new(());

pub fn temp() -> PathBuf {
    let bytes = poenv::util::random_bytes(6);
    let dir = std::env::temp_dir().join(format!("poenv-test-{}", poenv::util::hex(&bytes)));
    poenv::util::mkdir_private(&dir).unwrap(); // owner-only, like mkdtemp
    // The real path, as poenv reports it (no `\\?\` prefix on Windows).
    PathBuf::from(poenv::repos::realpath(&dir.to_string_lossy()).unwrap())
}

pub fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_poenv"))
}

/// The helper program from examples/child.rs (cargo test builds examples).
pub fn child() -> String {
    let dir = bin().parent().unwrap().join("examples");
    dir.join(format!("child{}", std::env::consts::EXE_SUFFIX)).to_string_lossy().into_owned()
}

pub struct Out {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Out {
    /// The last line of stdout as JSON.
    pub fn json(&self) -> Value {
        let last = self.stdout.trim().lines().last().unwrap_or("");
        serde_json::from_str(last).unwrap_or_else(|e| panic!("not JSON ({e}): {}\nstderr: {}", self.stdout, self.stderr))
    }
}

pub struct T {
    _guard: MutexGuard<'static, ()>,
    pub vault: PathBuf,
}

impl T {
    /// A fresh vault, like `beforeEach(newVault)`.
    pub fn new() -> T {
        let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut t = T { _guard: guard, vault: PathBuf::new() };
        t.new_vault();
        t
    }

    pub fn new_vault(&mut self) {
        self.empty_vault();
        poenv::store::init_store().unwrap();
    }

    /// A home with no vault in it (a machine poenv hasn't been set up on).
    pub fn empty_vault(&mut self) {
        self.vault = temp();
        unsafe { std::env::set_var("POENV_HOME", &self.vault) };
    }

    pub fn poenv(&self, cwd: &Path, args: &[&str]) -> Out {
        self.run(cwd, args, None)
    }

    pub fn poenv_in(&self, cwd: &Path, args: &[&str], input: &str) -> Out {
        self.run(cwd, args, Some(input))
    }

    fn run(&self, cwd: &Path, args: &[&str], input: Option<&str>) -> Out {
        let mut cmd = Command::new(bin());
        cmd.args(args).current_dir(cwd).env("POENV_HOME", &self.vault).env("NO_COLOR", "1");
        for k in ["CLAUDECODE", "CI"] {
            cmd.env_remove(k);
        }
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd.stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() });
        let mut c = cmd.spawn().unwrap();
        if let Some(text) = input {
            c.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
        }
        let out = c.wait_with_output().unwrap();
        Out {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }
}

pub fn git(cwd: &Path, args: &[&str]) {
    Command::new("git").args(args).current_dir(cwd).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap();
}

/// A git repo with Next.js in package.json and, unless empty, a .env.local.
pub fn next_repo(name: &str, env_local: &str) -> PathBuf {
    let root = temp().join(name);
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    fs::write(root.join("package.json"), r#"{"dependencies":{"next":"15.0.0"}}"#).unwrap();
    if !env_local.is_empty() {
        fs::write(root.join(".env.local"), env_local).unwrap();
    }
    root
}

pub const DEFAULT_ENV: &str = "DB=postgres://localhost/dev\nOPENAI_API_KEY=sk-123\n";

pub fn repo() -> PathBuf {
    next_repo("my-app", DEFAULT_ENV)
}

pub fn read(root: &Path, file: &str) -> String {
    fs::read_to_string(root.join(file)).unwrap()
}

/// Like Jest's toMatchObject: every field in `expected` is in `actual` (arrays match element by element).
pub fn matches(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Object(a), Value::Object(e)) => e.iter().all(|(k, v)| a.get(k).is_some_and(|av| matches(av, v))),
        (Value::Array(a), Value::Array(e)) => a.len() == e.len() && a.iter().zip(e).all(|(x, y)| matches(x, y)),
        _ => actual == expected,
    }
}

#[macro_export]
macro_rules! assert_match {
    ($actual:expr, $($expected:tt)+) => {{
        let actual = $actual;
        let expected = serde_json::json!($($expected)+);
        assert!($crate::common::matches(&actual, &expected), "expected {actual} to match {expected}");
    }};
}

pub fn check<'a>(out: &'a Value, id: &str) -> Option<&'a Value> {
    out["checks"].as_array().unwrap().iter().find(|c| c["id"] == id)
}

pub fn update(f: impl FnOnce(&mut poenv::store::model::Store) -> poenv::errors::Res<()>) {
    poenv::store::open_store().unwrap().update(f).unwrap();
}
