// End-to-end: runs the real binary as a subprocess, the way an agent would (no TTY).
mod common;
use common::*;
use poenv::store::model::{Target, resolve_env, set_ref, set_var};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

fn count_lines_starting(text: &str, prefix: &str) -> usize {
    text.lines().filter(|l| l.starts_with(prefix)).count()
}

fn env_entry(out: &Value, env: &str) -> Value {
    out["envs"].as_array().unwrap().iter().find(|e| e["env"] == env).unwrap().clone()
}

// ---- init ----

#[test]
fn init_sets_up_a_nextjs_repo() {
    let t = T::new();
    let root = repo();
    fs::write(root.join(".env.local"), "DB=postgres://localhost/dev\nOPENAI_API_KEY=sk-123\nDEBUG=true\n").unwrap();
    let r = t.poenv(&root, &["init", "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_match!(r.json(), {
        "ok": true, "app": "my-app", "env": "local", "file": ".env.local", "format": "next",
        "imported": ["DB", "DEBUG", "OPENAI_API_KEY"], "gitignore": "added", "markerGitignore": "added",
    });
    let text = read(&root, ".env.local");
    assert!(text.contains("# app: my-app  env: local"));
    assert_eq!(count_lines_starting(&text, "DB="), 1); // moved into the block, not duplicated
    assert!(read(&root, ".poenv").contains("format = \"next\""));
    assert!(read(&root, ".gitignore").contains("/.env.local"));
    assert!(read(&root, ".gitignore").contains("/.poenv"));
    assert!(read(&root, "AGENTS.md").contains("If this folder has a `.poenv` file"));
    assert!(!r.stdout.contains("sk-123"));
}

#[test]
fn init_twice_is_refused() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    assert_eq!(t.poenv(&root, &["init"]).code, ERROR);
}

#[test]
fn without_a_vault_an_agent_cant_create_one() {
    let mut t = T::new();
    t.empty_vault();
    let r = t.poenv(&repo(), &["init", "--json"]);
    assert_eq!(r.code, WAITING_ON_USER);
    assert!(r.json()["fix"].as_str().unwrap().contains("poenv init"));
}

#[test]
fn init_reports_duplicate_values_in_other_apps() {
    let t = T::new();
    t.poenv(&next_repo("app-one", "OPENAI_API_KEY=sk-same\n"), &["init", "--json"]);
    let r = t.poenv(&next_repo("app-two", "OPENAI_API_KEY=sk-same\n"), &["init", "--json"]);
    assert_eq!(
        r.json()["duplicates"],
        json!([{ "key": "OPENAI_API_KEY", "places": [{ "app": "app-one", "env": "local" }], "sharedExists": false }])
    );
}

#[test]
fn init_manages_the_env_file_that_has_the_keys() {
    let t = T::new();
    let root = next_repo("only-env", "");
    fs::write(root.join(".env"), "DB=postgres://staging-host/app\n").unwrap();
    fs::write(root.join(".env.example"), "DB=\nOTHER=x\n").unwrap();
    let r = t.poenv(&root, &["init", "--json", "--env", "staging"]);
    assert_match!(r.json(), { "file": ".env", "env": "staging", "imported": ["DB"] });
    assert!(!root.join(".env.local").exists());
}

#[test]
fn with_keys_in_several_env_files_scripts_get_the_usual_one() {
    let t = T::new();
    let root = repo();
    fs::write(root.join(".env"), "NEXT_PUBLIC_URL=http://localhost\n").unwrap();
    assert_match!(t.poenv(&root, &["init", "--json"]).json(), { "file": ".env.local" });
}

#[test]
fn init_leaves_keys_poenv_cant_store_in_the_file() {
    let t = T::new();
    let root = next_repo("my-app", "DB=postgres://localhost/dev\nFOO.BAR=1\nMY-KEY=2\n");
    let r = t.poenv(&root, &["init", "--json"]);
    assert_eq!(r.code, 0, "{}", r.stdout);
    assert_match!(r.json(), { "imported": ["DB"], "invalid": ["FOO.BAR", "MY-KEY"] });
    let text = read(&root, ".env.local");
    assert!(text.contains("FOO.BAR=1") && text.contains("MY-KEY=2"));
    let human = t.poenv(&next_repo("other", "OK=1\nBAD.KEY=x\n"), &["init"]);
    assert!(human.stdout.contains("BAD.KEY can't be stored in poenv"));
}

// ---- use ----

#[test]
fn use_switches_env_and_reports_the_diff_by_key_name_only() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    assert_eq!(t.poenv_in(&root, &["set", "DB", "--env", "staging"], "postgres://staging/db").code, 0);
    let r = t.poenv(&root, &["use", "staging", "--json"]);
    assert_eq!(r.code, 0);
    assert_match!(r.json(), {
        "env": "staging", "from": { "app": "my-app", "env": "local" },
        "diff": { "added": [], "changed": ["DB"], "removed": ["OPENAI_API_KEY"] },
    });
    assert!(read(&root, ".env.local").contains("DB=postgres://staging/db"));
    assert!(!r.stdout.contains("postgres://staging/db"));
    assert_eq!(t.poenv(&root, &["status", "--short"]).stdout.trim(), "my-app:staging");
}

