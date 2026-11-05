//! A torrent's info hash: from a `.torrent` file (SHA-1 of its bencoded `info` dictionary) or from a magnet link
//! (`xt=urn:btih:`, hex or base32). Apps that think they talk to qBittorrent find their torrents by it.

use sha1::{Digest, Sha1};

/// Where the bencoded value starting at `i` ends (one past its last byte); `None` when it is broken.
fn skip(b: &[u8], i: usize) -> Option<usize> {
    match *b.get(i)? {
        b'i' => Some(i + b[i..].iter().position(|&c| c == b'e')? + 1),
        b'l' | b'd' => {
            let mut j = i + 1;
            while *b.get(j)? != b'e' {
                j = skip(b, j)?;
            }
            Some(j + 1)
        }
        b'0'..=b'9' => {
            let colon = i + b[i..].iter().position(|&c| c == b':')?;
            let len: usize = std::str::from_utf8(&b[i..colon]).ok()?.parse().ok()?;
            let end = colon + 1 + len;
            (end <= b.len()).then_some(end)
        }
        _ => None,
    }
}

/// The info hash of a `.torrent` file, in lower-case hex.
pub fn info_hash(torrent: &[u8]) -> Option<String> {
    if torrent.first() != Some(&b'd') {
        return None;
    }
    let mut i = 1;
    while *torrent.get(i)? != b'e' {
        let key_end = skip(torrent, i)?;
        let key = &torrent[i..key_end];
        let value_end = skip(torrent, key_end)?;
        if key == b"4:info" {
            return Some(hex::encode(Sha1::digest(&torrent[key_end..value_end])));
        }
        i = value_end;
    }
    None
}

/// The info hash of a magnet link, in lower-case hex.
pub fn magnet_hash(magnet: &str) -> Option<String> {
    let q = magnet.strip_prefix("magnet:?")?;
    let xt = url::form_urlencoded::parse(q.as_bytes()).find(|(k, v)| k == "xt" && v.to_lowercase().starts_with("urn:btih:"))?.1;
    let h = &xt[9..];
    if h.len() == 40 && h.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Some(h.to_lowercase());
    }
    if h.len() == 32 {
        // base32 (RFC 4648, no padding)
        let mut bits: u64 = 0;
        let mut n = 0;
        let mut out = Vec::with_capacity(20);
        for c in h.to_uppercase().bytes() {
            let v = match c {
                b'A'..=b'Z' => c - b'A',
                b'2'..=b'7' => c - b'2' + 26,
                _ => return None,
            };
            bits = (bits << 5) | v as u64;
            n += 5;
            if n >= 8 {
                n -= 8;
                out.push((bits >> n) as u8);
            }
        }
        return (out.len() == 20).then(|| hex::encode(out));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_torrents_and_magnets() {
        let info = b"d6:lengthi5e4:name5:a.mkv12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaae";
        let mut t = b"d8:announce14:http://t.test/4:info".to_vec();
        t.extend_from_slice(info);
        t.push(b'e');
        assert_eq!(info_hash(&t), Some(hex::encode(Sha1::digest(info))));
        assert_eq!(info_hash(b"not a torrent"), None);
        assert_eq!(
            magnet_hash("magnet:?xt=urn:btih:ABCDEF0123456789ABCDEF0123456789ABCDEF01&dn=x").as_deref(),
            Some("abcdef0123456789abcdef0123456789abcdef01")
        );
        // the same hash in base32
        let b32 = "VPG66AJDIVTYTK6N54ASGRLHRGV433YB";
        assert_eq!(magnet_hash(&format!("magnet:?xt=urn:btih:{b32}")).as_deref(), Some("abcdef0123456789abcdef0123456789abcdef01"));
        assert_eq!(magnet_hash("magnet:?dn=x"), None);
    }
}
