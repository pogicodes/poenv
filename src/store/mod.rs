// Opening, creating and recovering the encrypted store on disk.
//
// The master key comes from the 24-word recovery phrase and is kept in keys.json next to the store,
// so this machine opens the vault without asking. The store file on its own (and every backup) needs
// the phrase.
pub mod crypto;
pub mod lock;
pub mod model;
pub mod recovery;

use crate::errors::{PoenvError, Res, code};
use crate::util::{StorePaths, exists, iso, mkdir_private, now_ms, poenv_home, store_paths, write_file_atomic};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use lock::with_lock;
use model::{Store, empty_store};
use std::fs;
use std::path::Path;

/// The fix when the store is here but its key isn't (keys.json lost or unreadable).
pub const RECOVER_FIX: &str = "poenv recover   (type your 24-word recovery phrase once)";

pub struct StoreHandle {
    paths: StorePaths,
    key: Vec<u8>,
    /// Snapshot as of open (or the last update through this handle).
    pub data: Store,
}

impl StoreHandle {
    /// Re-reads the file under the lock, applies `f`, and writes atomically.
    pub fn update<T>(&mut self, f: impl FnOnce(&mut Store) -> Res<T>) -> Res<T> {
        let _lock = lock::acquire(&self.paths.lock)?;
        let mut fresh = read_store(&self.paths.store, &self.key)?;
        let result = f(&mut fresh)?;
        write_store(&self.paths.store, &self.key, &fresh)?;
        self.data = fresh;
        Ok(result)
    }
}

pub fn paths() -> StorePaths {
    store_paths(&poenv_home())
}

/// Creates the store. The phrase is returned once and never stored.
pub fn init_store() -> Res<String> {
    let p = paths();
    mkdir_private(&p.home)?;
    with_lock(&p.lock, || {
        if exists(&p.store) {
            return Err(PoenvError::fix(
                format!("A poenv store already exists at {}.", p.store.display()),
                code::ERROR,
                "poenv status",
            ));
        }
        let (phrase, key) = recovery::generate_recovery();
        write_key(&p.keys, &key)?;
        write_store(&p.store, &key, &empty_store())?;
        Ok(phrase)
    })
}

pub fn open_store() -> Res<StoreHandle> {
    let p = paths();
    if !exists(&p.store) {
        return Err(PoenvError::fix("poenv isn't set up on this machine yet.", code::NOT_SET_UP, "poenv init"));
    }
    let Some(key) = read_key(&p.keys) else {
        return Err(PoenvError::fix("This vault's key isn't on this machine.", code::WAITING_ON_USER, RECOVER_FIX));
    };
    let data = read_store(&p.store, &key)?;
    Ok(StoreHandle { paths: p, key, data })
}

const DIFFERENT_VAULT: &str =
    "That is a valid recovery phrase, but for a different vault (did poenv init run more than once?).";

/// Rebuilds this machine's key from the phrase (keys.json lost or unreadable).
pub fn recover_store(phrase: &str) -> Res<()> {
    let p = paths();
    if !exists(&p.store) {
        return Err(PoenvError::fix(
            format!("No store at {} to recover.", p.store.display()),
            code::NOT_SET_UP,
            format!("copy your store.enc to {}, or poenv restore <backup>", p.home.display()),
        ));
    }
    let key = recovery::key_from_phrase(phrase)?;
    if !opens(&key, &fs::read(&p.store)?) {
        return Err(PoenvError::new(DIFFERENT_VAULT));
    }
    write_key(&p.keys, &key)
}

/// Whether `phrase` opens this vault. Errors when it isn't a valid phrase at all.
pub fn phrase_matches(phrase: &str) -> Res<bool> {
    let key = recovery::key_from_phrase(phrase)?;
    Ok(opens(&key, &fs::read(paths().store)?))
}

/// Re-encrypts the vault under a brand-new recovery phrase. Backups made before this still need the old one.
pub fn rekey_store() -> Res<String> {
    let mut data = open_store()?.data;
    let p = paths();
    with_lock(&p.lock, || {
        let (phrase, key) = recovery::generate_recovery();
        data.meta.recovery_confirmed = false;
        let next = p.keys.with_file_name("keys.json.new");
        write_key(&next, &key)?;
        write_store(&p.store, &key, &data)?;
        fs::rename(&next, &p.keys)?;
        Ok(phrase)
    })
}