#[test]
fn an_agent_cant_switch_to_a_protected_env() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    let before = read(&root, ".env.local");
    let r = t.poenv(&root, &["use", "prod", "--json"]);
    assert_eq!(r.code, REFUSED);
    assert!(r.json()["error"].as_str().unwrap().contains("protected"));
    assert_eq!(read(&root, ".env.local"), before);
}

#[test]
fn unknown_env_lists_the_real_ones() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    let r = t.poenv(&root, &["use", "qa"]);
    assert_eq!(r.code, UNKNOWN);
    assert!(r.stderr.contains("local, staging, prod"));
}

#[test]
fn the_vault_folder_above_the_cwd_is_not_a_repo() {
    let t = T::new();
    let home = temp();
    fs::create_dir(home.join(".poenv")).unwrap();
    let dir = home.join("git").join("somewhere");
    fs::create_dir_all(&dir).unwrap();
    let r = t.poenv(&dir, &["status", "--json"]);
    assert_eq!(r.code, NOT_SET_UP);
    assert_eq!(r.json()["fix"], "poenv init");
    assert!(t.poenv(&dir, &["doctor", "--json"]).json()["checks"].is_array());
    assert_eq!(t.poenv(&dir, &["ls", "--json"]).code, 0);
}

#[test]
fn works_from_a_subfolder() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    let sub = root.join("src").join("lib");
    fs::create_dir_all(&sub).unwrap();
    assert_eq!(t.poenv(&sub, &["status", "--json"]).json()["app"], "my-app");
}

// ---- shared keys ----

#[test]
fn add_pulls_in_a_shared_key_and_rotating_it_rewrites_the_file() {
    let t = T::new();
    let root = next_repo("my-app", "");
    t.poenv(&root, &["init", "--json"]);
    t.poenv_in(&root, &["set", "OPENAI_API_KEY", "--shared"], "sk-old");
    assert_eq!(t.poenv(&root, &["add", "OPENAI_API_KEY", "--json"]).code, 0);
    assert!(read(&root, ".env.local").contains("OPENAI_API_KEY=sk-old"));
    t.poenv_in(&root, &["set", "OPENAI_API_KEY", "--shared"], "sk-new");
    assert!(read(&root, ".env.local").contains("OPENAI_API_KEY=sk-new"));
}

#[test]
fn as_renames_and_from_copies_from_another_app() {
    let t = T::new();
    t.poenv(&next_repo("other", "STRIPE_KEY=sk-stripe\n"), &["init", "--json"]);
    let root = next_repo("my-app", "");
    t.poenv(&root, &["init", "--json"]);
    t.poenv_in(&root, &["set", "OPENAI_API_KEY", "--shared"], "sk-1");
    t.poenv(&root, &["add", "OPENAI_API_KEY", "--as", "NEXT_PUBLIC_OPENAI_KEY"]);
    t.poenv(&root, &["add", "STRIPE_KEY", "--from", "other/local"]);
    let text = read(&root, ".env.local");
    assert!(text.contains("NEXT_PUBLIC_OPENAI_KEY=sk-1"));
    assert!(text.contains("STRIPE_KEY=sk-stripe"));
}

fn repo_with_shared_k(t: &T) -> std::path::PathBuf {
    let root = next_repo("my-app", "");
    t.poenv(&root, &["init", "--json"]);
    t.poenv_in(&root, &["set", "K", "--shared"], "v");
    t.poenv(&root, &["add", "K"]);
    root
}

#[test]
fn rm_shared_on_a_used_key_needs_a_flag_from_agents() {
    let t = T::new();
    let root = repo_with_shared_k(&t);
    let r = t.poenv(&root, &["rm", "K", "--shared", "--json"]);
    assert_eq!(r.code, ERROR);
    assert!(r.json()["fix"].as_str().unwrap().contains("--keep-copies"));
    assert!(read(&root, ".env.local").contains("K=v"));
}

#[test]
fn rm_shared_keep_copies_undoes_sharing() {
    let t = T::new();
    let root = repo_with_shared_k(&t);
    assert_eq!(t.poenv(&root, &["rm", "K", "--shared", "--keep-copies"]).code, 0);
    assert!(read(&root, ".env.local").contains("K=v"));
    let local = env_entry(&t.poenv(&root, &["ls", "--json"]).json(), "local");
    assert!(local["keys"].as_array().unwrap().contains(&json!({ "key": "K", "ref": null })));
}

#[test]
fn rm_shared_everywhere_removes_it_from_the_apps_too() {
    let t = T::new();
    let root = repo_with_shared_k(&t);
    assert_eq!(t.poenv(&root, &["rm", "K", "--shared", "--everywhere"]).code, 0);
    assert!(!read(&root, ".env.local").contains("K="));
    assert!(!t.poenv(&root, &["doctor", "--json"]).stdout.contains("dangling"));
}

