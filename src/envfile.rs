// Reading and writing the repo's env file. poenv owns only the managed block; every other
// line in the file is the user's and is preserved byte-for-byte.
use crate::errors::{PoenvError, Res, code};
use crate::repo::is_tracked;
use crate::util::{exists, file_mode, write_file_atomic};
use indexmap::IndexMap;
use std::fs;
use std::path::Path;

const BEGIN: &str = "# >>> poenv >>>";
const BEGIN_LINE: &str = "# >>> poenv >>> (managed: edits here are overwritten)";
const END: &str = "# <<< poenv <<<";

/// Which loader reads the file. They disagree about `$` and escapes:
/// - dotenv: Node dotenv, Docker Compose, most others. Single quotes are fully literal.
/// - next: Next.js (dotenv + dotenv-expand). Expands `$VAR` even in single quotes; `\$` escapes it.
/// - python: python-dotenv. Expands `${VAR}` even in single quotes, with no escape; double quotes
///   honour `\\` and `\"`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Dialect {
    Dotenv,
    Next,
    Python,
}

impl Dialect {
    pub fn parse(s: &str) -> Option<Dialect> {
        match s {
            "dotenv" => Some(Dialect::Dotenv),
            "next" => Some(Dialect::Next),
            "python" => Some(Dialect::Python),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Dialect::Dotenv => "dotenv",
            Dialect::Next => "next",
            Dialect::Python => "python",
        }
    }
}

pub type Vars = IndexMap<String, String>;

#[derive(Clone, Debug)]
pub struct Block {
    pub app: String,
    pub env: String,
    pub vars: Vars,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeyDiff {
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub removed: Vec<String>,
}

impl KeyDiff {
    pub fn count(&self) -> usize {
        self.added.len() + self.changed.len() + self.removed.len()
    }
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({ "added": self.added, "changed": self.changed, "removed": self.removed })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct From {
    pub app: String,
    pub env: String,
}

#[derive(Clone, Debug)]
pub struct ApplyResult {
    pub created: bool,
    /// The app/env the file was on before, if it had a block.
    pub from: Option<From>,
    /// Key names only, never values.
    pub diff: KeyDiff,
    /// Keys poenv wrote that the user also sets outside the block (theirs wins in most loaders).
    pub conflicts: Vec<String>,
    /// Keys whose values this dialect can't represent exactly; suggest `poenv run` for these.
    pub lossy: Vec<String>,
}

impl ApplyResult {
    /// The fields spread into `--json` output.
    pub fn json_fields(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut m = serde_json::Map::new();
        m.insert("created".into(), self.created.into());
        m.insert(
            "from".into(),
            match &self.from {
                Some(f) => serde_json::json!({ "app": f.app, "env": f.env }),
                None => serde_json::Value::Null,
            },
        );
        m.insert("diff".into(), self.diff.to_json());
        m.insert("conflicts".into(), self.conflicts.clone().into());
        m.insert("lossy".into(), self.lossy.clone().into());
        m
    }
}

pub fn js_ws(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

fn bare_safe(v: &str) -> bool {
    v.chars().all(|c| c.is_ascii_alphanumeric() || "_-./:@+,%=~".contains(c))
}

/// Quote a value so `dialect`'s loader reads it back unchanged (see `is_lossy` for the exceptions).
pub fn format_value(value: &str, dialect: Dialect) -> String {
    if bare_safe(value) {
        return value.to_string();
    }
    if dialect == Dialect::Python {
        if !value.contains('\'') {
            return format!("'{value}'");
        }
        return format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""));
    }
    // Node dotenv does no escape processing except \n and \r inside double quotes,
    // so prefer the quote styles it reads literally.
    let v = if dialect == Dialect::Next { value.replace('$', "\\$") } else { value.to_string() };
    if !value.contains('\'') {
        return format!("'{v}'");
    }
    if !value.contains('`') {
        return format!("`{v}`");
    }
    format!("\"{v}\"")
}

/// True when `dialect`'s loader can't read `value` back exactly, whatever the quoting.
pub fn is_lossy(value: &str, dialect: Dialect) -> bool {
    if dialect == Dialect::Python {
        return value.contains("${");
    }
    if bare_safe(value) || !value.contains('\'') || !value.contains('`') {
        return false;
    }
    value.contains('"') || value.contains("\\n") || value.contains("\\r")
}

fn split_lines(text: &str) -> Vec<String> {
    text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l).to_string()).collect()
}

