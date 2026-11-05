//! Reading a `.torrent` file's size, for an announce that doesn't say it while a filter has a size limit. The file is
//! read by cross-seed's reader ([`crate::crossseed::torrent`]), so Lantern has one bencode reader.

/// The total size in bytes of the torrent file `bytes` (padding files left out); `None` when it isn't one.
pub fn size(bytes: &[u8]) -> Option<u64> {
    u64::try_from(crate::crossseed::torrent::parse(bytes)?.total()).ok()
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