#[test]
fn rotating_a_shared_key_rewrites_every_repo_that_uses_it() {
    let t = T::new();
    let one = next_repo("app-one", "");
    let two = next_repo("app-two", "DEBUG=true\n");
    let other = next_repo("app-three", "");
    for root in [&one, &two, &other] {
        t.poenv(root, &["init", "--json"]);
    }
    t.poenv_in(&one, &["set", "OPENAI_API_KEY", "--shared"], "sk-old");
    t.poenv(&one, &["add", "OPENAI_API_KEY"]);
    t.poenv(&two, &["add", "OPENAI_API_KEY", "--as", "NEXT_PUBLIC_OPENAI_KEY"]);
    let unrelated = read(&other, ".env.local");

    // From outside any repo: no "this repo" file, both others still get it.
    let r = t.poenv_in(&temp(), &["set", "OPENAI_API_KEY", "--shared", "--json"], "sk-new");
    assert_eq!(r.code, 0);
    assert!(read(&one, ".env.local").contains("OPENAI_API_KEY=sk-new"));
    assert!(read(&two, ".env.local").contains("NEXT_PUBLIC_OPENAI_KEY=sk-new"));
    assert!(read(&two, ".env.local").contains("DEBUG=true")); // outside the block: untouched
    assert_eq!(read(&other, ".env.local"), unrelated);
    let mut apps: Vec<String> =
        r.json()["otherFiles"].as_array().unwrap().iter().map(|o| o["app"].as_str().unwrap().to_string()).collect();
    apps.sort();
    assert_eq!(apps, ["app-one", "app-two"]);
    assert!(!r.stdout.contains("sk-new"));

    // From inside one repo: that repo is "file", the other is listed by key name only.
    let human = t.poenv_in(&one, &["set", "OPENAI_API_KEY", "--shared"], "sk-newer");
    assert!(human.stdout.contains("Also updated 1 other env file"));
    assert!(human.stdout.contains("app-two/local: NEXT_PUBLIC_OPENAI_KEY"));
    assert!(!human.stdout.contains("sk-newer"));
    assert!(read(&two, ".env.local").contains("NEXT_PUBLIC_OPENAI_KEY=sk-newer"));
}

#[test]
fn other_repos_keep_their_env() {
    let t = T::new();
    let one = next_repo("app-one", "");
    let two = next_repo("app-two", "");
    t.poenv(&one, &["init", "--json"]);
    t.poenv(&two, &["init", "--json"]);
    t.poenv_in(&one, &["set", "K", "--shared"], "v1");
    t.poenv(&two, &["add", "K", "--env", "staging"]);
    t.poenv(&two, &["use", "staging"]);
    t.poenv_in(&one, &["set", "K", "--shared"], "v2");
    assert!(read(&two, ".env.local").contains("env: staging"));
    assert!(read(&two, ".env.local").contains("K=v2"));
}

#[test]
fn repos_that_are_gone_are_skipped_and_forgotten() {
    let t = T::new();
    let one = next_repo("app-one", "");
    let gone = next_repo("app-gone", "");
    t.poenv(&one, &["init", "--json"]);
    t.poenv(&gone, &["init", "--json"]);
    fs::remove_dir_all(&gone).unwrap();
    t.poenv_in(&one, &["set", "K", "--shared"], "v");
    assert_eq!(t.poenv(&one, &["add", "K"]).code, 0);
    let repos: Value = serde_json::from_str(&fs::read_to_string(t.vault.join("repos.json")).unwrap()).unwrap();
    let repos = repos["repos"].as_array().unwrap();
    assert_eq!(repos.len(), 1);
    assert!(repos[0].as_str().unwrap().ends_with("app-one"));
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}
#[cfg(not(unix))]
fn set_mode(path: &Path, mode: u32) {
    let mut p = fs::metadata(path).unwrap().permissions();
    p.set_readonly(mode & 0o200 == 0);
    fs::set_permissions(path, p).unwrap();
}

#[test]
fn a_repo_that_cant_be_written_is_reported_and_kept() {
    let t = T::new();
    let one = next_repo("app-one", "");
    let locked = next_repo("app-locked", "");
    t.poenv(&one, &["init", "--json"]);
    t.poenv(&locked, &["init", "--json"]);
    t.poenv_in(&one, &["set", "K", "--shared"], "v1");
    t.poenv(&locked, &["add", "K"]);
    // The atomic write creates a temp file next to the env file (POSIX); Windows has no folder
    // permissions but refuses to replace a read-only file.
    set_mode(&locked, 0o555);
    set_mode(&locked.join(".env.local"), 0o444);
    let r = t.poenv_in(&one, &["set", "K", "--shared", "--json"], "v2");
    let human = t.poenv_in(&one, &["set", "K", "--shared"], "v3");
    set_mode(&locked, 0o755);
    set_mode(&locked.join(".env.local"), 0o600);
    assert_eq!(r.code, 0);
    let path = locked.join(".env.local").to_string_lossy().into_owned();
    assert_eq!(r.json()["skippedFiles"], json!([{ "path": path, "reason": "permission denied" }]));
    assert_eq!(human.code, 0);
    assert!(human.stdout.contains("Couldn't update"));
    assert!(human.stdout.contains("Run there: poenv use"));
    assert!(!human.stdout.contains("v3"));
    assert!(read(&locked, ".env.local").contains("K=v1"));
    assert_eq!(t.poenv_in(&one, &["set", "K", "--shared"], "v4").code, 0);
    assert!(read(&locked, ".env.local").contains("K=v4")); // still known, so it catches up
}

