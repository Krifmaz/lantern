//! Reading a `.torrent` file's size, for an announce that doesn't say it while a filter has a size limit.

/// One bencoded value.
#[derive(Debug, Clone, PartialEq)]
enum Ben<'a> {
    Int(i64),
    Bytes(&'a [u8]),
    List(Vec<Ben<'a>>),
    Dict(Vec<(&'a [u8], Ben<'a>)>),
}

impl<'a> Ben<'a> {
    /// The value under `key` of a dictionary.
    fn get(&self, key: &str) -> Option<&Ben<'a>> {
        match self {
            Ben::Dict(d) => d.iter().find(|(k, _)| *k == key.as_bytes()).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The number, if it is one.
    fn int(&self) -> Option<i64> {
        match self {
            Ben::Int(i) => Some(*i),
            _ => None,
        }
    }
}

/// Reads one value at `at`; the value and where the next starts. Nested at most 64 deep.
fn value(b: &[u8], at: usize, depth: u32) -> Option<(Ben<'_>, usize)> {
    if depth > 64 {
        return None;
    }
    match *b.get(at)? {
        b'i' => {
            let end = at + 1 + b[at + 1..].iter().position(|&c| c == b'e')?;
            Some((Ben::Int(std::str::from_utf8(&b[at + 1..end]).ok()?.parse().ok()?), end + 1))
        }
        b'l' => {
            let mut items = Vec::new();
            let mut i = at + 1;
            while *b.get(i)? != b'e' {
                let (v, n) = value(b, i, depth + 1)?;
                items.push(v);
                i = n;
            }
            Some((Ben::List(items), i + 1))
        }
        b'd' => {
            let mut items = Vec::new();
            let mut i = at + 1;
            while *b.get(i)? != b'e' {
                let (k, n) = value(b, i, depth + 1)?;
                let Ben::Bytes(k) = k else { return None };
                let (v, n) = value(b, n, depth + 1)?;
                items.push((k, v));
                i = n;
            }
            Some((Ben::Dict(items), i + 1))
        }
        b'0'..=b'9' => {
            let colon = at + b[at..].iter().position(|&c| c == b':')?;
            let len: usize = std::str::from_utf8(&b[at..colon]).ok()?.parse().ok()?;
            let start = colon + 1;
            let end = start.checked_add(len)?;
            Some((Ben::Bytes(b.get(start..end)?), end))
        }
        _ => None,
    }
}

/// The total size in bytes of the torrent file `bytes`; `None` when it isn't one.
pub fn size(bytes: &[u8]) -> Option<u64> {
    let (root, _) = value(bytes, 0, 0)?;
    let info = root.get("info")?;
    if let Some(len) = info.get("length").and_then(Ben::int) {
        return u64::try_from(len).ok();
    }
    match info.get("files")? {
        Ben::List(files) => files.iter().map(|f| f.get("length").and_then(Ben::int).and_then(|l| u64::try_from(l).ok())).sum(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_single_and_multi_file_sizes() {
        assert_eq!(size(b"d8:announce3:url4:infod6:lengthi1234e4:name1:aee"), Some(1234));
        assert_eq!(size(b"d4:infod5:filesld6:lengthi10e4:pathl1:aeed6:lengthi5e4:pathl1:beee4:name1:xee"), Some(15));
        assert_eq!(size(b"<html>login</html>"), None);
        assert_eq!(size(b"d4:info"), None);
    }
}
