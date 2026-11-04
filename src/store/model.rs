// The decrypted store and pure operations on it. No I/O here.
use crate::errors::{PoenvError, Res, code};
use crate::util::now_iso;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

pub const HISTORY_LIMIT: usize = 10;
pub const DEFAULT_ENVS: [&str; 3] = ["local", "staging", "prod"];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Hist {
    pub value: String,
    pub at: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Variable {
    pub value: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    /// Previous values, newest first, at most HISTORY_LIMIT.
    pub history: Vec<Hist>,
}

/// A live reference to a key in the shared pool.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Ref {
    #[serde(rename = "ref")]
    pub target: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum Var {
    Ref(Ref),
    Lit(Variable),
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Env {
    pub protected: bool,
    pub vars: IndexMap<String, Var>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct App {
    pub envs: IndexMap<String, Env>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Meta {
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "recoveryConfirmed")]
    pub recovery_confirmed: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Store {
    pub version: u32,
    pub meta: Meta,
    pub shared: IndexMap<String, Variable>,
    pub apps: IndexMap<String, App>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Shared,
    App { app: String, env: String },
}

impl Target {
    pub fn app(app: &str, env: &str) -> Target {
        Target::App { app: app.into(), env: env.into() }
    }
    pub fn describe(&self) -> String {
        match self {
            Target::Shared => "the shared pool".into(),
            Target::App { app, env } => format!("{app}/{env}"),
        }
    }
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Target::Shared => serde_json::json!({ "shared": true }),
            Target::App { app, env } => serde_json::json!({ "app": app, "env": env }),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Use {
    pub app: String,
    pub env: String,
    pub key: String,
}

impl Use {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({ "app": self.app, "env": self.env, "key": self.key })
    }
}

/// `^[A-Za-z_][A-Za-z0-9_]*$`
pub fn valid_key(key: &str) -> bool {
    let mut cs = key.chars();
    matches!(cs.next(), Some(c) if c.is_ascii_alphabetic() || c == '_') && cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `^[A-Za-z0-9][A-Za-z0-9._-]*$`
pub fn valid_name(name: &str) -> bool {
    let mut cs = name.chars();
    matches!(cs.next(), Some(c) if c.is_ascii_alphanumeric())
        && cs.all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

pub fn empty_store() -> Store {
    Store {
        version: 1,
        meta: Meta { created_at: now_iso(), recovery_confirmed: false },
        shared: IndexMap::new(),
        apps: IndexMap::new(),
    }
}

pub fn create_app(store: &mut Store, name: &str) -> Res<()> {
    if !valid_name(name) {
        return Err(PoenvError::new(format!(
            "\"{name}\" is not a valid app name. Use letters, digits, \".\", \"_\" or \"-\"."
        )));
    }
    if store.apps.contains_key(name) {
        return Err(PoenvError::new(format!("App \"{name}\" already exists.")));
    }
    let mut envs = IndexMap::new();
    for env in DEFAULT_ENVS {
        envs.insert(env.to_string(), Env { protected: env == "prod", vars: IndexMap::new() });
    }
    store.apps.insert(name.to_string(), App { envs });
    Ok(())
}

pub fn create_env(store: &mut Store, app: &str, env: &str) -> Res<()> {
    let Some(a) = store.apps.get_mut(app) else {
        return Err(PoenvError::code(format!("No app \"{app}\"."), code::UNKNOWN));
    };
    if !valid_name(env) {
        return Err(PoenvError::new(format!(
            "\"{env}\" is not a valid environment name. Use letters, digits, \".\", \"_\" or \"-\"."
        )));
    }
    if a.envs.contains_key(env) {
        return Err(PoenvError::new(format!("{app} already has an environment \"{env}\".")));
    }
    a.envs.insert(env.to_string(), Env::default());
    Ok(())
}

/// A new env with `from`'s keys: values copied once (history not carried over), references kept as references.
pub fn copy_env(store: &mut Store, app: &str, from: &str, to: &str) -> Res<()> {
    let source = get_env(store, app, from)?.vars.clone();
    create_env(store, app, to)?;
    let at = now_iso();
    let vars = &mut store.apps[app].envs[to].vars;
    for (key, v) in source {
        let copy = match v {
            Var::Ref(r) => Var::Ref(Ref { target: r.target, updated_at: at.clone() }),
            Var::Lit(l) => Var::Lit(Variable { value: l.value, updated_at: at.clone(), history: vec![] }),
        };
        vars.insert(key, copy);
    }
    Ok(())
}

/// Renames in place, keeping the env's keys, history, protection and position.
pub fn rename_env(store: &mut Store, app: &str, from: &str, to: &str) -> Res<()> {
    get_env(store, app, from)?;
    create_env(store, app, to)?; // validates the new name and that it's free
    let a = &mut store.apps[app];
    a.envs.shift_remove(to);
    let idx = a.envs.get_index_of(from).unwrap();
    let (_, env) = a.envs.shift_remove_index(idx).unwrap();
    a.envs.shift_insert(idx, to.to_string(), env);
    Ok(())
}

pub fn remove_env(store: &mut Store, app: &str, env: &str) -> Res<()> {
    get_env(store, app, env)?;
    let a = &mut store.apps[app];
    if a.envs.len() == 1 {
        return Err(PoenvError::new(format!("{app} needs at least one environment.")));
    }
    a.envs.shift_remove(env);
    Ok(())
}

pub fn set_protected(store: &mut Store, app: &str, env: &str, value: bool) -> Res<()> {
    get_env_mut(store, app, env)?.protected = value;
    Ok(())
}

pub fn get_env<'a>(store: &'a Store, app: &str, env: &str) -> Res<&'a Env> {
    let Some(a) = store.apps.get(app) else {
        return Err(PoenvError::fix(format!("No app \"{app}\"."), code::UNKNOWN, "poenv init (inside the app's repo)"));
    };
    a.envs.get(env).ok_or_else(|| {
        let known: Vec<&str> = a.envs.keys().map(|s| s.as_str()).collect();
        PoenvError::code(
            format!("No environment \"{env}\" in {app}. Environments: {}.", known.join(", ")),
            code::UNKNOWN,
        )
    })
}

pub fn get_env_mut<'a>(store: &'a mut Store, app: &str, env: &str) -> Res<&'a mut Env> {
    get_env(store, app, env)?;
    Ok(&mut store.apps[app].envs[env])
}

/// What a target holds: the shared pool's literals, or an env's variables.
pub enum Vars<'a> {
    Shared(&'a mut IndexMap<String, Variable>),
    Env(&'a mut IndexMap<String, Var>),
}

fn vars_of<'a>(store: &'a mut Store, target: &Target) -> Res<Vars<'a>> {
    match target {
        Target::Shared => Ok(Vars::Shared(&mut store.shared)),
        Target::App { app, env } => Ok(Vars::Env(&mut get_env_mut(store, app, env)?.vars)),
    }
}