#[test]
fn adding_a_shared_key_that_doesnt_exist_says_how_to_create_it() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    let r = t.poenv(&root, &["add", "NOPE", "--json"]);
    assert_eq!(r.code, UNKNOWN);
    assert_eq!(r.json()["fix"], "poenv set NOPE --shared");
}

// ---- env ----

#[test]
fn env_new_creates_and_copies_and_use_switches_to_it() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    assert_match!(t.poenv(&root, &["env", "new", "qa", "--json"]).json(), { "env": "qa", "keys": 0 });
    let r = t.poenv(&root, &["env", "new", "client-b", "--from", "local", "--json"]);
    assert_eq!(r.code, 0);
    assert_match!(r.json(), { "app": "my-app", "env": "client-b", "from": "local", "keys": 2 });
    assert!(!r.stdout.contains("sk-123"));
    t.poenv_in(&root, &["set", "DB", "--env", "client-b"], "postgres://client-b/db");
    assert_eq!(t.poenv(&root, &["use", "client-b", "--json"]).code, 0);
    let text = read(&root, ".env.local");
    assert!(text.contains("DB=postgres://client-b/db"));
    assert!(text.contains("OPENAI_API_KEY=sk-123"));
    assert_eq!(t.poenv(&root, &["env", "new", "qa"]).code, ERROR); // exists
}

#[test]
fn env_rename_moves_the_env_file_and_poenv_default_along() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    let r = t.poenv(&root, &["env", "rename", "local", "dev", "--json"]);
    assert_match!(r.json(), { "from": "local", "to": "dev", "fileUpdated": true, "markerUpdated": true });
    assert!(read(&root, ".env.local").contains("# app: my-app  env: dev"));
    assert!(read(&root, ".poenv").contains("env = \"dev\""));
    assert_eq!(t.poenv(&root, &["status", "--short"]).stdout.trim(), "my-app:dev");
    assert_eq!(t.poenv(&root, &["use", "local"]).code, UNKNOWN);
}

#[test]
fn file_moves_the_block_and_keeps_your_own_lines() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    fs::write(root.join(".env.local"), read(&root, ".env.local") + "MINE=1\n").unwrap();
    assert_match!(t.poenv(&root, &["file", "--json"]).json(), { "file": ".env.local" });
    let r = t.poenv(&root, &["file", ".env", "--json"]);
    assert_match!(r.json(), { "from": ".env.local", "to": ".env", "moved": true, "env": "local" });
    assert!(!r.stdout.contains("sk-123"));
    assert_eq!(read(&root, ".env.local"), "MINE=1\n");
    assert!(read(&root, ".env").contains("OPENAI_API_KEY=sk-123"));
    assert!(read(&root, ".poenv").contains("file = \".env\""));
    assert!(read(&root, ".gitignore").contains("/.env\n"));
    t.poenv_in(&root, &["set", "DB", "--env", "local"], "postgres://new/db");
    assert!(read(&root, ".env").contains("DB=postgres://new/db"));
    assert_eq!(t.poenv(&root, &["file", "../x"]).code, ERROR);
}

#[test]
fn agents_cant_copy_or_rename_a_protected_env() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    let copy = t.poenv(&root, &["env", "new", "prod-copy", "--from", "prod", "--json"]);
    assert_eq!(copy.code, REFUSED);
    assert!(copy.json()["fix"].as_str().unwrap().contains("poenv env new prod-copy --from prod"));
    assert_eq!(t.poenv(&root, &["env", "rename", "prod", "production"]).code, REFUSED);
    assert!(!t.poenv(&root, &["ls", "--json"]).stdout.contains("prod-copy"));
}

#[test]
fn bad_env_usage_points_at_help() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    for args in [vec!["env"], vec!["env", "new"], vec!["env", "rename", "local"], vec!["env", "nope", "x"]] {
        let mut a = args.clone();
        a.push("--json");
        let r = t.poenv(&root, &a);
        assert_eq!(r.code, ERROR);
        assert_eq!(r.json()["fix"], "poenv env --help");
    }
}

#[test]
fn init_env_creates_a_custom_env_and_imports_into_it() {
    let t = T::new();
    let root = repo();
    let r = t.poenv(&root, &["init", "--env", "client-a", "--json"]);
    assert_eq!(r.code, 0);
    assert_match!(r.json(), { "env": "client-a", "imported": ["DB", "OPENAI_API_KEY"] });
    assert!(read(&root, ".env.local").contains("# app: my-app  env: client-a"));
    assert!(read(&root, ".poenv").contains("env = \"client-a\""));
}

#[test]
fn a_fresh_clone_with_a_custom_env_sets_up_on_a_new_machine() {
    let mut t = T::new();
    let root = next_repo("my-app", "");
    t.poenv(&root, &["init", "--env", "client-a", "--json"]);
    t.new_vault();
    let r = t.poenv(&root, &["init", "--json"]);
    assert_eq!(r.code, 0);
    assert_match!(r.json(), { "env": "client-a", "appExisted": false });
}

// ---- set / rm ----

#[test]
fn set_refuses_a_value_passed_as_an_argument() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    let r = t.poenv(&root, &["set", "DB", "secret-value"]);
    assert_eq!(r.code, ERROR);
    assert!(r.stderr.contains("never passed as arguments"));
}

