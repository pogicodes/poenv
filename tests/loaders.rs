// Checks the written block against the real loaders (Node dotenv, Next.js, Expo, python-dotenv,
// java.util.Properties, Spring Boot), not just our own parser. They're JS, Python and Java libraries,
// so this runs only where they're installed:
//   cd tests/loaders && npm ci && pip install -r requirements.txt
//   Spring Boot: put spring-boot, spring-core and spring-jcl jars in tests/loaders/jars (CI downloads them)
//   POENV_TEST_LOADERS=1 cargo test --test loaders
// CI sets POENV_TEST_LOADERS, which makes a missing loader a failure instead of a skip.
use poenv::envfile::{Dialect, Vars, is_lossy, write_block};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const CASES: [(&str, &str); 17] = [
    ("T_EMPTY", ""),
    ("T_SIMPLE", "abc123"),
    ("T_URL", "postgres://user:pa55@db.example.com:5432/app?sslmode=require"),
    ("T_SPACES", "hello world"),
    ("T_HASH", "abc#def"),
    ("T_DOLLAR", "pa$$word $HOME ${X}"),
    ("T_DQ", "say \"hi\""),
    ("T_SQ", "it's"),
    ("T_BOTH", "it's \"fine\""),
    ("T_NL", "line1\nline2"),
    ("T_NLQ", "it's\nmultiline"),
    ("T_BS", "C:\\path\\to"),
    ("T_EQ", "a=b=c"),
    ("T_UNI", "héllo 🌏"),
    ("T_PEM", "-----BEGIN KEY-----\nMIIB\n-----END KEY-----"),
    ("T_PAD", "  padded  "),
    ("T_ALL", "it's `all` \"three\" \\n"),
];

fn required() -> bool {
    std::env::var_os("POENV_TEST_LOADERS").is_some()
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("loaders")
}

fn write_cases(dialect: Dialect) -> PathBuf {
    write_cases_to(dialect, ".env")
}

fn write_cases_to(dialect: Dialect, file: &str) -> PathBuf {
    let bytes = poenv::util::random_bytes(6);
    let dir = std::env::temp_dir().join(format!("poenv-loaders-{}", poenv::util::hex(&bytes)));
    std::fs::create_dir_all(&dir).unwrap();
    let vars: Vars = CASES.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    std::fs::write(dir.join(file), write_block("", "t", "local", &vars, dialect).unwrap()).unwrap();
    dir
}

/// Every representable value must come back exactly; lossy ones are only flagged, not checked.
fn expect_faithful(got: &Value, dialect: Dialect) {
    for (k, v) in CASES {
        if is_lossy(v, dialect) {
            continue;
        }
        assert_eq!(got[k].as_str(), Some(v), "{dialect:?} {k}");
    }
}

/// Runs a loader; None (a skip) when it isn't installed and the test isn't required.
fn load(cmd: &str, args: &[&str]) -> Option<Value> {
    let out = match Command::new(cmd).args(args).current_dir(fixture()).output() {
        Ok(o) if o.status.success() => o,
        Ok(o) if required() => panic!("{cmd} failed: {}", String::from_utf8_lossy(&o.stderr)),
        Err(e) if required() => panic!("{cmd} isn't installed: {e}"),
        _ => {
            eprintln!("skipping: {cmd} or its loader isn't installed (see the top of tests/loaders.rs)");
            return None;
        }
    };
    Some(serde_json::from_slice(&out.stdout).unwrap())
}

fn node(dialect: &str, dir: &Path) -> Option<Value> {
    if !fixture().join("node_modules").exists() && !required() {
        eprintln!("skipping: run npm ci in tests/loaders first");
        return None;
    }
    load("node", &["read.mjs", dialect, &dir.to_string_lossy()])
}

#[test]
fn node_dotenv_reads_the_dotenv_dialect_back_exactly() {
    if let Some(got) = node("dotenv", &write_cases(Dialect::Dotenv)) {
        expect_faithful(&got, Dialect::Dotenv);
    }
}

#[test]
fn nextjs_reads_the_next_dialect_back_exactly() {
    if let Some(got) = node("next", &write_cases(Dialect::Next)) {
        expect_faithful(&got, Dialect::Next);
    }
}

#[test]
fn expo_reads_the_next_dialect_back_exactly() {
    if let Some(got) = node("expo", &write_cases(Dialect::Next)) {
        expect_faithful(&got, Dialect::Next);
    }
}

fn properties_file() -> String {
    write_cases_to(Dialect::Properties, "application.properties").join("application.properties").to_string_lossy().into_owned()
}

#[test]
fn java_properties_reads_the_properties_dialect_back_exactly() {
    if let Some(got) = load("java", &["Read.java", "java", &properties_file()]) {
        expect_faithful(&got, Dialect::Properties);
    }
}

#[test]
fn spring_boot_reads_the_properties_dialect_back_exactly() {
    if !fixture().join("jars").exists() && !required() {
        eprintln!("skipping: put the Spring Boot jars in tests/loaders/jars first");
        return;
    }
    let cp = Path::new("jars").join("*").to_string_lossy().into_owned();
    if let Some(got) = load("java", &["-cp", &cp, "Read.java", "spring", &properties_file()]) {
        expect_faithful(&got, Dialect::Properties);
    }
}

#[test]
fn python_dotenv_reads_the_python_dialect_back_exactly() {
    let python = std::env::var("POENV_TEST_PYTHON").unwrap_or_else(|_| "python3".into());
    let dir = write_cases(Dialect::Python);
    let script = "import json,sys; from dotenv import dotenv_values; print(json.dumps(dotenv_values(sys.argv[1])))";
    let path = dir.join(".env").to_string_lossy().into_owned();
    if let Some(got) = load(&python, &["-c", script, &path]) {
        expect_faithful(&got, Dialect::Python);
    }
}

#[test]
fn lossy_detection_matches_what_the_loaders_cant_do() {
    let get = |k: &str| CASES.iter().find(|(n, _)| *n == k).unwrap().1;
    assert!(is_lossy(get("T_ALL"), Dialect::Dotenv));
    assert!(is_lossy(get("T_DOLLAR"), Dialect::Python));
    assert!(!is_lossy(get("T_DOLLAR"), Dialect::Next));
    assert!(!is_lossy(get("T_BOTH"), Dialect::Dotenv));
    assert!(is_lossy(get("T_DOLLAR"), Dialect::Properties)); // Spring resolves ${X}
    assert!(!is_lossy(get("T_ALL"), Dialect::Properties));
}
