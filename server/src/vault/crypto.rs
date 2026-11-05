//! The vault's locks: keys made from a passphrase with argon2id, and sealing with XChaCha20-Poly1305.
//!
//! A sealed value is `nonce (24 bytes) ‖ ciphertext ‖ tag (16 bytes)`. Every seal takes a fresh random nonce, and an
//! "associated" text (the item's id and kind) is bound to it, so a sealed value moved to another row no longer opens.
//! Keys and opened values live in [`Zeroizing`] buffers, wiped when they are dropped.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// The length of every key, in bytes.
pub const KEY_LEN: usize = 32;

/// The length of a nonce, in bytes.
pub const NONCE_LEN: usize = 24;

/// A 32-byte key, wiped when dropped.
pub type Key = Zeroizing<[u8; KEY_LEN]>;

/// How hard argon2id works to make a key from a passphrase, and the salt it mixes in. Kept next to what it locked,
/// so a later Lantern with other defaults still opens it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KdfParams {
    /// always `argon2id`
    pub name: String,
    /// memory, in KiB
    pub m: u32,
    /// passes over the memory
    pub t: u32,
    /// lanes
    pub p: u32,
    /// the random salt, base64
    pub salt: String,
}

impl KdfParams {
    /// New parameters with a fresh random salt: 64 MiB, 3 passes, 1 lane (OWASP's argon2id advice).
    pub fn fresh() -> KdfParams {
        KdfParams { name: "argon2id".into(), m: 64 * 1024, t: 3, p: 1, salt: b64(&random::<16>()) }
    }
}

/// `N` random bytes from the operating system's generator.
pub fn random<const N: usize>() -> [u8; N] {
    use rand::Rng;
    let mut b = [0u8; N];
    rand::rng().fill(&mut b[..]);
    b
}

/// A fresh random key.
pub fn random_key() -> Key {
    Zeroizing::new(random::<KEY_LEN>())
}

/// `b` as standard base64.
pub fn b64(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(b)
}

/// The bytes of standard base64 `s`; `None` when it isn't base64.
pub fn unb64(s: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(s.trim()).ok()
}

/// The key argon2id makes from `secret` with `params`. An error in words when the parameters are not ones
/// argon2id takes (a damaged file).
pub fn derive(secret: &[u8], params: &KdfParams) -> Result<Key, String> {
    let bad = || "The key settings saved with it are damaged.".to_string();
    if params.name != "argon2id" {
        return Err(bad());
    }
    // a damaged or hostile file must not make the server use gigabytes or hours
    if params.m > 1024 * 1024 || params.t > 20 || params.p > 16 {
        return Err(bad());
    }
    let salt = unb64(&params.salt).ok_or_else(bad)?;
    let p = Params::new(params.m, params.t, params.p, Some(KEY_LEN)).map_err(|_| bad())?;
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, p).hash_password_into(secret, &salt, &mut key[..]).map_err(|_| bad())?;
    Ok(key)
}

/// `plain` sealed with `key`, bound to `aad`: nonce ‖ ciphertext ‖ tag.
pub fn seal(key: &Key, plain: &[u8], aad: &[u8]) -> Vec<u8> {
    let cipher = XChaCha20Poly1305::new(key.as_ref().into());
    let nonce = random::<NONCE_LEN>();
    // encrypting into memory only fails when the text is longer than the cipher allows (256 GiB)
    let sealed = cipher.encrypt(XNonce::from_slice(&nonce), Payload { msg: plain, aad }).unwrap_or_default();
    let mut out = Vec::with_capacity(NONCE_LEN + sealed.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    out
}

/// What [`seal`] sealed, when `key` and `aad` are the ones it was sealed with and nothing was changed; `None`
/// otherwise.
pub fn open(key: &Key, sealed: &[u8], aad: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    if sealed.len() < NONCE_LEN + 16 {
        return None;
    }
    let (nonce, body) = sealed.split_at(NONCE_LEN);
    let cipher = XChaCha20Poly1305::new(key.as_ref().into());
    cipher.decrypt(XNonce::from_slice(nonce), Payload { msg: body, aad }).ok().map(Zeroizing::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fast parameters for tests (the real ones take a fraction of a second each).
    fn quick() -> KdfParams {
        KdfParams { m: 1024, t: 1, ..KdfParams::fresh() }
    }

    #[test]
    fn sealed_values_open_only_with_their_key_and_place() {
        let key = random_key();
        let sealed = seal(&key, b"hunter2", b"item-1");
        assert!(!sealed.windows(7).any(|w| w == b"hunter2"));
        assert_eq!(open(&key, &sealed, b"item-1").as_deref().map(|v| v.as_slice()), Some(&b"hunter2"[..]));
        assert!(open(&key, &sealed, b"item-2").is_none(), "moved to another item");
        assert!(open(&random_key(), &sealed, b"item-1").is_none(), "another key");
        let mut changed = sealed.clone();
        *changed.last_mut().unwrap() ^= 1;
        assert!(open(&key, &changed, b"item-1").is_none(), "changed on disk");
        assert!(open(&key, &sealed[..10], b"item-1").is_none(), "cut short");
        assert_ne!(seal(&key, b"hunter2", b"item-1"), sealed, "each seal takes a fresh nonce");
    }

    #[test]
    fn the_same_passphrase_and_salt_make_the_same_key() {
        let p = quick();
        let a = derive(b"correct horse", &p).unwrap();
        assert_eq!(*a, *derive(b"correct horse", &p).unwrap());
        assert_ne!(*a, *derive(b"correct horsf", &p).unwrap());
        assert_ne!(*a, *derive(b"correct horse", &quick()).unwrap(), "another salt");
        let huge = KdfParams { m: u32::MAX, ..p.clone() };
        assert!(derive(b"x", &huge).is_err());
        assert!(derive(b"x", &KdfParams { name: "scrypt".into(), ..p }).is_err());
    }
}