#[test]
fn empty_stdin_is_an_error_unless_empty() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    assert_eq!(t.poenv_in(&root, &["set", "X"], "").code, ERROR);
    assert_eq!(t.poenv(&root, &["set", "X", "--empty"]).code, 0);
    assert!(read(&root, ".env.local").contains("X=\n"));
}

#[test]
fn rm_removes_the_key_from_the_vault_and_the_file() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    assert_eq!(t.poenv(&root, &["rm", "DB"]).code, 0);
    assert!(!read(&root, ".env.local").contains("DB="));
}

// ---- import ----

#[test]
fn import_saves_pasted_keys_overwrites_and_moves_them_into_the_block() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    let pasted = "DB=\"postgres://pasted/db\"\nNEW_KEY=\"a b\"\nEMPTY=\"\"\n";
    fs::write(root.join(".env.local"), read(&root, ".env.local") + pasted).unwrap();
    let r = t.poenv(&root, &["import", "--json"]);
    assert_eq!(r.code, 0);
    assert_match!(r.json(), {
        "app": "my-app", "env": "local", "imported": ["DB", "EMPTY", "NEW_KEY"], "overwritten": ["DB"],
    });
    assert!(!r.stdout.contains("pasted"));
    let text = read(&root, ".env.local");
    assert_eq!(count_lines_starting(&text, "DB="), 1);
    assert!(text.contains("postgres://pasted/db"));
    assert!(text.contains("OPENAI_API_KEY=sk-123"));
    assert_eq!(t.poenv(&root, &["import"]).code, ERROR); // nothing left to import
}

#[test]
fn import_env_saves_into_another_env() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    fs::write(root.join(".env.local"), read(&root, ".env.local") + "STAGING_ONLY=x\n").unwrap();
    assert_eq!(t.poenv(&root, &["import", "--env", "staging"]).code, 0);
    assert!(!read(&root, ".env.local").contains("STAGING_ONLY"));
    t.poenv(&root, &["use", "staging"]);
    assert!(read(&root, ".env.local").contains("STAGING_ONLY=x"));
}

#[test]
fn import_into_a_protected_env_is_refused_for_agents() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    fs::write(root.join(".env.local"), read(&root, ".env.local") + "PROD_KEY=x\n").unwrap();
    assert_eq!(t.poenv(&root, &["import", "--env", "prod"]).code, REFUSED);
    assert!(read(&root, ".env.local").contains("PROD_KEY=x"));
}

// ---- ls / status ----

#[test]
fn ls_json_lists_names_and_references_never_values() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    t.poenv_in(&root, &["set", "SHARED_ONE", "--shared"], "shh");
    t.poenv(&root, &["add", "SHARED_ONE"]);
    let r = t.poenv(&root, &["ls", "--json"]);
    assert_eq!(r.code, 0);
    let local = env_entry(&r.json(), "local");
    assert_eq!(local["current"], true);
    assert!(local["keys"].as_array().unwrap().contains(&json!({ "key": "SHARED_ONE", "ref": "SHARED_ONE" })));
    assert!(!r.stdout.contains("sk-123"));
    assert!(!r.stdout.contains("shh"));
}

#[test]
fn status_outside_a_repo_exits_2_and_short_prints_nothing() {
    let t = T::new();
    let dir = temp();
    let r = t.poenv(&dir, &["status", "--json"]);
    assert_eq!(r.code, NOT_SET_UP);
    assert_eq!(r.json()["fix"], "poenv init");
    assert_eq!(t.poenv(&dir, &["status", "--short"]).stdout, "");
}

// ---- run ----

#[test]
fn run_injects_the_env_and_passes_the_exit_code_through() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    t.poenv_in(&root, &["set", "DB", "--env", "staging"], "it's $staging");
    let c = child();
    assert_eq!(t.poenv(&root, &["run", "staging", "--", &c, "env-is", "DB", "it's $staging"]).code, 0);
    assert_eq!(t.poenv(&root, &["run", "--", &c, "exit", "7"]).code, 7);
}

#[test]
fn run_masks_values_in_the_commands_output() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    t.poenv_in(&root, &["set", "OPENAI_API_KEY"], "sk-123-long");
    let r = t.poenv(&root, &["run", "--", &child(), "mask-demo"]);
    assert_eq!(r.code, 0);
    assert_eq!(r.stdout.replace("\r\n", "\n"), "db ***\ndone\n");
    assert_eq!(r.stderr.replace("\r\n", "\n"), "key ***\n");
}

#[test]
fn run_on_a_protected_env_is_refused_for_agents() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    assert_eq!(t.poenv(&root, &["run", "prod", "--", &child(), "exit", "0"]).code, REFUSED);
}

#[test]
fn json_after_the_separator_belongs_to_the_command() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    let r = t.poenv(&root, &["run", "--", &child(), "last-arg", "--json"]);
    assert_eq!(r.stdout.trim(), "--json");
}

#[test]
fn run_with_a_missing_command_says_so() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    let r = t.poenv(&root, &["run", "--", "no-such-program-xyz", "--json"]);
    assert_eq!(r.code, ERROR);
    assert!(r.stderr.contains("Couldn't start \"no-such-program-xyz\""));
}

