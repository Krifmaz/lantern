//! Reading a `.torrent` file: its info hash (SHA-1 of the bencoded `info` dictionary as it is in the file), its
//! name and its files with their sizes, which is all cross-seeding needs to compare two torrents.

use sha1::{Digest, Sha1};

/// What a `.torrent` holds that cross-seeding compares.
#[derive(Debug, Clone, PartialEq)]
pub struct Meta {
    /// The info hash, 40 lowercase hex digits.
    pub info_hash: String,
    /// The torrent's name (its file, or its folder).
    pub name: String,
    /// Each file's path inside the save folder (`Folder/a.mkv`, or `a.mkv` for a one-file torrent) and size.
    pub files: Vec<(String, i64)>,
}

impl Meta {
    /// Every file's size added up.
    pub fn total(&self) -> i64 {
        self.files.iter().map(|(_, s)| s).sum()
    }
}

/// A bencoded value.
#[derive(Debug, Clone, PartialEq)]
enum B {
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<B>),
    Dict(Vec<(Vec<u8>, B)>),
}

impl B {
    fn get(&self, key: &str) -> Option<&B> {
        match self {
            B::Dict(d) => d.iter().find(|(k, _)| k == key.as_bytes()).map(|(_, v)| v),
            _ => None,
        }
    }
    fn text(&self) -> Option<String> {
        match self {
            B::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
            _ => None,
        }
    }
    fn int(&self) -> Option<i64> {
        match self {
            B::Int(i) => Some(*i),
            _ => None,
        }
    }
}

/// Reads one value at `*at`; on return `*at` is just past it.
fn value(d: &[u8], at: &mut usize, depth: usize) -> Option<B> {
    if depth > 64 {
        return None;
    }
    match *d.get(*at)? {
        b'i' => {
            let end = *at + d[*at..].iter().position(|&c| c == b'e')?;
            let n = std::str::from_utf8(&d[*at + 1..end]).ok()?.parse().ok()?;
            *at = end + 1;
            Some(B::Int(n))
        }
        b'l' => {
            *at += 1;
            let mut list = Vec::new();
            while *d.get(*at)? != b'e' {
                list.push(value(d, at, depth + 1)?);
            }
            *at += 1;
            Some(B::List(list))
        }
        b'd' => {
            *at += 1;
            let mut dict = Vec::new();
            while *d.get(*at)? != b'e' {
                let B::Bytes(k) = value(d, at, depth + 1)? else { return None };
                let v = value(d, at, depth + 1)?;
                dict.push((k, v));
            }
            *at += 1;
            Some(B::Dict(dict))
        }
        b'0'..=b'9' => {
            let colon = *at + d[*at..].iter().position(|&c| c == b':')?;
            let len: usize = std::str::from_utf8(&d[*at..colon]).ok()?.parse().ok()?;
            let start = colon + 1;
            let end = start.checked_add(len)?;
            let bytes = d.get(start..end)?.to_vec();
            *at = end;
            Some(B::Bytes(bytes))
        }
        _ => None,
    }
}

/// The bytes of the top dictionary's `info` value, as they are in the file.
fn info_span(d: &[u8]) -> Option<(usize, usize)> {
    if d.first() != Some(&b'd') {
        return None;
    }
    let mut at = 1;
    while *d.get(at)? != b'e' {
        let B::Bytes(k) = value(d, &mut at, 0)? else { return None };
        let start = at;
        value(d, &mut at, 0)?;
        if k == b"info" {
            return Some((start, at));
        }
    }
    None
}

/// Reads a `.torrent` file; `None` when it isn't one.
pub fn parse(d: &[u8]) -> Option<Meta> {
    let (start, end) = info_span(d)?;
    let mut at = start;
    let info = value(d, &mut at, 0)?;
    let name = info.get("name.utf-8").or_else(|| info.get("name"))?.text()?;
    let files = match info.get("files") {
        Some(B::List(list)) => {
            let mut out = Vec::new();
            for f in list {
                let size = f.get("length")?.int()?;
                let B::List(parts) = f.get("path.utf-8").or_else(|| f.get("path"))? else { return None };
                let parts: Vec<String> = parts.iter().filter_map(B::text).collect();
                // padding files (BEP 47) aren't on disk
                if parts.first().is_some_and(|p| p.starts_with(".pad")) {
                    continue;
                }
                out.push((format!("{name}/{}", parts.join("/")), size));
            }
            out
        }
        _ => vec![(name.clone(), info.get("length")?.int()?)],
    };
    let hash = Sha1::digest(&d[start..end]);
    Some(Meta { info_hash: hash.iter().map(|b| format!("{b:02x}")).collect(), name, files })
}

