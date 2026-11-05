//! Two-factor codes (TOTP, RFC 6238) from a saved 2FA seed, so Lantern can sign in to a site that asks for one and
//! the owner can read the code next to the password.
//!
//! A seed is the base32 text a site shows under its QR code (spaces and dashes allowed), or an `otpauth://` link.
//! Codes are 6 digits, 30 seconds, HMAC-SHA1, unless the link says otherwise.

use hmac::{Hmac, Mac};

/// A seed read into what makes its codes.
#[derive(Debug, Clone, PartialEq)]
pub struct Seed {
    /// the secret bytes
    pub key: Vec<u8>,
    /// how many digits a code has (6 to 8)
    pub digits: u32,
    /// how many seconds a code lasts
    pub period: u64,
}

/// The seed in `text` (base32 or an `otpauth://totp/...?secret=...` link); `None` when it is neither.
pub fn parse(text: &str) -> Option<Seed> {
    let text = text.trim();
    let (mut secret, mut digits, mut period) = (text.to_string(), 6, 30);
    if let Some(q) = text.strip_prefix("otpauth://").and_then(|rest| rest.split_once('?')).map(|(_, q)| q) {
        secret.clear();
        for (k, v) in url::form_urlencoded::parse(q.as_bytes()) {
            match k.as_ref() {
                "secret" => secret = v.into_owned(),
                "digits" => digits = v.parse().ok()?,
                "period" => period = v.parse().ok()?,
                "algorithm" if !v.eq_ignore_ascii_case("SHA1") => return None,
                _ => {}
            }
        }
    }
    let key = base32(&secret)?;
    (!key.is_empty() && (6..=8).contains(&digits) && period > 0).then_some(Seed { key, digits, period })
}

/// The bytes of base32 text (RFC 4648 letters, any case; spaces, dashes and `=` padding ignored).
fn base32(s: &str) -> Option<Vec<u8>> {
    let (mut out, mut acc, mut bits) = (Vec::new(), 0u32, 0u32);
    for c in s.chars().filter(|c| !matches!(c, ' ' | '-' | '=')) {
        let v = match c.to_ascii_uppercase() {
            c @ 'A'..='Z' => c as u32 - 'A' as u32,
            c @ '2'..='7' => c as u32 - '2' as u32 + 26,
            _ => return None,
        };
        acc = (acc << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The code of `seed` at `unix` seconds, and the seconds it still lasts.
pub fn code(seed: &Seed, unix: u64) -> (String, u64) {
    let counter = unix / seed.period;
    let Ok(mut mac) = Hmac::<sha1::Sha1>::new_from_slice(&seed.key) else { return (String::new(), 0) };
    mac.update(&counter.to_be_bytes());
    let h = mac.finalize().into_bytes();
    let at = (h[19] & 0x0f) as usize;
    let n = u32::from_be_bytes([h[at] & 0x7f, h[at + 1], h[at + 2], h[at + 3]]);
    let code = n % 10u32.pow(seed.digits);
    (format!("{code:0width$}", width = seed.digits as usize), seed.period - unix % seed.period)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_match_the_rfc_6238_examples() {
        // RFC 6238 appendix B: the SHA-1 seed "12345678901234567890", 8 digits
        let seed = Seed { key: b"12345678901234567890".to_vec(), digits: 8, period: 30 };
        assert_eq!(code(&seed, 59).0, "94287082");
        assert_eq!(code(&seed, 1111111109).0, "07081804");
        assert_eq!(code(&seed, 1234567890).0, "89005924");
        assert_eq!(code(&seed, 2000000000).0, "69279037");
        assert_eq!(code(&seed, 59).1, 1);
    }

    #[test]
    fn seeds_are_read_from_base32_or_a_link() {
        // "12345678901234567890" in base32, as sites show it: grouped and lower case
        let s = parse("gezd gnbv gy3t qojq gezd gnbv gy3t qojq").unwrap();
        assert_eq!((s.key.as_slice(), s.digits, s.period), (&b"12345678901234567890"[..], 6, 30));
        assert_eq!(code(&s, 59).0, "287082");
        let link = parse("otpauth://totp/Site:me?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=Site&digits=8").unwrap();
        assert_eq!(link.digits, 8);
        assert!(parse("not base32 !").is_none());
        assert!(parse("").is_none());
        assert!(parse("otpauth://totp/x?secret=GEZDGNBV&algorithm=SHA512").is_none());
    }
}