fn eol(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

pub struct Parsed {
    pub block: Option<Block>,
    pub outside: Vars,
}

pub fn parse_env_file(text: &str, dialect: Dialect) -> Res<Parsed> {
    let lines = split_lines(text);
    let span = find_block(&lines)?;
    let mut outside = Vars::new();
    let collect = |from: usize, to: usize, into: &mut Vars| {
        for e in scan_entries(&lines, from, to, dialect) {
            into.insert(e.key, e.value);
        }
    };
    let Some((begin, end)) = span else {
        collect(0, lines.len(), &mut outside);
        return Ok(Parsed { block: None, outside });
    };
    collect(0, begin, &mut outside);
    collect(end + 1, lines.len(), &mut outside);
    let mut vars = Vars::new();
    collect(begin + 1, end, &mut vars);
    let header = lines[begin + 1..end].iter().find_map(|l| header(l));
    let (app, env) = header.unwrap_or_default();
    Ok(Parsed { block: Some(Block { app, env, vars }), outside })
}

/// `^# app: (\S+)\s+env: (\S+)`
fn header(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("# app: ")?;
    let app_end = rest.find(js_ws).unwrap_or(rest.len());
    if app_end == 0 {
        return None;
    }
    let app = &rest[..app_end];
    let after = &rest[app_end..];
    let trimmed = after.trim_start_matches(js_ws);
    if trimmed.len() == after.len() {
        return None;
    }
    let env_part = trimmed.strip_prefix("env: ")?;
    let env_end = env_part.find(js_ws).unwrap_or(env_part.len());
    if env_end == 0 {
        return None;
    }
    Some((app.to_string(), env_part[..env_end].to_string()))
}

/// Returns `text` with the managed block replaced (or added on top).
pub fn write_block(text: &str, app: &str, env: &str, vars: &Vars, dialect: Dialect) -> Res<String> {
    let eol = eol(text);
    let mut keys: Vec<&String> = vars.keys().collect();
    keys.sort();
    let mut block = vec![BEGIN_LINE.to_string(), format!("# app: {app}  env: {env}")];
    block.extend(keys.iter().map(|k| format!("{k}={}", format_value(&vars[*k], dialect))));
    block.push(END.to_string());
    let mut lines = split_lines(text);
    if let Some((begin, end)) = find_block(&lines)? {
        lines.splice(begin..=end, block);
        return Ok(lines.join(eol));
    }
    if text.is_empty() {
        return Ok(block.join(eol) + eol);
    }
    Ok(format!("{}{eol}{eol}{text}", block.join(eol)))
}

/// Returns `text` without the managed block (and the blank line `write_block` put after it).
pub fn remove_block(text: &str) -> Res<String> {
    let eol = eol(text);
    let mut lines = split_lines(text);
    let Some((begin, end)) = find_block(&lines)? else { return Ok(text.to_string()) };
    let blank = begin == 0 && lines.get(end + 1).is_some_and(|l| l.is_empty()) && end + 2 < lines.len();
    lines.drain(begin..=end + usize::from(blank));
    Ok(lines.join(eol))
}

/// Removes the user's own definitions of `keys` (e.g. after absorbing them into poenv).
pub fn absorb_outside(text: &str, keys: &[String]) -> Res<String> {
    let eol = eol(text);
    let lines = split_lines(text);
    let regions = match find_block(&lines)? {
        Some((begin, end)) => vec![(0, begin), (end + 1, lines.len())],
        None => vec![(0, lines.len())],
    };
    let mut drop = vec![false; lines.len()];
    for (from, to) in regions {
        for e in scan_entries(&lines, from, to, Dialect::Dotenv) {
            if keys.contains(&e.key) {
                for d in drop.iter_mut().take(e.end + 1).skip(e.start) {
                    *d = true;
                }
            }
        }
    }
    let kept: Vec<&str> = lines.iter().enumerate().filter(|(i, _)| !drop[*i]).map(|(_, l)| l.as_str()).collect();
    Ok(kept.join(eol))
}

pub fn diff_keys(before: &Vars, after: &Vars) -> KeyDiff {
    let mut added: Vec<String> = after.keys().filter(|k| !before.contains_key(*k)).cloned().collect();
    let mut removed: Vec<String> = before.keys().filter(|k| !after.contains_key(*k)).cloned().collect();
    let mut changed: Vec<String> =
        after.keys().filter(|k| before.get(*k).is_some_and(|b| b != &after[*k])).cloned().collect();
    added.sort();
    removed.sort();
    changed.sort();
    KeyDiff { added, changed, removed }
}

/// Writes `vars` into the managed block of the env file at `path`.
pub fn apply_env(path: &Path, app: &str, env: &str, vars: &Vars, dialect: Dialect) -> Res<ApplyResult> {
    let present = exists(path);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| ".".into());
    // Gitignoring a tracked file doesn't stop git: the next commit would publish the values.
    if present && is_tracked(&dir, &name) {
        return Err(PoenvError::fix(
            format!("git tracks {name}, so values written there would be committed."),
            code::ERROR,
            format!("untrack it first (git rm --cached {name}), or pick another file: poenv file .env"),
        ));
    }
    let text = if present { fs::read_to_string(path)? } else { String::new() };
    let parsed = parse_env_file(&text, dialect)?;
    let next = write_block(&text, app, env, vars, dialect)?;
    if next != text {
        write_file_atomic(path, next.as_bytes(), if present { file_mode(path) } else { 0o600 })?;
    }
    let mut keys: Vec<String> = vars.keys().cloned().collect();
    keys.sort();
    Ok(ApplyResult {
        created: !present,
        from: parsed.block.as_ref().map(|b| From { app: b.app.clone(), env: b.env.clone() }),
        diff: diff_keys(parsed.block.as_ref().map(|b| &b.vars).unwrap_or(&Vars::new()), vars),
        conflicts: keys.iter().filter(|k| parsed.outside.contains_key(*k)).cloned().collect(),
        lossy: keys.iter().filter(|k| is_lossy(&vars[*k], dialect)).cloned().collect(),
    })
}