/// A `.torrent` file with these files, for tests and the test tracker: one file at the top when there is one,
/// else a folder `name` holding them. `salt` goes into the info dictionary, so the same files give another hash
/// (as another tracker's copy of the same release does).
pub fn build(name: &str, files: &[(&str, i64)], salt: &str) -> Vec<u8> {
    fn s(out: &mut Vec<u8>, t: &str) {
        out.extend_from_slice(format!("{}:", t.len()).as_bytes());
        out.extend_from_slice(t.as_bytes());
    }
    let mut info = Vec::new();
    info.push(b'd');
    if files.len() == 1 && files[0].0 == name {
        s(&mut info, "length");
        info.extend_from_slice(format!("i{}e", files[0].1).as_bytes());
    } else {
        s(&mut info, "files");
        info.push(b'l');
        for (p, size) in files {
            info.extend_from_slice(b"d");
            s(&mut info, "length");
            info.extend_from_slice(format!("i{size}e").as_bytes());
            s(&mut info, "path");
            info.push(b'l');
            for part in p.split('/') {
                s(&mut info, part);
            }
            info.extend_from_slice(b"ee");
        }
        info.push(b'e');
    }
    s(&mut info, "name");
    s(&mut info, name);
    s(&mut info, "piece length");
    info.extend_from_slice(b"i16384e");
    s(&mut info, "pieces");
    s(&mut info, "aaaaaaaaaaaaaaaaaaaa");
    s(&mut info, "source");
    s(&mut info, salt);
    info.push(b'e');
    let mut out = Vec::new();
    out.push(b'd');
    s(&mut out, "announce");
    s(&mut out, "http://t.test/");
    s(&mut out, "info");
    out.extend_from_slice(&info);
    out.push(b'e');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_one_file_and_folder_torrents() {
        let one = parse(&build("Movie.2025.mkv", &[("Movie.2025.mkv", 1000)], "a")).unwrap();
        assert_eq!((one.name.as_str(), one.files.clone()), ("Movie.2025.mkv", vec![("Movie.2025.mkv".to_string(), 1000)]));
        assert_eq!(one.info_hash.len(), 40);
        let many = parse(&build("Show.S01", &[("e1.mkv", 10), ("Subs/e1.srt", 2)], "a")).unwrap();
        assert_eq!(many.files, vec![("Show.S01/e1.mkv".to_string(), 10), ("Show.S01/Subs/e1.srt".to_string(), 2)]);
        assert_eq!(many.total(), 12);
        let other = parse(&build("Show.S01", &[("e1.mkv", 10), ("Subs/e1.srt", 2)], "b")).unwrap();
        assert_ne!(many.info_hash, other.info_hash, "another tracker's copy has another hash");
        assert_eq!(many.files, other.files);
    }

    #[test]
    fn the_hash_is_of_the_info_as_written() {
        // the test tracker's tiny torrent; its info hash worked out by hand with sha1sum
        let t = b"d8:announce14:http://t.test/4:infod6:lengthi1e4:name1:a12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaaee";
        let m = parse(t).unwrap();
        let info = &t[b"d8:announce14:http://t.test/4:info".len()..t.len() - 1];
        let want: String = Sha1::digest(info).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(m.info_hash, want);
        assert_eq!(m.files, vec![("a".to_string(), 1)]);
    }

    #[test]
    fn refuses_what_isnt_a_torrent() {
        assert!(parse(b"<html>").is_none());
        assert!(parse(b"d4:infoi1ee").is_none());
        assert!(parse(b"d4:info99999:xe").is_none());
        assert!(parse(b"").is_none());
    }
}
