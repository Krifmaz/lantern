// Ported from cross-seed v6.13.9 (src/parseTorrent.ts), Apache-2.0.
//! Reading a `.torrent` file: its info hash (SHA-1 of the bencoded `info` dictionary as it is in the file), its
//! name and its files with their sizes. [`parse`] is the short form the release feeds use; [`Metafile`] is
//! cross-seed's reading, with its checks (required fields, no path outside the torrent's folder), its file order,
//! its parsed title and its trackers.

use super::searchee::File;
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
pub enum B {
    /// An integer.
    Int(i64),
    /// A byte string.
    Bytes(Vec<u8>),
    /// A list.
    List(Vec<B>),
    /// A dictionary, keys in the order they came.
    Dict(Vec<(Vec<u8>, B)>),
}

/// The bencoded value at the start of `d` (a `.fastresume` file, for one), or `None` when it isn't one.
pub fn decode_value(d: &[u8]) -> Option<B> {
    let mut at = 0;
    value(d, &mut at, 0)
}

impl B {
    /// The list's items, when it is a list.
    pub fn list(&self) -> Option<&[B]> {
        match self {
            B::List(l) => Some(l),
            _ => None,
        }
    }

    /// JavaScript's `toString()` of the value as cross-seed's bencode reader gives it: a byte string as text, a
    /// list as its items joined with commas, a number as digits.
    pub fn js_string(&self) -> String {
        match self {
            B::Int(i) => i.to_string(),
            B::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            B::List(l) => l.iter().map(B::js_string).collect::<Vec<_>>().join(","),
            B::Dict(_) => "[object Object]".into(),
        }
    }

    /// The value under `key`, when it is a dictionary that has it.
    pub fn get(&self, key: &str) -> Option<&B> {
        match self {
            B::Dict(d) => d.iter().find(|(k, _)| k == key.as_bytes()).map(|(_, v)| v),
            _ => None,
        }
    }
    /// A byte string as text.
    pub fn text(&self) -> Option<String> {
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

/// `{ ...meta.raw, [key]: value }` re-encoded: the `.torrent` with `key` set to the bencoded `value` in its top
/// dictionary, keys in byte order as bencode wants (rTorrent's `libtorrent_resume`). `None` when it isn't one.
pub fn with_top_key(d: &[u8], key: &str, value_bencoded: &[u8]) -> Option<Vec<u8>> {
    if d.first() != Some(&b'd') {
        return None;
    }
    let mut entries: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut at = 1;
    while *d.get(at)? != b'e' {
        let B::Bytes(k) = value(d, &mut at, 0)? else { return None };
        let start = at;
        value(d, &mut at, 0)?;
        if k != key.as_bytes() {
            entries.push((k, d[start..at].to_vec()));
        }
    }
    entries.push((key.as_bytes().to_vec(), value_bencoded.to_vec()));
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = vec![b'd'];
    for (k, v) in entries {
        out.extend_from_slice(format!("{}:", k.len()).as_bytes());
        out.extend_from_slice(&k);
        out.extend_from_slice(&v);
    }
    out.push(b'e');
    Some(out)
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

/// `ensure`: a missing required field, in cross-seed's words.
fn missing(field: &str) -> String {
    format!("Torrent is missing required field: {field}")
}

/// `ensureContained`: refuses a name (or, with `can_start_at_root`, a file path part) that would put a file outside
/// the torrent's folder: a `..` part, or a name that starts at a root.
fn ensure_contained(path: &str, can_start_at_root: bool) -> Result<(), String> {
    let b = path.as_bytes();
    let from_root =
        path.starts_with(['/', '\\']) || (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'/' | b'\\'));
    if (!can_start_at_root && from_root) || path.split(['/', '\\']).any(|p| p == "..") {
        // JSON.stringify(path.slice(0, 200)): the first 200 UTF-16 units, quoted
        let units: Vec<u16> = path.encode_utf16().take(200).collect();
        let shown = serde_json::to_string(&String::from_utf16_lossy(&units)).unwrap_or_default();
        return Err(format!("Torrent has a path outside of its directory: {shown}"));
    }
    Ok(())
}

/// `path.join` (POSIX) of the torrent's name and a file's path parts.
fn join_posix(parts: &[&str]) -> String {
    let mut out: Vec<&str> = Vec::new();
    for part in parts {
        for seg in part.split('/') {
            match seg {
                "" | "." => {}
                ".." => {
                    out.pop();
                }
                s => out.push(s),
            }
        }
    }
    if out.is_empty() {
        ".".into()
    } else {
        out.join("/")
    }
}

/// An order close to JavaScript's `localeCompare` (the default collation): spaces and punctuation, then digits,
/// then letters, ignoring case first, small letters before capitals on a tie.
pub fn locale_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    fn key(c: char) -> (u8, String) {
        let class = if c.is_alphabetic() {
            2
        } else if c.is_numeric() {
            1
        } else {
            0
        };
        (class, c.to_lowercase().collect())
    }
    let primary = a.chars().map(key).cmp(b.chars().map(key));
    // on a tie, a small letter before its capital
    primary.then_with(|| a.chars().map(|c| !c.is_lowercase()).cmp(b.chars().map(|c| !c.is_lowercase())))
}

/// `sanitizeTrackerUrl`: a tracker address's host (and port, when not the default one).
pub fn sanitize_tracker_url(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    let host = u.host_str()?;
    Some(match u.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_string(),
    })
}