fn opens(key: &[u8], bytes: &[u8]) -> bool {
    parse_store(bytes, key).is_ok()
}

/// Writes an encrypted backup. It's the store file itself, so the recovery phrase opens it anywhere.
pub fn export_store(dest: &Path, force: bool) -> Res<Vec<String>> {
    let data = open_store()?.data;
    if exists(dest) && !force {
        return Err(PoenvError::fix(
            format!("{} already exists.", dest.display()),
            code::ERROR,
            format!("poenv backup {} --force", dest.display()),
        ));
    }
    write_file_atomic(dest, &fs::read(paths().store)?, 0o600)?;
    Ok(sorted_apps(&data))
}

/// Restores a backup as this machine's vault, asking for the phrase when this machine's key can't open it.
/// The old store is kept as store.enc.bak-<time>. A backup from a different vault is refused while one exists.
pub fn import_store(src: &Path, ask_phrase: impl FnOnce() -> Res<String>) -> Res<(Option<String>, Vec<String>)> {
    let p = paths();
    if !exists(src) {
        return Err(PoenvError::new(format!("No file at {}.", src.display())));
    }
    let bytes = fs::read(src)?;
    let mut key = read_key(&p.keys);
    let mut data = key.as_ref().and_then(|k| parse_store(&bytes, k).ok());
    let new_key = data.is_none();
    if data.is_none() {
        if exists(&p.store) {
            if key.is_none() {
                return Err(PoenvError::fix(
                    "This vault's key isn't on this machine.",
                    code::WAITING_ON_USER,
                    RECOVER_FIX,
                ));
            }
            return Err(PoenvError::fix(
                "This backup belongs to a different vault (made with a different recovery phrase).",
                code::REFUSED,
                format!("move {} aside first if you mean to switch vaults", p.store.display()),
            ));
        }
        let k = recovery::key_from_phrase(&ask_phrase()?)?;
        if !opens(&k, &bytes) {
            return Err(PoenvError::new("That recovery phrase does not match this backup."));
        }
        data = Some(parse_store(&bytes, &k)?);
        key = Some(k);
    }
    mkdir_private(&p.home)?;
    let previous = with_lock(&p.lock, || {
        let mut kept = None;
        if exists(&p.store) {
            let stamp = iso(now_ms()).replace([':', '.'], "-");
            let path = format!("{}.bak-{stamp}", p.store.display());
            fs::copy(&p.store, &path)?;
            kept = Some(path);
        }
        write_file_atomic(&p.store, &bytes, 0o600)?;
        Ok(kept)
    })?;
    if new_key {
        write_key(&p.keys, key.as_ref().unwrap())?;
    }
    Ok((previous, sorted_apps(data.as_ref().unwrap())))
}

fn sorted_apps(data: &Store) -> Vec<String> {
    let mut apps: Vec<String> = data.apps.keys().cloned().collect();
    apps.sort();
    apps
}

fn read_store(path: &Path, key: &[u8]) -> Res<Store> {
    parse_store(&fs::read(path)?, key)
}

fn parse_store(bytes: &[u8], key: &[u8]) -> Res<Store> {
    let plain = crypto::decrypt(key, bytes)?;
    let value: serde_json::Value = serde_json::from_slice(&plain)?;
    let version = value.get("version").cloned().unwrap_or(serde_json::Value::Null);
    if version != serde_json::json!(1) {
        return Err(PoenvError::fix(format!("Unsupported store version {version}."), code::ERROR, "upgrade poenv"));
    }
    Ok(serde_json::from_value(value)?)
}

fn write_store(path: &Path, key: &[u8], data: &Store) -> Res<()> {
    write_file_atomic(path, &crypto::encrypt(key, serde_json::to_string(data)?.as_bytes()), 0o600)
}

// keys.json: the vault's master key on this machine, owner-only.
fn write_key(path: &Path, key: &[u8]) -> Res<()> {
    let text = format!("{{\n  \"version\": 2,\n  \"key\": \"{}\"\n}}\n", B64.encode(key));
    write_file_atomic(path, text.as_bytes(), 0o600)
}

/// The master key, or None when keys.json is missing or isn't one poenv can read.
fn read_key(path: &Path) -> Option<Vec<u8>> {
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    if v.get("version")? != 2 {
        return None;
    }
    B64.decode(v.get("key")?.as_str()?).ok()
}