fn find_block(lines: &[String]) -> Res<Option<(usize, usize)>> {
    let Some(begin) = lines.iter().position(|l| l.starts_with(BEGIN)) else { return Ok(None) };
    match lines.iter().enumerate().skip(begin + 1).find(|(_, l)| l.starts_with(END)) {
        Some((end, _)) => Ok(Some((begin, end))),
        None => Err(PoenvError::fix(
            format!("The env file has a \"{BEGIN}\" line but no \"{END}\" line."),
            code::ERROR,
            format!("add \"{END}\" after the poenv lines, or delete the poenv block and run poenv use"),
        )),
    }
}

struct Entry {
    key: String,
    value: String,
    start: usize,
    end: usize,
}

/// `^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_.-]*)\s*=\s*(.*)$` → (key, rest)
fn match_var(line: &str) -> Option<(String, String)> {
    let s = line.trim_start_matches(js_ws);
    if let Some(after) = s.strip_prefix("export") {
        let rest = after.trim_start_matches(js_ws);
        if rest.len() < after.len() {
            if let Some(m) = key_eq(rest) {
                return Some(m);
            }
        }
    }
    key_eq(s)
}

fn key_eq(s: &str) -> Option<(String, String)> {
    let mut chars = s.char_indices();
    match chars.next() {
        Some((_, c)) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return None,
    }
    let end = s
        .char_indices()
        .skip(1)
        .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '_' || *c == '.' || *c == '-'))
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    let key = &s[..end];
    let after = s[end..].trim_start_matches(js_ws);
    let rest = after.strip_prefix('=')?;
    Some((key.to_string(), rest.trim_start_matches(js_ws).to_string()))
}