/// `Metafile`: a `.torrent` file as cross-seed reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct Metafile {
    /// The info hash, 40 lowercase hex digits.
    pub info_hash: String,
    /// Every file's size added up.
    pub length: i64,
    /// The torrent's name (its file, or its folder).
    pub name: String,
    /// The title parsed from the name and files ([`super::searchee::parse_title`]), else the name.
    pub title: String,
    /// The piece length in bytes.
    pub piece_length: i64,
    /// Its files, by path in [`locale_cmp`] order: name, path inside the save folder, size.
    pub files: Vec<File>,
    /// Whether the torrent is one file at the top.
    pub is_single_file_torrent: bool,
    /// Its category in a client (from the client's resume data), when known.
    pub category: Option<String>,
    /// Its tags in a client, when known.
    pub tags: Option<Vec<String>>,
    /// Its trackers' hosts.
    pub trackers: Vec<String>,
    /// The file as it came.
    pub raw: Vec<u8>,
}

impl Metafile {
    /// `Metafile.decode`: reads a `.torrent` file, or says in cross-seed's words why it can't.
    pub fn decode(d: &[u8]) -> Result<Metafile, String> {
        let mut at = 0;
        let top = value(d, &mut at, 0).ok_or_else(|| "Torrent is not a valid bencoded file".to_string())?;
        let info = top.get("info").filter(|i| matches!(i, B::Dict(_))).ok_or_else(|| missing("info"))?;
        let name = info.get("name.utf-8").or_else(|| info.get("name")).and_then(B::text).ok_or_else(|| missing("info.name"))?;
        let piece_length = info.get("piece length").and_then(B::int).filter(|n| *n != 0).ok_or_else(|| missing("info['piece length']"))?;
        info.get("pieces").ok_or_else(|| missing("info.pieces"))?;
        let raw_files = match info.get("files") {
            Some(B::List(list)) => {
                for f in list {
                    f.get("length").and_then(B::int).ok_or_else(|| missing("info.files[0].length"))?;
                    if !matches!(f.get("path.utf-8").or_else(|| f.get("path")), Some(B::List(_))) {
                        return Err(missing("info.files[0].path"));
                    }
                }
                Some(list)
            }
            _ => {
                info.get("length").and_then(B::int).ok_or_else(|| missing("info.length"))?;
                None
            }
        };
        let (start, end) = info_span(d).ok_or_else(|| missing("info"))?;
        let info_hash: String = Sha1::digest(&d[start..end]).iter().map(|b| format!("{b:02x}")).collect();
        ensure_contained(&name, false)?;
        let (files, length, single) = match raw_files {
            None => {
                let length = info.get("length").and_then(B::int).unwrap_or(0);
                (vec![File { name: name.clone(), path: name.clone(), length }], length, true)
            }
            Some(list) => {
                let mut files = Vec::new();
                for f in list {
                    let Some(B::List(parts)) = f.get("path.utf-8").or_else(|| f.get("path")) else { continue };
                    let mut segs = Vec::new();
                    for p in parts {
                        let seg = p.text().unwrap_or_default();
                        ensure_contained(&seg, true)?;
                        // a zero-length path part is an underscore by convention
                        segs.push(if seg.is_empty() { "_".to_string() } else { seg });
                    }
                    let mut all: Vec<&str> = vec![name.as_str()];
                    all.extend(segs.iter().map(String::as_str));
                    files.push(File {
                        name: segs.last().cloned().unwrap_or_default(),
                        length: f.get("length").and_then(B::int).unwrap_or(0),
                        path: join_posix(&all),
                    });
                }
                files.sort_by(|a, b| locale_cmp(&a.path, &b.path));
                let length = files.iter().map(|f| f.length).sum();
                (files, length, false)
            }
        };
        let title = super::searchee::parse_title(&name, &files, None).unwrap_or_else(|| name.clone());
        let tier_hosts = |tier: &B| -> Vec<String> {
            match tier {
                B::List(urls) => urls.iter().filter_map(B::text).filter_map(|u| sanitize_tracker_url(&u)).collect(),
                _ => Vec::new(),
            }
        };
        let trackers = match top.get("announce-list") {
            Some(B::List(tiers)) if !tiers.is_empty() => tiers.iter().flat_map(tier_hosts).collect(),
            _ => top.get("announce").and_then(B::text).and_then(|u| sanitize_tracker_url(&u)).into_iter().collect(),
        };
        Ok(Metafile {
            info_hash,
            length,
            name,
            title,
            piece_length,
            files,
            is_single_file_torrent: single,
            category: None,
            tags: None,
            trackers,
            raw: d.to_vec(),
        })
    }