// ---- values ----

#[test]
fn run_mode_keeps_values_out_of_the_env_file() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    let r = t.poenv(&root, &["values", "run", "--json"]);
    assert_eq!(r.code, 0);
    assert_match!(r.json(), { "ok": true, "values": "run", "changed": true });
    assert!(read(&root, ".poenv").contains("values = \"run\""));
    let file = read(&root, ".env.local");
    assert!(file.contains("DB=\n") && file.contains("OPENAI_API_KEY=\n"));
    assert!(!file.contains("postgres://localhost/dev") && !file.contains("sk-123"));
    let c = child();
    assert_eq!(t.poenv(&root, &["run", "--", &c, "env-is", "DB", "postgres://localhost/dev"]).code, 0);
}

#[test]
fn later_writes_stay_keys_only() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    t.poenv(&root, &["values", "run"]);
    t.poenv_in(&root, &["set", "DB"], "postgres://changed/db");
    t.poenv_in(&root, &["set", "DB", "--env", "staging"], "postgres://staging/db");
    t.poenv(&root, &["use", "staging", "--json"]);
    let file = read(&root, ".env.local");
    assert!(file.contains("env: staging") && file.contains("DB=\n"));
    assert!(!file.contains("postgres://"));
    let doctor = t.poenv(&root, &["doctor", "--json"]).json();
    assert_eq!(check(&doctor, "env-file").unwrap()["status"], "ok");
}

#[test]
fn switching_back_to_file_is_refused_for_agents() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    t.poenv(&root, &["values", "run"]);
    assert_eq!(t.poenv(&root, &["values", "file", "--json"]).code, REFUSED);
    assert!(read(&root, ".poenv").contains("values = \"run\""));
    assert!(!read(&root, ".env.local").contains("postgres://localhost/dev"));
}

#[test]
fn values_shows_the_mode_and_rejects_unknown_ones() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    assert_match!(t.poenv(&root, &["values", "--json"]).json(), { "values": "file" });
    assert_eq!(t.poenv(&root, &["values", "nope", "--json"]).code, ERROR);
}

// ---- human-only commands ----

#[test]
fn human_only_commands_are_refused_for_agents_with_nothing_revealed() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    let commands: [(&str, Vec<&str>); 6] = [
        ("get", vec!["get", "DB"]),
        ("history", vec!["history", "DB"]),
        ("rollback", vec!["rollback", "DB"]),
        ("backup", vec!["backup"]),
        ("restore", vec!["restore", "backup.enc"]),
        ("recover", vec!["recover"]),
    ];
    for (name, mut args) in commands {
        args.push("--json");
        let r = t.poenv(&root, &args);
        assert_eq!(r.code, REFUSED, "{name}");
        assert!(r.json()["fix"].as_str().unwrap().contains(&format!("poenv {name}")));
        assert!(!r.stdout.contains("postgres://localhost/dev"));
        assert!(!root.join("backup.enc").exists());
    }
}

// ---- doctor ----

#[test]
fn doctor_passes_a_healthy_repo() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    let r = t.poenv(&root, &["doctor", "--json"]);
    assert_eq!(r.code, 0);
    let out = r.json();
    assert_eq!(out["ok"], true);
    assert_eq!(check(&out, "vault").unwrap()["status"], "ok");
    assert_eq!(check(&out, "env-file").unwrap()["status"], "ok");
    assert_match!(check(&out, "recovery-phrase").unwrap().clone(), { "status": "warn", "needsUser": true });
    assert!(out["waitingOnUser"].as_array().unwrap().contains(&json!("recovery-phrase")));
}

#[test]
fn doctor_without_a_vault_exits_5() {
    let mut t = T::new();
    t.empty_vault();
    let r = t.poenv(&temp(), &["doctor", "--json"]);
    assert_eq!(r.code, WAITING_ON_USER);
    assert_match!(r.json(), { "ok": false, "onlyUserCanFix": true });
    assert!(check(&r.json(), "vault").unwrap()["fix"].as_str().unwrap().contains("poenv init"));
}

#[test]
fn a_lost_key_file_points_to_recover() {
    let t = T::new();
    fs::remove_file(t.vault.join("keys.json")).unwrap();
    let r = t.poenv(&temp(), &["doctor", "--json"]);
    assert_eq!(r.code, WAITING_ON_USER);
    assert!(check(&r.json(), "vault").unwrap()["fix"].as_str().unwrap().contains("poenv recover"));
}

#[test]
fn doctor_fix_restores_gitignore_notes_and_a_stale_env_file() {
    let t = T::new();
    let root = next_repo("my-app", "");
    t.poenv(&root, &["init", "--json"]);
    t.poenv_in(&root, &["set", "OPENAI_API_KEY", "--shared"], "sk-old");
    t.poenv(&root, &["add", "OPENAI_API_KEY"]);
    // Rotate the shared key behind poenv's back, and break the repo files.
    update(|s| {
        s.shared.get_mut("OPENAI_API_KEY").unwrap().value = "sk-new".into();
        Ok(())
    });
    fs::write(root.join(".gitignore"), "").unwrap();
    fs::remove_file(root.join("AGENTS.md")).unwrap();

    let before = t.poenv(&root, &["doctor", "--json"]).json();
    let ids = ["agent-notes", "env-file", "gitignore", "marker-gitignore"];
    for id in ids {
        assert_match!(check(&before, id).unwrap().clone(), { "status": "warn", "autoFixable": true });
    }
    let after = t.poenv(&root, &["doctor", "--fix", "--json"]).json();
    let mut fixed: Vec<&str> = after["fixed"].as_array().unwrap().iter().map(|f| f.as_str().unwrap()).collect();
    fixed.sort();
    assert_eq!(fixed, ids);
    for id in ids {
        assert_eq!(check(&after, id).unwrap()["status"], "ok");
    }
    assert!(read(&root, ".env.local").contains("OPENAI_API_KEY=sk-new"));
}