fn bump(existing: &mut Variable, value: &str, at: &str) {
    if existing.value == value {
        return;
    }
    let mut history = vec![Hist { value: existing.value.clone(), at: existing.updated_at.clone() }];
    history.append(&mut existing.history);
    history.truncate(HISTORY_LIMIT);
    existing.history = history;
    existing.value = value.to_string();
    existing.updated_at = at.to_string();
}

pub fn set_var(store: &mut Store, target: &Target, key: &str, value: &str) -> Res<()> {
    assert_key(key)?;
    let at = now_iso();
    let fresh = || Variable { value: value.to_string(), updated_at: at.clone(), history: vec![] };
    match vars_of(store, target)? {
        Vars::Shared(m) => match m.get_mut(key) {
            Some(existing) => bump(existing, value, &at),
            None => {
                m.insert(key.to_string(), fresh());
            }
        },
        Vars::Env(m) => match m.get_mut(key) {
            Some(Var::Lit(existing)) => bump(existing, value, &at),
            _ => {
                m.insert(key.to_string(), Var::Lit(fresh()));
            }
        },
    }
    Ok(())
}

pub fn set_ref(store: &mut Store, app: &str, env: &str, key: &str, shared_key: &str) -> Res<()> {
    assert_key(key)?;
    if !store.shared.contains_key(shared_key) {
        return Err(PoenvError::fix(
            format!("No shared key \"{shared_key}\"."),
            code::UNKNOWN,
            format!("poenv set {shared_key} --shared"),
        ));
    }
    let vars = &mut get_env_mut(store, app, env)?.vars;
    vars.insert(key.to_string(), Var::Ref(Ref { target: shared_key.to_string(), updated_at: now_iso() }));
    Ok(())
}

