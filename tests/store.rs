// The encrypted store on disk: create, open, update, recover, rekey, lock, backup and restore.
mod common;
use common::*;
use poenv::errors::PoenvError;
use poenv::store::model::{App, Env, Variable};
use poenv::store::{export_store, import_store, init_store, lock, open_store, phrase_matches, recover_store, rekey_store};
use std::fs;

fn code_of<T>(r: Result<T, PoenvError>) -> Option<i32> {
    r.err().map(|e| e.code)
}

fn var(value: &str) -> Variable {
    Variable { value: value.into(), updated_at: "t".into(), history: vec![] }
}

/// A fresh vault (POENV_HOME pointed at it) holding one app, and its phrase.
fn vault_with_app(t: &mut T) -> String {
    t.empty_vault();
    let phrase = init_store().unwrap();
    update(|s| {
        let mut envs = indexmap::IndexMap::new();
        envs.insert("local".to_string(), Env::default());
        s.apps.insert("my-app".into(), App { envs });
        Ok(())
    });
    phrase
}

fn point_at(t: &mut T, home: &std::path::Path) {
    t.vault = home.to_path_buf();
    unsafe { std::env::set_var("POENV_HOME", home) };
}

#[test]
fn init_creates_an_encrypted_store_and_returns_the_phrase_once() {
    let mut t = T::new();
    t.empty_vault();
    let phrase = init_store().unwrap();
    assert_eq!(phrase.split(' ').count(), 24);
    let raw = fs::read(t.vault.join("store.enc")).unwrap();
    assert!(raw.starts_with(b"POENV\x01"));
    assert!(!String::from_utf8_lossy(&raw).contains("version"));
    assert_eq!(open_store().unwrap().data.version, 1);
}

#[cfg(unix)]
#[test]
fn store_key_file_and_home_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let mut t = T::new();
    t.empty_vault();
    init_store().unwrap();
    let mode = |p: &std::path::Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&t.vault.join("store.enc")), 0o600);
    assert_eq!(mode(&t.vault.join("keys.json")), 0o600);
    assert_eq!(mode(&t.vault), 0o700);
}

#[test]
fn init_twice_is_refused() {
    let _t = T::new();
    assert!(init_store().unwrap_err().problem.contains("already"));
}

#[test]
fn open_without_init_says_how_to_fix_it() {
    let mut t = T::new();
    t.empty_vault();
    let e = open_store().err().unwrap();
    assert_eq!(e.code, NOT_SET_UP);
    assert_eq!(e.fix.as_deref(), Some("poenv init"));
}

#[test]
fn without_its_key_file_open_points_to_recover() {
    let t = T::new();
    fs::remove_file(t.vault.join("keys.json")).unwrap();
    let e = open_store().err().unwrap();
    assert_eq!(e.code, WAITING_ON_USER);
    assert!(e.fix.unwrap().contains("poenv recover"));
}

#[test]
fn a_key_file_poenv_cant_read_counts_as_missing() {
    let t = T::new();
    fs::write(t.vault.join("keys.json"), r#"{"version":1,"kdf":{"name":"scrypt"},"wrapped":"x"}"#).unwrap();
    assert!(open_store().err().unwrap().fix.unwrap().contains("poenv recover"));
}

#[test]
fn changes_persist_across_opens() {
    let _t = T::new();
    update(|s| {
        s.meta.recovery_confirmed = true;
        Ok(())
    });
    assert!(open_store().unwrap().data.meta.recovery_confirmed);
}

#[test]
fn a_failing_update_leaves_the_file_untouched_and_no_temp_files() {
    let t = T::new();
    let before = fs::read(t.vault.join("store.enc")).unwrap();
    let r: Result<(), _> = open_store().unwrap().update(|_| Err(PoenvError::new("boom")));
    assert_eq!(r.unwrap_err().problem, "boom");
    assert_eq!(fs::read(t.vault.join("store.enc")).unwrap(), before);
    let names: Vec<String> = fs::read_dir(&t.vault).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert!(!names.iter().any(|n| n.contains(".tmp")));
    assert!(!t.vault.join("store.lock").exists());
}

#[test]
fn update_rereads_the_latest_file_so_two_handles_dont_lose_writes() {
    let _t = T::new();
    let mut a = open_store().unwrap();
    let mut b = open_store().unwrap();
    a.update(|s| {
        s.shared.insert("A".into(), var("1"));
        Ok(())
    })
    .unwrap();
    b.update(|s| {
        s.shared.insert("B".into(), var("2"));
        Ok(())
    })
    .unwrap();
    let mut keys: Vec<String> = open_store().unwrap().data.shared.keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["A", "B"]);
}

#[test]
fn the_phrase_rebuilds_a_lost_key_file() {
    let mut t = T::new();
    t.empty_vault();
    let phrase = init_store().unwrap();
    fs::remove_file(t.vault.join("keys.json")).unwrap();
    recover_store(&phrase).unwrap();
    assert_eq!(open_store().unwrap().data.version, 1);
}

#[test]
fn a_phrase_from_another_vault_is_refused_and_writes_nothing() {
    let mut t = T::new();
    let mine = temp();
    point_at(&mut t, &mine);
    init_store().unwrap();
    fs::remove_file(mine.join("keys.json")).unwrap();
    point_at(&mut t, &temp());
    let other = init_store().unwrap();
    point_at(&mut t, &mine);
    assert!(recover_store(&other).unwrap_err().problem.contains("different vault"));
    assert!(!mine.join("keys.json").exists());
}

