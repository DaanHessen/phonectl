//! The host's ADB identity: an RSA-2048 key.
//!
//! Over USB, adbd sends a 20-byte token in `AUTH(TOKEN)`; the host signs it
//! (`AUTH(SIGNATURE)`) or, if the phone does not know the key yet, offers the
//! public key (`AUTH(RSAPUBLICKEY)`) so the user can accept it. Over wireless
//! debugging the same key goes into the TLS client certificate instead.
//!
//! The private key is stored as PKCS#8 PEM, the same format as
//! `~/.android/adbkey`, with mode 0600.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use base64::Engine;
use rsa::pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding};
use rsa::traits::PublicKeyParts;
use rsa::{BigUint, Pkcs1v15Sign, RsaPrivateKey, RsaPublicKey};

use crate::error::{Error, Result};

pub const AUTH_TOKEN: u32 = 1;
pub const AUTH_SIGNATURE: u32 = 2;
pub const AUTH_RSAPUBLICKEY: u32 = 3;

pub const TOKEN_LEN: usize = 20;
const KEY_BITS: usize = 2048;
const KEY_WORDS: usize = KEY_BITS / 32;

pub struct HostKey {
    private: RsaPrivateKey,
}

impl std::fmt::Debug for HostKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostKey").field("bits", &KEY_BITS).finish_non_exhaustive()
    }
}

impl HostKey {
    pub fn generate() -> Result<Self> {
        let private = RsaPrivateKey::new(&mut rand::rngs::OsRng, KEY_BITS)
            .map_err(|e| Error::Key(e.to_string()))?;
        Ok(Self { private })
    }

    pub fn from_pem(pem: &str) -> Result<Self> {
        let private = RsaPrivateKey::from_pkcs8_pem(pem).map_err(|e| Error::Key(e.to_string()))?;
        if private.size() * 8 != KEY_BITS {
            return Err(Error::Key(format!("expected a {KEY_BITS}-bit key, got {}", private.size() * 8)));
        }
        Ok(Self { private })
    }

    pub fn to_pem(&self) -> Result<String> {
        Ok(self
            .private
            .to_pkcs8_pem(LineEnding::LF)
            .map_err(|e| Error::Key(e.to_string()))?
            .to_string())
    }

    /// Loads the key at `path`, creating it (mode 0600, parent dirs 0700) if
    /// it does not exist. Refuses a key file that others can read.
    pub fn load_or_generate(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(pem) => {
                let mode = fs::metadata(path)?.permissions().mode();
                if mode & 0o077 != 0 {
                    return Err(Error::Key(format!(
                        "{} is readable by other users (mode {:o}); run chmod 600 on it",
                        path.display(),
                        mode & 0o777
                    )));
                }
                Self::from_pem(&pem)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let key = Self::generate()?;
                if let Some(dir) = path.parent() {
                    fs::create_dir_all(dir)?;
                    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
                }
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(path)?;
                file.write_all(key.to_pem()?.as_bytes())?;
                Ok(key)
            }
            Err(e) => Err(e.into()),
        }
    }

    pub fn private_key(&self) -> &RsaPrivateKey {
        &self.private
    }

    /// Signs an `AUTH(TOKEN)` challenge. adbd treats the token as an already
    /// computed SHA-1 digest and verifies a PKCS#1 v1.5 signature over it.
    pub fn sign_token(&self, token: &[u8]) -> Result<Vec<u8>> {
        if token.len() != TOKEN_LEN {
            return Err(Error::Key(format!("auth token is {} bytes, expected {TOKEN_LEN}", token.len())));
        }
        self.private
            .sign(Pkcs1v15Sign::new::<sha1::Sha1>(), token)
            .map_err(|e| Error::Key(e.to_string()))
    }

    /// The `AUTH(RSAPUBLICKEY)` payload: base64 of Android's binary public key,
    /// a space, a comment naming this host, and a NUL.
    pub fn public_key_payload(&self, comment: &str) -> Vec<u8> {
        let b64 = base64::engine::general_purpose::STANDARD.encode(android_public_key(&self.private.to_public_key()));
        let mut out = format!("{b64} {comment}").into_bytes();
        out.push(0);
        out
    }
}