/// Moves an app's key into the shared pool (keeping its history) and makes the app reference it.
/// Returns whether the shared key was created, rather than an equal one reused.
pub fn move_to_shared(store: &mut Store, app: &str, env: &str, key: &str) -> Res<bool> {
    let target = Target::app(app, env);
    let v = match get_env(store, app, env)?.vars.get(key) {
        None => {
            return Err(PoenvError::code(format!("Key \"{key}\" not found in {}.", target.describe()), code::UNKNOWN));
        }
        Some(Var::Ref(r)) => return Err(PoenvError::new(format!("{key} is already shared ({}).", r.target))),
        Some(Var::Lit(v)) => v.clone(),
    };
    let existing = store.shared.get(key).cloned();
    if let Some(e) = &existing {
        if e.value != v.value {
            return Err(PoenvError::new(format!("A different shared {key} already exists. Rename one of them first.")));
        }
    } else {
        store.shared.insert(key.to_string(), v);
    }
    set_ref(store, app, env, key, key)?;
    Ok(existing.is_none())
}

/// Every app key that references shared `shared_key` (renamed references included).
pub fn shared_uses(store: &Store, shared_key: &str) -> Vec<Use> {
    let mut out = vec![];
    for (app, a) in &store.apps {
        for (env, e) in &a.envs {
            for (key, v) in &e.vars {
                if let Var::Ref(r) = v {
                    if r.target == shared_key {
                        out.push(Use { app: app.clone(), env: env.clone(), key: key.clone() });
                    }
                }
            }
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq)]
pub enum RemoveMode {
    KeepCopies,
    Everywhere,
}

/// Deletes a shared key without leaving broken references.
pub fn remove_shared(store: &mut Store, shared_key: &str, mode: RemoveMode) -> Res<()> {
    let Some(shared) = store.shared.get(shared_key).cloned() else {
        return Err(PoenvError::code(format!("Key \"{shared_key}\" not found in the shared pool."), code::UNKNOWN));
    };
    for u in shared_uses(store, shared_key) {
        let vars = &mut store.apps[&u.app].envs[&u.env].vars;
        match mode {
            RemoveMode::Everywhere => {
                vars.shift_remove(&u.key);
            }
            RemoveMode::KeepCopies => {
                vars.insert(u.key.clone(), Var::Lit(shared.clone()));
            }
        }
    }
    store.shared.shift_remove(shared_key);
    Ok(())
}

pub fn remove_var(store: &mut Store, target: &Target, key: &str) -> Res<()> {
    let removed = match vars_of(store, target)? {
        Vars::Shared(m) => m.shift_remove(key).is_some(),
        Vars::Env(m) => m.shift_remove(key).is_some(),
    };
    if !removed {
        return Err(PoenvError::code(format!("Key \"{key}\" not found in {}.", target.describe()), code::UNKNOWN));
    }
    Ok(())
}

/// The stored variable at `key` (shared pool: always a literal).
pub fn var_at(store: &Store, target: &Target, key: &str) -> Res<Option<Var>> {
    Ok(match target {
        Target::Shared => store.shared.get(key).cloned().map(Var::Lit),
        Target::App { app, env } => get_env(store, app, env)?.vars.get(key).cloned(),
    })
}

/// Restore `history[index]`; the current value is kept in history like any other change.
pub fn rollback_var(store: &mut Store, target: &Target, key: &str, index: usize) -> Res<()> {
    let v = match var_at(store, target, key)? {
        None => {
            return Err(PoenvError::code(format!("Key \"{key}\" not found in {}.", target.describe()), code::UNKNOWN));
        }
        Some(Var::Ref(r)) => {
            return Err(PoenvError::fix(
                format!("{key} in {} is a reference to shared {}; roll that back instead.", target.describe(), r.target),
                code::UNKNOWN,
                format!("poenv rollback {} --shared", r.target),
            ));
        }
        Some(Var::Lit(v)) => v,
    };
    let Some(version) = v.history.get(index) else {
        return Err(PoenvError::fix(
            format!("{key} has no version {index} (it has {}).", v.history.len()),
            code::UNKNOWN,
            format!("poenv history {key}"),
        ));
    };
    set_var(store, target, key, &version.value.clone())
}

/// The env's final key → value map with references resolved, sorted by key.
pub fn resolve_env(store: &Store, app: &str, env: &str) -> Res<IndexMap<String, String>> {
    let vars = &get_env(store, app, env)?.vars;
    let mut keys: Vec<&String> = vars.keys().collect();
    keys.sort();
    let mut out = IndexMap::new();
    for key in keys {
        match &vars[key] {
            Var::Lit(v) => {
                out.insert(key.clone(), v.value.clone());
            }
            Var::Ref(r) => {
                let Some(shared) = store.shared.get(&r.target) else {
                    return Err(PoenvError::fix(
                        format!(
                            "{key} in {app}/{env} points to shared key \"{}\", which no longer exists.",
                            r.target
                        ),
                        code::UNKNOWN,
                        format!("poenv set {} --shared", r.target),
                    ));
                };
                out.insert(key.clone(), shared.value.clone());
            }
        }
    }
    Ok(out)
}

fn assert_key(key: &str) -> Res<()> {
    if valid_key(key) {
        return Ok(());
    }
    Err(PoenvError::new(format!(
        "\"{key}\" is not a valid variable name. Use letters, digits and \"_\", not starting with a digit."
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(env: &str) -> Target {
        Target::app("my-app", env)
    }

    fn with_app() -> Store {
        let mut s = empty_store();
        create_app(&mut s, "my-app").unwrap();
        s
    }

    fn lit(s: &Store, env: &str, key: &str) -> Variable {
        match &s.apps["my-app"].envs[env].vars[key] {
            Var::Lit(v) => v.clone(),
            Var::Ref(_) => panic!("{key} is a reference"),
        }
    }

    fn resolved(s: &Store, env: &str) -> Vec<(String, String)> {
        resolve_env(s, "my-app", env).unwrap().into_iter().collect()
    }

    fn kv(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn new_apps_get_local_staging_prod_with_prod_protected() {
        let mut s = with_app();
        let envs: Vec<&String> = s.apps["my-app"].envs.keys().collect();
        assert_eq!(envs, ["local", "staging", "prod"]);
        assert!(s.apps["my-app"].envs["prod"].protected && !s.apps["my-app"].envs["local"].protected);
        assert!(create_app(&mut s, "my-app").unwrap_err().problem.contains("already exists"));
        assert!(set_var(&mut s, &Target::app("nope", "local"), "K", "v").unwrap_err().fix.unwrap().contains("poenv init"));
        assert!(set_var(&mut s, &app("qa"), "K", "v").unwrap_err().problem.contains("local, staging, prod"));
    }

    #[test]
    fn history_keeps_previous_values_and_skips_no_ops() {
        let mut s = with_app();
        set_var(&mut s, &app("local"), "DB", "one").unwrap();
        set_var(&mut s, &app("local"), "DB", "one").unwrap();
        assert!(lit(&s, "local", "DB").history.is_empty());
        set_var(&mut s, &app("local"), "DB", "two").unwrap();
        let v = lit(&s, "local", "DB");
        assert_eq!(v.value, "two");
        assert_eq!(v.history.iter().map(|h| h.value.as_str()).collect::<Vec<_>>(), ["one"]);
    }

    #[test]
    fn keeps_only_the_last_10_versions() {
        let mut s = empty_store();
        for i in 0..=15 {
            set_var(&mut s, &Target::Shared, "K", &format!("v{i}")).unwrap();
        }
        let v = &s.shared["K"];
        assert_eq!(v.value, "v15");
        assert_eq!(v.history.len(), 10);
        assert_eq!((v.history[0].value.as_str(), v.history[9].value.as_str()), ("v14", "v5"));
    }

    #[test]
    fn rollback_restores_and_keeps_the_current_value() {
        let mut s = empty_store();
        for v in ["a", "b", "c"] {
            set_var(&mut s, &Target::Shared, "K", v).unwrap();
        }
        rollback_var(&mut s, &Target::Shared, "K", 1).unwrap(); // history[1] is "a"
        assert_eq!(s.shared["K"].value, "a");
        assert_eq!(s.shared["K"].history.iter().map(|h| h.value.as_str()).collect::<Vec<_>>(), ["c", "b", "a"]);
        let mut t = empty_store();
        set_var(&mut t, &Target::Shared, "K", "a").unwrap();
        assert!(rollback_var(&mut t, &Target::Shared, "K", 0).unwrap_err().problem.contains("no version"));
    }

    #[test]
    fn invalid_key_names_are_refused() {
        let mut s = empty_store();
        for bad in ["", "1ABC", "MY-KEY", "A B", "A=B", "FOO.BAR"] {
            assert!(set_var(&mut s, &Target::Shared, bad, "v").is_err(), "{bad}");
        }
        set_var(&mut s, &Target::Shared, "_ok_KEY_1", "v").unwrap();
    }

    #[test]
    fn references_resolve_live_and_can_be_renamed() {
        let mut s = with_app();
        set_var(&mut s, &Target::Shared, "OPENAI_API_KEY", "sk-1").unwrap();
        set_ref(&mut s, "my-app", "local", "OPENAI_API_KEY", "OPENAI_API_KEY").unwrap();
        set_ref(&mut s, "my-app", "staging", "NEXT_PUBLIC_OPENAI_KEY", "OPENAI_API_KEY").unwrap();
        assert_eq!(resolved(&s, "local"), kv(&[("OPENAI_API_KEY", "sk-1")]));
        set_var(&mut s, &Target::Shared, "OPENAI_API_KEY", "sk-2").unwrap();
        assert_eq!(resolved(&s, "local"), kv(&[("OPENAI_API_KEY", "sk-2")]));
        assert_eq!(resolved(&s, "staging"), kv(&[("NEXT_PUBLIC_OPENAI_KEY", "sk-2")]));
        let e = set_ref(&mut s, "my-app", "local", "X", "MISSING").unwrap_err();
        assert_eq!(e.fix.as_deref(), Some("poenv set MISSING --shared"));
    }

    #[test]
    fn a_dangling_reference_fails_resolve_with_unknown() {
        let mut s = with_app();
        set_var(&mut s, &Target::Shared, "K", "v").unwrap();
        set_ref(&mut s, "my-app", "local", "K", "K").unwrap();
        remove_var(&mut s, &Target::Shared, "K").unwrap();
        assert_eq!(resolve_env(&s, "my-app", "local").unwrap_err().code, code::UNKNOWN);
    }

    #[test]
    fn literals_and_references_replace_each_other_and_resolve_sorted() {
        let mut s = with_app();
        set_var(&mut s, &Target::Shared, "K", "shared-v").unwrap();
        set_ref(&mut s, "my-app", "local", "K", "K").unwrap();
        set_var(&mut s, &app("local"), "K", "literal").unwrap();
        assert_eq!(resolved(&s, "local"), kv(&[("K", "literal")]));
        set_ref(&mut s, "my-app", "local", "K", "K").unwrap();
        assert_eq!(resolved(&s, "local"), kv(&[("K", "shared-v")]));
        set_var(&mut s, &app("staging"), "Z", "z").unwrap();
        set_ref(&mut s, "my-app", "staging", "A", "K").unwrap();
        assert_eq!(resolved(&s, "staging").into_iter().map(|(k, _)| k).collect::<Vec<_>>(), ["A", "Z"]);
    }

    #[test]
    fn remove_var_and_missing_keys() {
        let mut s = with_app();
        set_var(&mut s, &app("local"), "K", "v").unwrap();
        remove_var(&mut s, &app("local"), "K").unwrap();
        assert!(resolved(&s, "local").is_empty());
        assert!(remove_var(&mut s, &app("local"), "K").unwrap_err().problem.contains("not found"));
    }

    #[test]
    fn copy_env_copies_values_once_and_keeps_references() {
        let mut s = with_app();
        set_var(&mut s, &app("prod"), "DB", "v1").unwrap();
        set_var(&mut s, &app("prod"), "DB", "v2").unwrap();
        set_var(&mut s, &Target::Shared, "OPENAI_API_KEY", "sk-1").unwrap();
        set_ref(&mut s, "my-app", "prod", "OPENAI_API_KEY", "OPENAI_API_KEY").unwrap();
        copy_env(&mut s, "my-app", "prod", "client-b").unwrap();
        let copy = &s.apps["my-app"].envs["client-b"];
        assert!(!copy.protected);
        assert!(matches!(&copy.vars["DB"], Var::Lit(v) if v.value == "v2" && v.history.is_empty()));
        assert!(matches!(&copy.vars["OPENAI_API_KEY"], Var::Ref(r) if r.target == "OPENAI_API_KEY"));
        set_var(&mut s, &app("client-b"), "DB", "other").unwrap();
        assert_eq!(resolve_env(&s, "my-app", "prod").unwrap()["DB"], "v2");
        assert!(copy_env(&mut s, "my-app", "local", "staging").unwrap_err().problem.contains("already has"));
        assert!(copy_env(&mut s, "my-app", "local", "a b").unwrap_err().problem.contains("not a valid environment name"));
        assert!(copy_env(&mut s, "my-app", "qa", "uat").unwrap_err().problem.contains("No environment \"qa\""));
    }

    #[test]
    fn rename_env_keeps_keys_protection_and_position() {
        let mut s = with_app();
        set_var(&mut s, &app("prod"), "DB", "v1").unwrap();
        rename_env(&mut s, "my-app", "prod", "production").unwrap();
        let envs: Vec<&String> = s.apps["my-app"].envs.keys().collect();
        assert_eq!(envs, ["local", "staging", "production"]);
        assert!(s.apps["my-app"].envs["production"].protected);
        assert_eq!(resolved(&s, "production"), kv(&[("DB", "v1")]));
        assert!(rename_env(&mut s, "my-app", "local", "staging").unwrap_err().problem.contains("already has"));
    }

    #[test]
    fn moving_keys_in_and_out_of_the_shared_pool() {
        let mut s = with_app();
        set_var(&mut s, &app("local"), "K", "a").unwrap();
        set_var(&mut s, &app("local"), "K", "b").unwrap();
        assert!(move_to_shared(&mut s, "my-app", "local", "K").unwrap());
        assert_eq!(s.shared["K"].value, "b");
        assert_eq!(s.shared["K"].history[0].value, "a");
        assert!(matches!(&s.apps["my-app"].envs["local"].vars["K"], Var::Ref(r) if r.target == "K"));

        let mut t = with_app();
        set_var(&mut t, &Target::Shared, "K", "x").unwrap();
        set_var(&mut t, &app("local"), "K", "y").unwrap();
        assert!(move_to_shared(&mut t, "my-app", "local", "K").unwrap_err().problem.contains("different shared K"));
    }

    #[test]
    fn remove_shared_keep_copies_or_everywhere() {
        let mut s = with_app();
        set_var(&mut s, &Target::Shared, "K", "a").unwrap();
        set_var(&mut s, &Target::Shared, "K", "b").unwrap();
        set_ref(&mut s, "my-app", "local", "K", "K").unwrap();
        set_ref(&mut s, "my-app", "staging", "NEXT_PUBLIC_K", "K").unwrap();
        remove_shared(&mut s, "K", RemoveMode::KeepCopies).unwrap();
        assert!(!s.shared.contains_key("K"));
        assert_eq!(resolved(&s, "local"), kv(&[("K", "b")]));
        assert_eq!(resolved(&s, "staging"), kv(&[("NEXT_PUBLIC_K", "b")]));
        assert_eq!(lit(&s, "local", "K").history[0].value, "a");

        let mut t = with_app();
        set_var(&mut t, &Target::Shared, "K", "v").unwrap();
        set_ref(&mut t, "my-app", "local", "K", "K").unwrap();
        remove_shared(&mut t, "K", RemoveMode::Everywhere).unwrap();
        assert!(resolved(&t, "local").is_empty());
    }
}
