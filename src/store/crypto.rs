// Whole-store encryption: AES-256-GCM over the entire JSON blob.
// Layout: "POENV" | version(1) | iv(12) | ciphertext | tag(16). The header is authenticated as AAD.
use crate::errors::{PoenvError, Res, code};
use crate::util::random_bytes;
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};

const MAGIC: &[u8] = b"POENV";
const FORMAT_VERSION: u8 = 1;
const HEADER: &[u8] = b"POENV\x01";
const IV_LEN: usize = 12;
const TAG_LEN: usize = 16;

pub fn encrypt(key: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let cipher = Aes256Gcm::new_from_slice(key).expect("32-byte key");
    let iv = random_bytes(IV_LEN);
    let nonce = Nonce::try_from(&iv[..]).expect("12-byte iv");
    let body = cipher.encrypt(&nonce, Payload { msg: plaintext, aad: HEADER }).expect("encrypt");
    let mut out = HEADER.to_vec();
    out.extend_from_slice(&iv);
    out.extend_from_slice(&body);
    out
}

pub fn decrypt(key: &[u8], blob: &[u8]) -> Res<Vec<u8>> {
    if blob.len() < HEADER.len() + IV_LEN + TAG_LEN || &blob[..MAGIC.len()] != MAGIC {
        return Err(PoenvError::new("This file is not a poenv store."));
    }
    let version = blob[MAGIC.len()];
    if version != FORMAT_VERSION {
        return Err(PoenvError::fix(
            format!("This store was written by a newer poenv (format {version})."),
            code::ERROR,
            "upgrade poenv",
        ));
    }
    let wrong = || PoenvError::new("Could not decrypt the store: wrong key, or the file is corrupted.");
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| wrong())?;
    let iv = &blob[HEADER.len()..HEADER.len() + IV_LEN];
    let nonce = Nonce::try_from(iv).map_err(|_| wrong())?;
    cipher.decrypt(&nonce, Payload { msg: &blob[HEADER.len() + IV_LEN..], aad: HEADER }).map_err(|_| wrong())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_detects_tampering() {
        let key = random_bytes(32);
        let blob = encrypt(&key, b"hello");
        assert_eq!(decrypt(&key, &blob).unwrap(), b"hello");
        let mut bad = blob.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert!(decrypt(&key, &bad).unwrap_err().problem.contains("Could not decrypt"));
        assert!(decrypt(&random_bytes(32), &blob).is_err());
        assert!(decrypt(&key, b"nope").unwrap_err().problem.contains("not a poenv store"));
        let mut newer = blob.clone();
        newer[5] = 2;
        assert!(decrypt(&key, &newer).unwrap_err().problem.contains("newer poenv"));
    }
}
