// Masks env values in the output of `poenv run` when an agent or script reads it (SPEC §5).
use crate::util::js_len;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use std::io::{Read, Write};

pub const MASK: &str = "***";

/// Shorter values are left alone: masking every "true" or "3000" would garble output and hide little.
pub const MIN_MASK_LENGTH: usize = 6;

/// The strings to hide for these values: each value as-is, base64, URL-encoded and JSON-escaped.
pub fn mask_patterns<'a>(values: impl IntoIterator<Item = &'a String>) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut add = |p: String| {
        if !out.contains(&p) {
            out.push(p);
        }
    };
    for value in values {
        if js_len(value) < MIN_MASK_LENGTH {
            continue;
        }
        add(value.clone());
        add(STANDARD.encode(value.as_bytes()));
        add(URL_SAFE_NO_PAD.encode(value.as_bytes()));
        add(encode_uri_component(value));
        let json = serde_json::to_string(value).unwrap_or_default();
        add(json[1..json.len() - 1].to_string());
    }
    // Longest first, so a value that contains another is replaced whole.
    out.retain(|p| js_len(p) >= MIN_MASK_LENGTH);
    out.sort_by_key(|p| std::cmp::Reverse(js_len(p)));
    out
}

/// JavaScript's encodeURIComponent.
pub fn encode_uri_component(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A streaming replacer. Output arrives in chunks that can split a value, so it holds back only the
/// end of the text that could still be the start of one, and lets everything else through at once.
pub struct Masker {
    patterns: Vec<String>,
    held: String,
    bytes: Vec<u8>,
}

impl Masker {
    pub fn new(patterns: Vec<String>) -> Self {
        Masker { patterns, held: String::new(), bytes: vec![] }
    }

    fn replace(&self, text: &str) -> String {
        self.patterns.iter().fold(text.to_string(), |t, p| t.replace(p.as_str(), MASK))
    }

    /// Length in bytes of the longest suffix of `text` that is a prefix of some pattern.
    fn pending(&self, text: &str) -> usize {
        let longest = self.patterns.first().map(|p| p.chars().count()).unwrap_or(0);
        let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
        let max = longest.saturating_sub(1).min(starts.len());
        for k in (1..=max).rev() {
            let tail = &text[starts[starts.len() - k]..];
            if self.patterns.iter().any(|p| p.starts_with(tail)) {
                return tail.len();
            }
        }
        0
    }

    /// Decodes UTF-8 like a streaming TextDecoder: incomplete sequences wait, invalid ones become U+FFFD.
    fn decode(&mut self, chunk: &[u8], last: bool) -> String {
        self.bytes.extend_from_slice(chunk);
        let mut out = String::new();
        loop {
            match std::str::from_utf8(&self.bytes) {
                Ok(s) => {
                    out.push_str(s);
                    self.bytes.clear();
                    return out;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    out.push_str(std::str::from_utf8(&self.bytes[..valid]).unwrap());
                    match e.error_len() {
                        Some(n) => {
                            out.push('\u{fffd}');
                            self.bytes.drain(..valid + n);
                        }
                        None => {
                            self.bytes.drain(..valid);
                            if last {
                                out.push('\u{fffd}');
                                self.bytes.clear();
                            }
                            return out;
                        }
                    }
                }
            }
        }
    }

    /// Masked text that is safe to print now.
    pub fn push(&mut self, chunk: &[u8]) -> String {
        let decoded = self.decode(chunk, false);
        let joined = std::mem::take(&mut self.held) + &decoded;
        let text = self.replace(&joined);
        let keep = self.pending(&text);
        let cut = text.len() - keep;
        self.held = text[cut..].to_string();
        text[..cut].to_string()
    }

    /// Whatever was held back, once the stream has ended.
    pub fn end(&mut self) -> String {
        let decoded = self.decode(&[], true);
        let joined = std::mem::take(&mut self.held) + &decoded;
        self.replace(&joined)
    }
}

/// Copies `input` to `out`, masking `patterns`.
pub fn pipe_masked(mut input: impl Read, mut out: impl Write, patterns: Vec<String>) {
    let mut masker = Masker::new(patterns);
    let mut buf = [0u8; 8192];
    loop {
        match input.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let text = masker.push(&buf[..n]);
                if !text.is_empty() {
                    let _ = out.write_all(text.as_bytes());
                    let _ = out.flush();
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    let rest = masker.end();
    if !rest.is_empty() {
        let _ = out.write_all(rest.as_bytes());
        let _ = out.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_across_chunks() {
        let v = "sk-secret-123".to_string();
        let mut m = Masker::new(mask_patterns([&v]));
        let mut out = m.push(b"key=sk-sec");
        out += &m.push(b"ret-123 done");
        out += &m.end();
        assert_eq!(out, "key=*** done");
    }

    #[test]
    fn encodings_and_short_values() {
        let v = "a b/c?d".to_string();
        let p = mask_patterns([&v, &"short".to_string()]);
        assert!(p.contains(&"a%20b%2Fc%3Fd".to_string()));
        assert!(!p.contains(&"short".to_string()));
    }

    fn stream(patterns: &[String], text: &str, size: usize) -> String {
        let mut m = Masker::new(patterns.to_vec());
        let mut out = String::new();
        for chunk in text.as_bytes().chunks(size) {
            out += &m.push(chunk);
        }
        out + &m.end()
    }

    fn pats(values: &[&str]) -> Vec<String> {
        let owned: Vec<String> = values.iter().map(|v| v.to_string()).collect();
        mask_patterns(owned.iter())
    }

    #[test]
    fn patterns() {
        let p = pats(&["sk-secret-123", "true", "3000"]);
        assert!(p.contains(&"sk-secret-123".to_string()));
        assert!(p.contains(&STANDARD.encode("sk-secret-123")));
        assert!(!p.contains(&"true".to_string()) && !p.contains(&"3000".to_string()));
        let p = pats(&["pa ss\"word\n"]);
        assert!(p.contains(&"pa%20ss%22word%0A".to_string()));
        assert!(p.contains(&"pa ss\\\"word\\n".to_string()));
        let p = pats(&["abcdef", "abcdefghij"]);
        let pos = |s: &str| p.iter().position(|x| x == s).unwrap();
        assert!(pos("abcdefghij") < pos("abcdef"));
    }

    #[test]
    fn streams() {
        let p = pats(&["hunter2-secret", "postgres://u:p@db/x"]);
        let text = "db=postgres://u:p@db/x\npw=hunter2-secret done\n";
        let masked = "db=***\npw=*** done\n";
        for size in 1..=text.len() {
            assert_eq!(stream(&p, text, size), masked, "chunk size {size}");
        }
        let b64 = STANDARD.encode("hunter2-secret");
        assert_eq!(stream(&p, &format!("token {b64}\n"), 3), "token ***\n");
        assert_eq!(stream(&p, "héllo → wörld\n", 1), "héllo → wörld\n");
        assert_eq!(stream(&[], text, 5), text);

        let mut m = Masker::new(p.clone());
        assert_eq!(m.push(b"Password? "), "Password? ");
        assert_eq!(m.push(b"ok hunt"), "ok ");
        assert_eq!(m.push(b"ing\n"), "hunting\n");
    }
}