/// KEY=value entries in lines[from, to), with the line span each one covers.
fn scan_entries(lines: &[String], from: usize, to: usize, dialect: Dialect) -> Vec<Entry> {
    let mut entries = vec![];
    let mut i = from;
    while i < to {
        let Some((key, rest)) = match_var(&lines[i]) else {
            i += 1;
            continue;
        };
        let quote = rest.chars().next();
        if let Some(q @ ('\'' | '"' | '`')) = quote {
            let mut buf = rest[1..].to_string();
            let mut j = i;
            let mut close = closing_quote(&buf, q);
            while close.is_none() && j + 1 < to {
                j += 1;
                buf.push('\n');
                buf.push_str(&lines[j]);
                close = closing_quote(&buf, q);
            }
            if let Some(c) = close {
                entries.push(Entry { key, value: decode(&buf[..c], Some(q), dialect), start: i, end: j });
                i = j + 1;
                continue;
            }
            // unterminated quote: read the line as a bare value
        }
        let bare = if rest.starts_with('#') { String::new() } else { strip_comment(&rest).trim_matches(js_ws).to_string() };
        entries.push(Entry { key, value: decode(&bare, None, dialect), start: i, end: i });
        i += 1;
    }
    entries
}

/// `rest.replace(/\s+#.*$/, "")`: cut at the first whitespace run that's followed by `#`.
fn strip_comment(rest: &str) -> &str {
    let idx: Vec<(usize, char)> = rest.char_indices().collect();
    let mut k = 0;
    while k < idx.len() {
        if js_ws(idx[k].1) {
            let mut m = k;
            while m < idx.len() && js_ws(idx[m].1) {
                m += 1;
            }
            if m < idx.len() && idx[m].1 == '#' {
                return &rest[..idx[k].0];
            }
            k = m;
        } else {
            k += 1;
        }
    }
    rest
}

fn closing_quote(s: &str, quote: char) -> Option<usize> {
    let mut it = s.char_indices();
    while let Some((i, c)) = it.next() {
        if quote == '"' && c == '\\' {
            it.next();
            continue;
        }
        if c == quote {
            return Some(i);
        }
    }
    None
}