#[test]
fn doctor_fix_clears_a_lock_left_by_a_dead_process() {
    let t = T::new();
    let at = poenv::util::now_ms();
    fs::write(t.vault.join("store.lock"), format!("{{\"pid\":999999999,\"at\":{at}}}")).unwrap();
    assert_eq!(check(&t.poenv(&temp(), &["doctor", "--json"]).json(), "stale-lock").unwrap()["status"], "warn");
    t.poenv(&temp(), &["doctor", "--fix", "--json"]);
    assert!(!t.vault.join("store.lock").exists());
}

#[test]
fn a_dangling_shared_reference_is_a_failure_with_the_fix() {
    let t = T::new();
    let root = repo_with_shared_k(&t);
    update(|s| {
        s.shared.shift_remove("K");
        Ok(())
    });
    let r = t.poenv(&root, &["doctor", "--json"]);
    assert_eq!(r.code, ERROR);
    assert!(check(&r.json(), "dangling-ref").unwrap()["fix"].as_str().unwrap().contains("poenv set K --shared"));
}

#[test]
fn a_fresh_clone_on_a_new_machine_doctor_says_init_and_init_adopts_poenv() {
    let mut t = T::new();
    let root = next_repo("my-app", "");
    t.poenv(&root, &["init", "--json", "--format", "python"]);
    t.new_vault(); // another machine: empty vault, same repo files
    let d = t.poenv(&root, &["doctor", "--json"]).json();
    assert_match!(check(&d, "app").unwrap().clone(), { "status": "fail", "fix": "poenv init (creates it from .poenv)" });
    let r = t.poenv(&root, &["init", "--json"]);
    assert_eq!(r.code, 0);
    assert_match!(r.json(), { "app": "my-app", "format": "python", "appExisted": false });
    assert_eq!(t.poenv(&root, &["init", "--json"]).code, ERROR); // now really set up
}

#[test]
fn a_repo_that_commits_poenv_keeps_it_tracked() {
    let mut t = T::new();
    let root = next_repo("my-app", "");
    fs::write(root.join(".poenv"), "app = \"my-app\"\nformat = \"next\"\n").unwrap();
    git(&root, &["add", ".poenv"]);
    t.new_vault();
    let r = t.poenv(&root, &["init", "--json"]);
    assert_match!(r.json(), { "app": "my-app", "markerGitignore": "tracked" });
    assert!(!read(&root, ".gitignore").contains("/.poenv"));
    assert!(check(&t.poenv(&root, &["doctor", "--json"]).json(), "marker-gitignore").is_none());
}

// ---- no password ----

#[test]
fn every_command_works_with_no_unlock_step() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    assert_eq!(t.poenv(&root, &["use", "staging", "--json"]).code, 0);
    let status = t.poenv(&root, &["status", "--json"]).json();
    assert_eq!(status.as_object().unwrap().len(), 7);
    assert_match!(status.clone(), {
        "ok": true, "app": "my-app", "env": "staging", "written": true, "file": ".env.local", "format": "next",
    });
    assert!(status["root"].is_string());
    for cmd in ["lock", "unlock", "password"] {
        assert_eq!(t.poenv(&root, &[cmd, "--json"]).code, ERROR);
    }
}

// ---- errors ----

#[test]
fn unknown_command_and_unknown_flag_point_at_help() {
    let t = T::new();
    let dir = temp();
    assert!(t.poenv(&dir, &["frobnicate"]).stderr.contains("poenv --help"));
    assert!(t.poenv(&dir, &["ls", "--nope"]).stderr.contains("poenv ls --help"));
}

#[test]
fn help_works_without_a_vault() {
    let mut t = T::new();
    t.empty_vault();
    assert_eq!(t.poenv(&temp(), &["--help"]).code, 0);
    assert!(t.poenv(&temp(), &["use", "--help"]).stdout.contains("poenv use [ENV]"));
    assert!(!t.vault.join("store.enc").exists());
}

#[test]
fn version_prints_the_crate_version() {
    let t = T::new();
    assert_eq!(t.poenv(&temp(), &["--version"]).stdout.trim(), env!("CARGO_PKG_VERSION"));
}

// ---- protected values can't be reached sideways ----

const PROD: &str = "postgres://prod/secret";

fn repo_with_prod(t: &T) -> std::path::PathBuf {
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    update(|s| set_var(s, &Target::app("my-app", "prod"), "DB", PROD));
    root
}

fn prod_value(key: &str) -> Option<String> {
    resolve_env(&poenv::store::open_store().unwrap().data, "my-app", "prod").unwrap().get(key).cloned()
}

