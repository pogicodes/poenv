// A lockfile so two poenv processes never write the store at the same time.
use crate::errors::{PoenvError, Res, code};
use crate::util::{now_ms, process_alive};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::Duration;

const TIMEOUT_MS: u64 = 5000;
pub const STALE_MS: u64 = 30_000;

pub struct Lock<'a> {
    path: &'a Path,
}

impl Drop for Lock<'_> {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.path);
    }
}

/// Blocks until the lock is acquired; it's released when the guard drops.
pub fn acquire(path: &Path) -> Res<Lock<'_>> {
    let started = now_ms();
    loop {
        match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(mut f) => {
                let _ = write!(f, "{{\"pid\":{},\"at\":{}}}", std::process::id(), now_ms());
                return Ok(Lock { path });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        if is_stale(path) {
            let _ = fs::remove_file(path);
            continue;
        }
        if now_ms() - started > TIMEOUT_MS {
            return Err(PoenvError::fix(
                "Another poenv command is writing the store. Try again in a moment.",
                code::ERROR,
                format!("if no poenv command is running, delete {}", path.display()),
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

pub fn with_lock<T>(path: &Path, f: impl FnOnce() -> Res<T>) -> Res<T> {
    let _lock = acquire(path)?;
    f()
}

pub fn is_stale(path: &Path) -> bool {
    let parsed = fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| Some((v.get("pid")?.as_u64()?, v.get("at")?.as_f64()? as u64)));
    match parsed {
        Some((pid, at)) => now_ms().saturating_sub(at) > STALE_MS || !process_alive(pid as u32),
        // Unreadable: possibly mid-write by its owner. Only reclaim once it's old.
        None => match fs::metadata(path).and_then(|m| m.modified()) {
            Ok(m) => m.elapsed().map(|d| d.as_millis() as u64 > STALE_MS).unwrap_or(false),
            Err(_) => true,
        },
    }
}