    /// `getFileSystemSafeName`: the name without its first `/`.
    pub fn file_system_safe_name(&self) -> String {
        self.name.replacen('/', "", 1)
    }
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

    /// A bencoded string.
    fn bs(t: &[u8]) -> Vec<u8> {
        let mut v = format!("{}:", t.len()).into_bytes();
        v.extend_from_slice(t);
        v
    }

    /// A bencoded dictionary of these (key, encoded value) pairs, keys sorted.
    fn dict(mut entries: Vec<(&[u8], Vec<u8>)>) -> Vec<u8> {
        entries.sort_by(|a, b| a.0.cmp(b.0));
        let mut out = b"d".to_vec();
        for (k, v) in entries {
            out.extend(bs(k));
            out.extend(v);
        }
        out.push(b'e');
        out
    }

    /// cross-seed's test torrent of one file of length 1 (piece length 16384, 20 bytes of pieces).
    fn single_file(name: &str, key: &str) -> Vec<u8> {
        let info = dict(vec![
            (key.as_bytes(), bs(name.as_bytes())),
            (b"length", b"i1e".to_vec()),
            (b"piece length", b"i16384e".to_vec()),
            (b"pieces", bs(&[1u8; 20])),
        ]);
        dict(vec![(b"info", info)])
    }

    /// cross-seed's test torrent of a folder `name` with one file of length 1 at `path`.
    fn multi_file(name: &str, path: &[&str]) -> Vec<u8> {
        let mut parts = b"l".to_vec();
        for p in path {
            parts.extend(bs(p.as_bytes()));
        }
        parts.push(b'e');
        let file = dict(vec![(b"length", b"i1e".to_vec()), (b"path", parts)]);
        let mut files = b"l".to_vec();
        files.extend(file);
        files.push(b'e');
        let info = dict(vec![
            (b"name", bs(name.as_bytes())),
            (b"files", files),
            (b"piece length", b"i16384e".to_vec()),
            (b"pieces", bs(&[1u8; 20])),
        ]);
        dict(vec![(b"info", info)])
    }

    fn file(name: &str, path: &str) -> File {
        File { name: name.into(), path: path.into(), length: 1 }
    }

    // Ported from cross-seed v6.13.9 (tests/metafileSafety.test.ts), Apache-2.0.
    #[test]
    fn metafile_reads_a_name_and_file_paths_that_stay_inside_their_directory() {
        assert_eq!(
            Metafile::decode(&single_file("Some.Release.mkv", "name")).unwrap().files,
            vec![file("Some.Release.mkv", "Some.Release.mkv")]
        );
        assert_eq!(
            Metafile::decode(&multi_file("Some.Release", &["sub", "a.mkv"])).unwrap().files,
            vec![file("a.mkv", "Some.Release/sub/a.mkv")]
        );
    }

