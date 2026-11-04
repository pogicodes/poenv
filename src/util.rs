// Small helpers: time, paths, atomic writes, random bytes, process lookups.
use crate::errors::Res;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Like JavaScript's `new Date().toISOString()`: `2026-11-04T09:30:00.000Z`.
pub fn now_iso() -> String {
    iso(now_ms())
}

pub fn iso(ms: u64) -> String {
    let secs = ms / 1000;
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60,
        ms % 1000
    )
}

// Howard Hinnant's days → civil date.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).expect("no OS random source");
    buf
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Where poenv keeps its store. `POENV_HOME` overrides (tests, portable setups).
pub fn poenv_home() -> PathBuf {
    if let Some(h) = std::env::var_os("POENV_HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(h);
    }
    if cfg!(windows) {
        if let Some(a) = std::env::var_os("APPDATA").filter(|a| !a.is_empty()) {
            return PathBuf::from(a).join("poenv");
        }
        return home_dir().join("AppData").join("Roaming").join("poenv");
    }
    home_dir().join(".poenv")
}

pub fn home_dir() -> PathBuf {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

pub struct StorePaths {
    pub home: PathBuf,
    pub store: PathBuf,
    pub lock: PathBuf,
    /// The master key on this machine (owner-only).
    pub keys: PathBuf,
}

pub fn store_paths(home: &Path) -> StorePaths {
    StorePaths {
        home: home.to_path_buf(),
        store: home.join("store.enc"),
        lock: home.join("store.lock"),
        keys: home.join("keys.json"),
    }
}

/// Write to a temp file, fsync, then rename over `path`, so a crash never leaves half a file.
pub fn write_file_atomic(path: &Path, data: &[u8], mode: u32) -> Res<()> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!("{name}.tmp-{}-{}", std::process::id(), hex(&random_bytes(4))));
    let result = (|| -> std::io::Result<()> {
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        set_mode(&mut opts, mode);
        let mut f = opts.open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    Ok(result?)
}

#[cfg(unix)]
fn set_mode(opts: &mut OpenOptions, mode: u32) {
    use std::os::unix::fs::OpenOptionsExt;
    opts.mode(mode);
}
#[cfg(not(unix))]
fn set_mode(_: &mut OpenOptions, _: u32) {}

/// The permission bits of an existing file (0o600 where there are none).
pub fn file_mode(path: &Path) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(m) = fs::metadata(path) {
            return m.permissions().mode() & 0o777;
        }
    }
    let _ = path;
    0o600
}

pub fn mkdir_private(path: &Path) -> Res<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().recursive(true).mode(0o700).create(path)?;
    }
    #[cfg(not(unix))]
    fs::create_dir_all(path)?;
    Ok(())
}

pub fn exists(path: &Path) -> bool {
    fs::metadata(path).is_ok()
}

pub fn is_file(path: &Path) -> bool {
    fs::metadata(path).map(|m| m.is_file()).unwrap_or(false)
}

/// Joins like Node's `path.join` for display: `root/file`.
pub fn join_display(root: &str, file: &str) -> String {
    Path::new(root).join(file).to_string_lossy().into_owned()
}

/// Lexical normalisation like Node's `path.normalize` (no filesystem access).
pub fn normalize(p: &str) -> String {
    let sep = std::path::MAIN_SEPARATOR;
    let unified: String = if cfg!(windows) { p.replace('/', "\\") } else { p.to_string() };
    let absolute = unified.starts_with(sep);
    let mut parts: Vec<&str> = Vec::new();
    for part in unified.split(sep) {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|l| *l != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }
    let mut out = parts.join(&sep.to_string());
    if absolute {
        out.insert(0, sep);
    }
    if out.is_empty() {
        out.push('.');
    }
    if unified.ends_with(sep) && out != sep.to_string() && out != "." {
        out.push(sep);
    }
    out
}

/// The first `name` on PATH, like `which`.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into())
            .split(';')
            .map(|s| s.to_string())
            .collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else { return false };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    true
}

/// Runs a program quietly and returns its exit code (None when it couldn't start).
pub fn run_quiet(cmd: &str, args: &[&str]) -> Option<i32> {
    std::process::Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()
        .and_then(|s| s.code())
}

/// Whether process `pid` is running (or exists but isn't ours).
pub fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
        r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return false;
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(h, &mut code) != 0;
            CloseHandle(h);
            ok && code == STILL_ACTIVE as u32
        }
    }
}

/// JavaScript string length (UTF-16 code units), so limits match what people expect.
pub fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Locale-ish ordering like `localeCompare`: case-insensitive first.
pub fn locale_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| b.cmp(a))
}

pub fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}