#[test]
fn a_valid_phrase_from_another_vault_is_told_apart_from_a_typo() {
    let mut t = T::new();
    let mine = t.vault.clone();
    point_at(&mut t, &temp());
    let other = init_store().unwrap();
    point_at(&mut t, &mine);
    assert!(!phrase_matches(&other).unwrap());
    assert!(phrase_matches("not a phrase").unwrap_err().problem.contains("24 words"));
}

#[test]
fn rekey_makes_a_new_phrase_and_keeps_the_data() {
    let mut t = T::new();
    t.empty_vault();
    let old = init_store().unwrap();
    update(|s| {
        s.shared.insert("K".into(), var("v"));
        Ok(())
    });
    let new = rekey_store().unwrap();
    assert_ne!(new, old);
    assert!(phrase_matches(&new).unwrap());
    assert!(!phrase_matches(&old).unwrap());
    let data = open_store().unwrap().data;
    assert_eq!(data.shared["K"].value, "v");
    assert!(!data.meta.recovery_confirmed);
}

#[test]
fn a_held_lock_blocks_a_second_writer_until_timeout() {
    let _t = T::new();
    let path = temp().join("store.lock");
    let held = lock::acquire(&path).unwrap();
    let started = std::time::Instant::now();
    assert_eq!(code_of(lock::acquire(&path)), Some(ERROR));
    assert!(started.elapsed().as_millis() >= 4000);
    drop(held);
    drop(lock::acquire(&path).unwrap());
}

#[test]
fn a_lock_left_by_a_dead_process_is_reclaimed() {
    let _t = T::new();
    let path = temp().join("store.lock");
    fs::write(&path, format!("{{\"pid\":999999999,\"at\":{}}}", poenv::util::now_ms())).unwrap();
    drop(lock::acquire(&path).unwrap());
}

#[test]
fn a_very_old_lock_is_reclaimed_even_if_the_pid_looks_alive() {
    let _t = T::new();
    let path = temp().join("store.lock");
    let at = poenv::util::now_ms() - 60_000;
    fs::write(&path, format!("{{\"pid\":{},\"at\":{at}}}", std::process::id())).unwrap();
    drop(lock::acquire(&path).unwrap());
}

#[test]
fn backup_writes_the_encrypted_store_and_wont_overwrite_without_force() {
    let mut t = T::new();
    vault_with_app(&mut t);
    let dest = temp().join("backup.enc");
    assert_eq!(export_store(&dest, false).unwrap(), ["my-app"]);
    assert_eq!(fs::read(&dest).unwrap(), fs::read(t.vault.join("store.enc")).unwrap());
    assert!(export_store(&dest, false).unwrap_err().problem.contains("already exists"));
    export_store(&dest, true).unwrap();
}

#[test]
fn on_a_new_machine_the_phrase_opens_the_backup() {
    let mut t = T::new();
    let phrase = vault_with_app(&mut t);
    let dest = temp().join("backup.enc");
    export_store(&dest, false).unwrap();
    point_at(&mut t, &temp());
    let (previous, apps) = import_store(&dest, || Ok(phrase)).unwrap();
    assert_eq!((previous, apps), (None, vec!["my-app".to_string()]));
    assert!(open_store().unwrap().data.apps.contains_key("my-app"));
}

#[test]
fn a_wrong_phrase_is_refused_and_nothing_is_written() {
    let mut t = T::new();
    vault_with_app(&mut t);
    let dest = temp().join("backup.enc");
    export_store(&dest, false).unwrap();
    let other = vault_with_app(&mut t);
    let fresh = temp();
    point_at(&mut t, &fresh);
    assert!(import_store(&dest, || Ok(other)).unwrap_err().problem.contains("does not match"));
    assert!(!fresh.join("store.enc").exists());
    assert!(!fresh.join("keys.json").exists());
}

#[test]
fn restoring_into_the_same_vault_keeps_the_old_one_as_bak() {
    let mut t = T::new();
    vault_with_app(&mut t);
    let dest = temp().join("backup.enc");
    export_store(&dest, false).unwrap();
    update(|s| {
        s.apps.shift_remove("my-app");
        Ok(())
    });
    let (previous, apps) = import_store(&dest, || panic!("phrase should not be needed")).unwrap();
    assert_eq!(apps, ["my-app"]);
    assert!(previous.unwrap().contains("store.enc.bak-"));
    let baks = fs::read_dir(&t.vault).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with("store.enc.bak-")).count();
    assert_eq!(baks, 1);
    assert!(open_store().unwrap().data.apps.contains_key("my-app"));
}

#[test]
fn a_backup_from_a_different_vault_is_refused_while_a_vault_exists() {
    let mut t = T::new();
    vault_with_app(&mut t);
    let a = t.vault.clone();
    let b_phrase = vault_with_app(&mut t);
    let dest = temp().join("backup.enc");
    export_store(&dest, false).unwrap();
    point_at(&mut t, &a);
    assert_eq!(code_of(import_store(&dest, || Ok(b_phrase))), Some(REFUSED));
}

#[test]
fn a_phrase_always_gives_the_same_key() {
    // Changing the derivation would break every saved phrase.
    let phrase = [vec!["abandon"; 23], vec!["art"]].concat().join(" ");
    let key = poenv::store::recovery::key_from_phrase(&phrase).unwrap();
    assert_eq!(poenv::util::hex(&key), "58fdad6f40878e3d7fc57f247497f4403a6ed8913b03389230f9ea4e65969c91");
    assert!(poenv::store::recovery::key_from_phrase(&vec!["abandon"; 24].join(" ")).is_err());
    assert!(poenv::store::recovery::key_from_phrase("too short").is_err());
}