#[test]
fn agents_cant_copy_a_protected_value_into_another_env() {
    let t = T::new();
    let root = repo_with_prod(&t);
    let r = t.poenv(&root, &["add", "DB", "--from", "my-app/prod", "--as", "LEAK", "--json"]);
    assert_eq!(r.code, REFUSED);
    assert!(r.json()["fix"].as_str().unwrap().contains("poenv add DB --from my-app/prod --as LEAK"));
    let other = next_repo("other-app", "X=1\n");
    t.poenv(&other, &["init", "--json"]);
    assert_eq!(t.poenv(&other, &["add", "DB", "--from", "my-app/prod"]).code, REFUSED);
    assert!(!(read(&root, ".env.local") + &read(&other, ".env.local")).contains(PROD));
}

#[test]
fn agents_cant_change_a_protected_env() {
    let t = T::new();
    let root = repo_with_prod(&t);
    assert_eq!(t.poenv_in(&root, &["set", "DB", "--env", "prod"], "postgres://evil").code, REFUSED);
    assert_eq!(t.poenv(&root, &["rm", "DB", "--env", "prod"]).code, REFUSED);
    assert_eq!(t.poenv(&root, &["add", "DB", "--from", "my-app/local", "--env", "prod"]).code, REFUSED);
    let other = next_repo("other", "DB=postgres://evil\n");
    assert_eq!(t.poenv(&other, &["init", "--app", "my-app", "--env", "prod", "--json"]).code, REFUSED);
    assert_eq!(prod_value("DB").as_deref(), Some(PROD));
}

#[test]
fn a_shared_key_prod_uses_is_guarded_like_prod() {
    let t = T::new();
    let root = repo_with_prod(&t);
    t.poenv_in(&root, &["set", "STRIPE", "--shared"], "sk_live_1");
    update(|s| set_ref(s, "my-app", "prod", "STRIPE", "STRIPE"));

    // Linking it into an unprotected env would show prod's value.
    assert_eq!(t.poenv(&root, &["add", "STRIPE"]).code, REFUSED);
    assert!(!read(&root, ".env.local").contains("sk_live_1"));
    // Changing or removing it changes prod.
    assert_eq!(t.poenv_in(&root, &["set", "STRIPE", "--shared"], "sk_evil").code, REFUSED);
    assert_eq!(t.poenv(&root, &["rm", "STRIPE", "--shared", "--everywhere"]).code, REFUSED);
    assert_eq!(prod_value("STRIPE").as_deref(), Some("sk_live_1"));

    // Once an unprotected env uses it too, it's no longer prod's alone.
    update(|s| set_ref(s, "my-app", "staging", "STRIPE", "STRIPE"));
    assert_eq!(t.poenv(&root, &["add", "STRIPE"]).code, 0);
}

#[test]
fn poenv_never_writes_values_into_a_file_git_tracks() {
    let t = T::new();
    let root = repo();
    t.poenv(&root, &["init", "--json"]);
    fs::write(root.join("README.md"), "hi\n").unwrap();
    git(&root, &["add", "README.md"]);

    assert_eq!(t.poenv(&root, &["file", "README.md"]).code, ERROR);
    // Pointing .poenv at it by hand doesn't get around it either.
    let marker = read(&root, ".poenv").replace("file = \".env.local\"", "file = \"README.md\"");
    fs::write(root.join(".poenv"), marker).unwrap();
    assert_eq!(t.poenv(&root, &["use", "local"]).code, ERROR);
    assert_eq!(read(&root, "README.md"), "hi\n");

    let fresh = next_repo("fresh", "");
    fs::write(fresh.join("README.md"), "hi\n").unwrap();
    git(&fresh, &["add", "README.md"]);
    assert_eq!(t.poenv(&fresh, &["init", "--file", "README.md"]).code, ERROR);
}

#[cfg(target_os = "macos")]
#[test]
fn a_faked_terminal_without_agent_markers_is_still_an_agent() {
    let t = T::new();
    let root = repo_with_prod(&t);
    // An agent (a program named "claude") unsets its markers and fakes a TTY with script(1).
    let dir = temp().join("agent");
    fs::create_dir(&dir).unwrap();
    std::os::unix::fs::symlink(child(), dir.join("claude")).unwrap();
    let bin = bin().to_string_lossy().into_owned();
    let out = std::process::Command::new(dir.join("claude"))
        .args(["spawn", "script", "-q", "/dev/null", &bin, "get", "DB", "--env", "prod"])
        .current_dir(&root)
        .env("POENV_HOME", &t.vault)
        .env("NO_COLOR", "1")
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .env_remove("AI_AGENT")
        .env_remove("CI")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("only a person at a terminal"), "{text}");
    assert!(!text.contains(PROD));
}

#[cfg(unix)]
#[test]
fn an_agent_parent_is_found_even_without_its_env_markers() {
    let _t = T::new();
    let dir = temp().join("Application Support");
    fs::create_dir(&dir).unwrap();
    std::os::unix::fs::symlink(child(), dir.join("claude")).unwrap();
    let out = std::process::Command::new(dir.join("claude")).args(["spawn", &child(), "ancestor"]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "claude");
}
