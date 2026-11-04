// The master key is derived from a 24-word BIP39 phrase, so the phrase alone can rebuild it.
use crate::errors::{PoenvError, Res};
use crate::util::random_bytes;
use bip39::{Language, Mnemonic};
use hkdf::Hkdf;
use sha2::Sha256;

const WORDS: usize = 24;

fn derive_key(entropy: &[u8]) -> Vec<u8> {
    let hk = Hkdf::<Sha256>::new(Some(b"poenv"), entropy);
    let mut okm = vec![0u8; 32];
    hk.expand(b"poenv master key v1", &mut okm).expect("32 bytes");
    okm
}

pub fn generate_recovery() -> (String, Vec<u8>) {
    let entropy = random_bytes(32);
    let phrase = Mnemonic::from_entropy(&entropy).expect("32 bytes").to_string();
    (phrase, derive_key(&entropy))
}

/// The master key for a phrase: 24 words, in order. Errors say what's wrong without echoing words.
pub fn key_from_phrase(input: &str) -> Res<Vec<u8>> {
    let words = tokens(input);
    let phrase = words.join(" ");
    match Mnemonic::parse_in_normalized(Language::English, &phrase) {
        Ok(m) => Ok(derive_key(&m.to_entropy())),
        Err(_) => Err(PoenvError::new(explain(&words))),
    }
}

fn tokens(input: &str) -> Vec<String> {
    input
        .to_lowercase()
        .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect()
}

fn explain(words: &[String]) -> String {
    if words.len() != WORDS {
        return format!("A recovery phrase has {WORDS} words; that had {}.", words.len());
    }
    let list = Language::English.word_list();
    let unknown: Vec<String> = words
        .iter()
        .enumerate()
        .filter(|(_, w)| !list.contains(&w.as_str()))
        .map(|(i, _)| (i + 1).to_string())
        .collect();
    if !unknown.is_empty() {
        let many = unknown.len() > 1;
        return format!(
            "Word{} {} {} (check the spelling).",
            if many { "s" } else { "" },
            unknown.join(", "),
            if many { "aren't recovery words" } else { "isn't a recovery word" }
        );
    }
    "All 24 words are valid recovery words, but not in this order. Check the order (or one swapped word).".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrase_rebuilds_the_key() {
        let (phrase, key) = generate_recovery();
        assert_eq!(phrase.split(' ').count(), 24);
        assert_eq!(key_from_phrase(&phrase).unwrap(), key);
        assert_eq!(key_from_phrase(&phrase.to_uppercase().replace(' ', ",\n")).unwrap(), key);
    }

    #[test]
    fn explains_bad_phrases() {
        let (phrase, _) = generate_recovery();
        let words: Vec<&str> = phrase.split(' ').collect();
        assert!(key_from_phrase(&words[..23].join(" ")).unwrap_err().problem.contains("that had 23"));
        let mut typo = words.clone();
        typo[2] = "zzzz";
        assert!(key_from_phrase(&typo.join(" ")).unwrap_err().problem.contains("Word 3 isn't"));
        let mut swapped = words.clone();
        swapped.swap(0, 1);
        if swapped != words {
            let e = key_from_phrase(&swapped.join(" "));
            if let Err(e) = e {
                assert!(e.problem.contains("not in this order"));
            }
        }
    }
}