/// Android's `RSAPublicKey` struct (system/core/libcrypto_utils), all words
/// little-endian: `len` (words in the modulus), `n0inv` (-1/n mod 2^32),
/// `n[len]`, `rr[len]` (R^2 mod n with R = 2^(32·len)), `exponent`.
pub fn android_public_key(key: &RsaPublicKey) -> Vec<u8> {
    let n = key.n();
    let n_words = to_words(n);
    let n0inv = n0inv(n_words[0]);
    let rr = (BigUint::from(1u8) << (2 * KEY_BITS)) % n;
    let exponent = to_words(key.e())[0];

    let mut out = Vec::with_capacity(4 * (3 + 2 * KEY_WORDS));
    out.extend_from_slice(&(KEY_WORDS as u32).to_le_bytes());
    out.extend_from_slice(&n0inv.to_le_bytes());
    for w in n_words {
        out.extend_from_slice(&w.to_le_bytes());
    }
    for w in to_words(&rr) {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out.extend_from_slice(&exponent.to_le_bytes());
    out
}

fn to_words(v: &BigUint) -> [u32; KEY_WORDS] {
    let bytes = v.to_bytes_le();
    let mut words = [0u32; KEY_WORDS];
    for (i, chunk) in bytes.chunks(4).enumerate().take(KEY_WORDS) {
        let mut w = [0u8; 4];
        w[..chunk.len()].copy_from_slice(chunk);
        words[i] = u32::from_le_bytes(w);
    }
    words
}

/// -(n0^-1) mod 2^32 for odd n0, by Newton iteration (each step doubles the
/// number of correct low bits: 1 → 2 → 4 → … → 32 after five steps).
fn n0inv(n0: u32) -> u32 {
    let mut inv: u32 = 1;
    for _ in 0..5 {
        inv = inv.wrapping_mul(2u32.wrapping_sub(n0.wrapping_mul(inv)));
    }
    inv.wrapping_neg()
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use rsa::Pkcs1v15Sign;

    use super::*;

    fn key() -> &'static HostKey {
        static KEY: OnceLock<HostKey> = OnceLock::new();
        KEY.get_or_init(|| HostKey::generate().unwrap())
    }

    fn word(bytes: &[u8], index: usize) -> u32 {
        u32::from_le_bytes(bytes[index * 4..index * 4 + 4].try_into().unwrap())
    }

    #[test]
    fn n0inv_is_the_negated_inverse() {
        for n0 in [1u32, 3, 0xffff_ffff, 0x1234_5679, 0x8000_0001] {
            assert_eq!(n0.wrapping_mul(n0inv(n0)), u32::MAX, "n0 = {n0:#x}");
        }
    }

    #[test]
    fn android_public_key_layout() {
        let public = key().private_key().to_public_key();
        let raw = android_public_key(&public);
        assert_eq!(raw.len(), 524);
        assert_eq!(word(&raw, 0), 64);
        assert_eq!(word(&raw, 1).wrapping_mul(word(&raw, 2)), u32::MAX);
        assert_eq!(word(&raw, 130), 65537);

        let n = BigUint::from_bytes_le(&raw[8..8 + 256]);
        assert_eq!(&n, public.n());
        let rr = BigUint::from_bytes_le(&raw[264..264 + 256]);
        assert_eq!(rr, (BigUint::from(1u8) << 4096) % public.n());
    }

    #[test]
    fn public_key_payload_is_base64_comment_nul() {
        let payload = key().public_key_payload("me@laptop");
        assert_eq!(payload.last(), Some(&0));
        let text = std::str::from_utf8(&payload[..payload.len() - 1]).unwrap();
        let (b64, comment) = text.split_once(' ').unwrap();
        assert_eq!(comment, "me@laptop");
        let raw = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
        assert_eq!(raw.len(), 524);
    }

    #[test]
    fn token_signature_verifies_as_prehashed_sha1() {
        let token = [7u8; TOKEN_LEN];
        let sig = key().sign_token(&token).unwrap();
        assert_eq!(sig.len(), 256);
        key()
            .private_key()
            .to_public_key()
            .verify(Pkcs1v15Sign::new::<sha1::Sha1>(), &token, &sig)
            .unwrap();
    }

    #[test]
    fn sign_rejects_wrong_token_length() {
        assert!(key().sign_token(&[0; 19]).is_err());
    }

    #[test]
    fn pem_round_trip() {
        let pem = key().to_pem().unwrap();
        assert!(pem.starts_with("-----BEGIN PRIVATE KEY-----"));
        let back = HostKey::from_pem(&pem).unwrap();
        assert_eq!(back.private_key(), key().private_key());
    }

    #[test]
    fn load_or_generate_creates_private_file_and_reloads_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keys/adbkey");
        let first = HostKey::load_or_generate(&path).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        let second = HostKey::load_or_generate(&path).unwrap();
        assert_eq!(first.private_key(), second.private_key());
    }

    #[test]
    fn load_refuses_world_readable_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("adbkey");
        fs::write(&path, key().to_pem().unwrap()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let err = HostKey::load_or_generate(&path).unwrap_err().to_string();
        assert!(err.contains("chmod 600"), "{err}");
    }
}