    #[test]
    fn metafile_accepts_names() {
        for name in ["..a", "a..", "a..b", "...", "X: The Series", "a b/c d", "é/ü"] {
            assert_eq!(Metafile::decode(&single_file(name, "name")).unwrap().name, name);
            assert_eq!(Metafile::decode(&multi_file(name, &["a.mkv"])).unwrap().name, name);
        }
    }

    #[test]
    fn metafile_rejects_names() {
        let message = "Torrent has a path outside of its directory";
        for name in [
            "..",
            "../evil.mkv",
            "../../escaped/evil.mkv",
            "a/../../evil.mkv",
            "a/..",
            r"a\..\..\evil.mkv",
            "/etc/evil.mkv",
            r"\evil.mkv",
            r"C:\evil.mkv",
            "c:/evil.mkv",
        ] {
            assert!(Metafile::decode(&single_file(name, "name")).unwrap_err().contains(message), "{name}");
            assert!(Metafile::decode(&single_file(name, "name.utf-8")).unwrap_err().contains(message), "{name}");
            assert!(Metafile::decode(&multi_file(name, &["a.mkv"])).unwrap_err().contains(message), "{name}");
        }
    }

    #[test]
    fn metafile_rejects_file_paths() {
        let paths: [&[&str]; 5] =
            [&[".."], &["..", "evil.mkv"], &["sub", "..", "..", "evil.mkv"], &["sub/../../evil.mkv"], &[r"sub\..\..\evil.mkv"]];
        for path in paths {
            let e = Metafile::decode(&multi_file("Release", path)).unwrap_err();
            assert!(e.contains("Torrent has a path outside of its directory"), "{path:?}");
        }
    }

    #[test]
    fn metafile_keeps_a_file_path_that_starts_at_a_root_inside_the_torrents_directory() {
        let meta = Metafile::decode(&multi_file("Release", &["/etc/evil.mkv"])).unwrap();
        assert_eq!(meta.files[0].path, "Release/etc/evil.mkv");
    }

    #[test]
    fn metafile_names_the_offending_path_in_the_error() {
        assert!(Metafile::decode(&single_file("../evil.mkv", "name")).unwrap_err().contains(r#""../evil.mkv""#));
    }

    #[test]
    fn metafile_does_not_put_a_whole_oversized_path_in_the_error() {
        let name = format!("../{}", "a".repeat(10_000));
        let message = Metafile::decode(&single_file(&name, "name")).unwrap_err();
        assert!(message.contains("outside of its directory"));
        assert!(message.len() < 300);
    }

    #[test]
    fn metafile_parses_long_names_quickly() {
        let hostile = format!("{}.mkv", "[a]".repeat(800));
        let start = std::time::Instant::now();
        assert_eq!(Metafile::decode(&single_file(&hostile, "name")).unwrap().title, hostile);
        assert_eq!(Metafile::decode(&multi_file("Release", &[&hostile])).unwrap().title, "Release");
        assert!(start.elapsed().as_millis() < 1000, "{:?}", start.elapsed());
    }

    #[test]
    fn metafile_parses_a_file_name_ending_in_a_long_run_of_digits_quickly() {
        let name = format!("Show.S01E01-{}x.mkv", "1".repeat(30));
        let start = std::time::Instant::now();
        let meta = Metafile::decode(&multi_file("Show", &[&name])).unwrap();
        assert!(meta.title.starts_with("Show S1E1"), "{}", meta.title);
        assert!(start.elapsed().as_millis() < 1000);
    }

    #[test]
    fn metafile_requires_its_fields_and_reads_trackers() {
        assert_eq!(Metafile::decode(b"d3:fooi1ee").unwrap_err(), "Torrent is missing required field: info");
        assert_eq!(Metafile::decode(b"d4:infod6:lengthi1eee").unwrap_err(), "Torrent is missing required field: info.name");
        let m = Metafile::decode(&build("Show.S01", &[("b.mkv", 2), ("A.mkv", 1)], "x")).unwrap();
        assert_eq!(m.trackers, vec!["t.test"]);
        assert_eq!(m.files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), vec!["A.mkv", "b.mkv"]);
        assert_eq!((m.length, m.piece_length, m.is_single_file_torrent), (3, 16384, false));
        assert_eq!(m.info_hash, parse(&m.raw).unwrap().info_hash);
    }

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
