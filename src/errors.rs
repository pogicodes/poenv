// Stable exit codes — agents branch on these (SPEC §7).
use std::fmt;
use std::io;

pub mod code {
    pub const OK: i32 = 0;
    pub const ERROR: i32 = 1;
    pub const NOT_SET_UP: i32 = 2;
    pub const UNKNOWN: i32 = 3;
    pub const REFUSED: i32 = 4;
    pub const WAITING_ON_USER: i32 = 5;
}

/// An expected failure (`unexpected == false`), whose `fix` is the exact command that resolves it,
/// or an unexpected one (an I/O error and the like).
#[derive(Debug, Clone)]
pub struct PoenvError {
    pub problem: String,
    pub code: i32,
    pub fix: Option<String>,
    pub unexpected: bool,
    pub io_kind: Option<io::ErrorKind>,
}

pub type Res<T> = Result<T, PoenvError>;

impl PoenvError {
    pub fn new(problem: impl Into<String>) -> Self {
        Self::with(problem, code::ERROR, None::<String>)
    }
    pub fn code(problem: impl Into<String>, code: i32) -> Self {
        Self::with(problem, code, None::<String>)
    }
    pub fn with(problem: impl Into<String>, code: i32, fix: Option<impl Into<String>>) -> Self {
        PoenvError { problem: problem.into(), code, fix: fix.map(Into::into), unexpected: false, io_kind: None }
    }
    pub fn fix(problem: impl Into<String>, code: i32, fix: impl Into<String>) -> Self {
        Self::with(problem, code, Some(fix))
    }
    pub fn unexpected(message: impl Into<String>) -> Self {
        PoenvError { problem: message.into(), code: code::ERROR, fix: None, unexpected: true, io_kind: None }
    }
}

impl fmt::Display for PoenvError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.problem)
    }
}

impl From<io::Error> for PoenvError {
    fn from(err: io::Error) -> Self {
        let mut e = PoenvError::unexpected(err.to_string());
        e.io_kind = Some(err.kind());
        e
    }
}

impl From<serde_json::Error> for PoenvError {
    fn from(err: serde_json::Error) -> Self {
        PoenvError::unexpected(err.to_string())
    }
}