/// Undo quoting the way `dialect`'s loader does. `quote` is None for bare values.
fn decode(raw: &str, quote: Option<char>, dialect: Dialect) -> String {
    let mut v = raw.to_string();
    if quote == Some('"') {
        if dialect == Dialect::Python {
            let mut out = String::new();
            let mut cs = v.chars().peekable();
            while let Some(c) = cs.next() {
                if c == '\\' {
                    if let Some(&n) = cs.peek() {
                        let mapped = match n {
                            'n' => Some('\n'),
                            'r' => Some('\r'),
                            't' => Some('\t'),
                            '"' => Some('"'),
                            '\\' => Some('\\'),
                            _ => None,
                        };
                        if let Some(m) = mapped {
                            out.push(m);
                            cs.next();
                            continue;
                        }
                    }
                }
                out.push(c);
            }
            v = out;
        } else {
            v = v.replace("\\n", "\n").replace("\\r", "\r");
        }
    }
    if dialect == Dialect::Next {
        v = v.replace("\\$", "$");
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> Vars {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn round_trips_every_dialect() {
        let values = ["plain", "with space", "p$$w $HOME", "a'b", "a'b`c", "multi\nline", "#hash", "x=y", "q\"uote"];
        for d in [Dialect::Dotenv, Dialect::Next, Dialect::Python] {
            for v in values {
                if is_lossy(v, d) {
                    continue;
                }
                let text = write_block("", "app", "local", &vars(&[("K", v)]), d).unwrap();
                let parsed = parse_env_file(&text, d).unwrap();
                assert_eq!(parsed.block.unwrap().vars["K"], v, "{d:?} {v:?}");
            }
        }
    }

    #[test]
    fn keeps_user_lines_and_crlf() {
        let text = "A=1\r\nB=2 # note\r\n";
        let next = write_block(text, "app", "dev", &vars(&[("C", "3")]), Dialect::Dotenv).unwrap();
        assert!(next.starts_with("# >>> poenv >>>"));
        assert!(next.ends_with("\r\nA=1\r\nB=2 # note\r\n"));
        let p = parse_env_file(&next, Dialect::Dotenv).unwrap();
        assert_eq!(p.outside["B"], "2");
        let b = p.block.unwrap();
        assert_eq!((b.app.as_str(), b.env.as_str()), ("app", "dev"));
        assert_eq!(remove_block(&next).unwrap(), text);
    }

    #[test]
    fn parses_export_and_multiline() {
        let p = parse_env_file("export A=1\nB=\"x\ny\"\nexport = 2\n", Dialect::Dotenv).unwrap();
        assert_eq!(p.outside["A"], "1");
        assert_eq!(p.outside["B"], "x\ny");
        assert_eq!(p.outside["export"], "2");
    }

    #[test]
    fn unterminated_block_is_an_error() {
        assert!(parse_env_file("# >>> poenv >>>\nA=1\n", Dialect::Dotenv).is_err());
    }

    const BEGIN_L: &str = "# >>> poenv >>> (managed: edits here are overwritten)";

    fn round_trip(value: &str) -> Option<String> {
        parse_env_file(&format!("K={}\n", format_value(value, Dialect::Dotenv)), Dialect::Dotenv).unwrap().outside.get("K").cloned()
    }

    #[test]
    fn format_and_parse_round_trip() {
        let cases = [
            "", "abc123", "postgres://user:pa55@db.example.com:5432/app?sslmode=require", "hello world", "abc#def",
            "pa$$word $HOME ${X}", "say \"hi\"", "it's", "it's \"fine\"", "line1\nline2", "it's\nmultiline",
            "C:\\path\\to", "a=b=c", "héllo 🌏", "-----BEGIN KEY-----\nMIIB\n-----END KEY-----", "  padded  ",
        ];
        for v in cases {
            assert_eq!(round_trip(v).as_deref(), Some(v), "{v:?}");
        }
        assert_eq!(format_value("abc-123_x.y/z:1@h+2,3", Dialect::Dotenv), "abc-123_x.y/z:1@h+2,3");
        assert_eq!(format_value("pa$$word", Dialect::Dotenv), "'pa$$word'");
    }

    #[test]
    fn reads_the_common_dotenv_forms() {
        let text = [
            "# comment", "", "A=1", "export B=two", "C = spaced ", "D=value # inline comment",
            "E='single # not a comment'", "F=\"double\\nescaped\"", "G=`back'tick\"`", "H=\"multi", "line\"", "I=",
            "not a var line",
        ]
        .join("\n");
        let got = parse_env_file(&text, Dialect::Dotenv).unwrap().outside;
        let want = vars(&[
            ("A", "1"), ("B", "two"), ("C", "spaced"), ("D", "value"), ("E", "single # not a comment"),
            ("F", "double\nescaped"), ("G", "back'tick\""), ("H", "multi\nline"), ("I", ""),
        ]);
        assert_eq!(got, want);
    }

    #[test]
    fn separates_the_block_from_the_users_keys() {
        let text = [BEGIN_L, "# app: my-app  env: staging", "A=1", END, "", "DEBUG=true"].join("\n");
        let p = parse_env_file(&text, Dialect::Dotenv).unwrap();
        let b = p.block.unwrap();
        assert_eq!((b.app.as_str(), b.env.as_str(), b.vars), ("my-app", "staging", vars(&[("A", "1")])));
        assert_eq!(p.outside, vars(&[("DEBUG", "true")]));
        let e = parse_env_file(&format!("{BEGIN_L}\nA=1\n"), Dialect::Dotenv).err().unwrap();
        assert!(e.problem.contains("<<< poenv <<<"));
    }

    #[test]
    fn write_block_cases() {
        let empty = write_block("", "my-app", "local", &vars(&[("B", "2"), ("A", "1")]), Dialect::Dotenv).unwrap();
        assert_eq!(empty, [BEGIN_L, "# app: my-app  env: local", "A=1", "B=2", END, ""].join("\n"));

        let user = "# my stuff\nDEBUG=true\n\nPORT=3000\n";
        let out = write_block(user, "my-app", "local", &vars(&[("A", "1")]), Dialect::Dotenv).unwrap();
        assert!(out.ends_with(user) && out.starts_with(BEGIN_L));
        assert!(out.contains(&format!("{END}\n\n# my stuff")));

        let before = format!("# top note\n{}\nDEBUG=true\n", write_block("", "my-app", "local", &vars(&[("A", "1")]), Dialect::Dotenv).unwrap());
        let out = write_block(&before, "my-app", "staging", &vars(&[("A", "2"), ("B", "3")]), Dialect::Dotenv).unwrap();
        assert!(out.starts_with("# top note\n") && out.ends_with("\nDEBUG=true\n"));
        let b = parse_env_file(&out, Dialect::Dotenv).unwrap().block.unwrap();
        assert_eq!((b.env.as_str(), b.vars), ("staging", vars(&[("A", "2"), ("B", "3")])));

        let once = write_block("X=1\n", "my-app", "local", &vars(&[("A", "a b")]), Dialect::Dotenv).unwrap();
        assert_eq!(write_block(&once, "my-app", "local", &vars(&[("A", "a b")]), Dialect::Dotenv).unwrap(), once);

        let crlf = write_block("DEBUG=true\r\n", "my-app", "local", &vars(&[("A", "1")]), Dialect::Dotenv).unwrap();
        assert!(crlf.contains(&format!("{END}\r\n")));
        assert!(!crlf.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn diff_and_absorb() {
        let d = diff_keys(&vars(&[("A", "1"), ("B", "2"), ("C", "3")]), &vars(&[("B", "2"), ("C", "x"), ("D", "4"), ("A0", "0")]));
        assert_eq!(d, KeyDiff { added: vec!["A0".into(), "D".into()], changed: vec!["C".into()], removed: vec!["A".into()] });
        let text = "KEEP=1\nTAKE=\"multi\nline\"\n# note\nALSO=2\n";
        assert_eq!(absorb_outside(text, &["TAKE".into(), "ALSO".into()]).unwrap(), "KEEP=1\n# note\n");
    }

    fn temp() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("poenv-unit-{}", crate::util::hex(&crate::util::random_bytes(6))));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn apply_env_creates_swaps_and_never_reports_values() {
        let dir = temp();
        let path = dir.join(".env.local");
        let r = apply_env(&path, "my-app", "local", &vars(&[("A", "1"), ("B", "2")]), Dialect::Dotenv).unwrap();
        assert!(r.created && r.from.is_none());
        assert_eq!(r.diff.added, ["A", "B"]);
        assert!(r.conflicts.is_empty() && r.lossy.is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let names: Vec<_> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, [".env.local"]); // no temp files left behind

        let path = temp().join(".env");
        fs::write(&path, "DEBUG=true\nDB=local-override\n").unwrap();
        apply_env(&path, "my-app", "local", &vars(&[("DB", "local"), ("A", "1")]), Dialect::Dotenv).unwrap();
        let r = apply_env(&path, "my-app", "staging", &vars(&[("DB", "staging"), ("B", "2")]), Dialect::Dotenv).unwrap();
        assert!(!r.created);
        assert_eq!(r.from, Some(From { app: "my-app".into(), env: "local".into() }));
        assert_eq!(r.diff, KeyDiff { added: vec!["B".into()], changed: vec!["DB".into()], removed: vec!["A".into()] });
        assert_eq!(r.conflicts, ["DB"]);
        assert!(fs::read_to_string(&path).unwrap().ends_with("DEBUG=true\nDB=local-override\n"));

        let r = apply_env(&temp().join(".env"), "my-app", "local", &vars(&[("SECRET", "sk-very-secret")]), Dialect::Dotenv).unwrap();
        let json = serde_json::Value::Object(r.json_fields()).to_string();
        assert!(!json.contains("sk-very-secret"));
    }

    #[test]
    fn lossy_detection() {
        let all = "it's `all` \"three\" \\n";
        assert!(is_lossy(all, Dialect::Dotenv));
        assert!(is_lossy("pa$$word $HOME ${X}", Dialect::Python));
        assert!(!is_lossy("pa$$word $HOME ${X}", Dialect::Next));
        assert!(!is_lossy("it's \"fine\"", Dialect::Dotenv));
    }
}
