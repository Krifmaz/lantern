// Ported from dustin/go-humanize v1.1.0 (bytes.go: ParseBytes, Bytes), MIT, as autobrr v1.90.0 uses it.
//! Sizes in words, as autobrr reads and writes them: `ParseBytes` reads `10 GB`, `1.5GiB` or `700m`; `Bytes`
//! writes `3.8 GB`.

/// The units go-humanize knows, lower case, with what they multiply by (`kb` is 1000, `kib` 1024; no letter is
/// bytes).
const SIZES: [(&str, u64); 26] = [
    ("b", 1),
    ("kib", 1 << 10),
    ("kb", 1_000),
    ("mib", 1 << 20),
    ("mb", 1_000_000),
    ("gib", 1 << 30),
    ("gb", 1_000_000_000),
    ("tib", 1 << 40),
    ("tb", 1_000_000_000_000),
    ("pib", 1 << 50),
    ("pb", 1_000_000_000_000_000),
    ("eib", 1 << 60),
    ("eb", 1_000_000_000_000_000_000),
    ("", 1),
    ("ki", 1 << 10),
    ("k", 1_000),
    ("mi", 1 << 20),
    ("m", 1_000_000),
    ("gi", 1 << 30),
    ("g", 1_000_000_000),
    ("ti", 1 << 40),
    ("t", 1_000_000_000_000),
    ("pi", 1 << 50),
    ("p", 1_000_000_000_000_000),
    ("ei", 1 << 60),
    ("e", 1_000_000_000_000_000_000),
];

/// Reads a size in words as bytes (go-humanize's `ParseBytes`); the problem in words when it can't.
pub fn parse_bytes(s: &str) -> Result<u64, String> {
    let mut last = 0;
    let mut has_comma = false;
    for (i, r) in s.char_indices() {
        if !(r.is_ascii_digit() || r == '.' || r == ',') {
            break;
        }
        if r == ',' {
            has_comma = true;
        }
        last = i + r.len_utf8();
    }
    let mut num = s[..last].to_string();
    if has_comma {
        num = num.replace(',', "");
    }
    let f: f64 = num.parse().map_err(|_| format!("strconv.ParseFloat: parsing {num:?}: invalid syntax"))?;
    let extra = s[last..].trim().to_lowercase();
    match SIZES.iter().find(|(k, _)| *k == extra) {
        Some((_, m)) => {
            let f = f * *m as f64;
            if f >= u64::MAX as f64 {
                return Err(format!("too large: {s}"));
            }
            Ok(f as u64)
        }
        None => Err(format!("unhandled size name: {extra}")),
    }
}

/// Bytes in words with decimal units (go-humanize's `Bytes`: `3.8 GB`, `82 MB`, `5 B`).
pub fn bytes(s: u64) -> String {
    const UNITS: [&str; 7] = ["B", "kB", "MB", "GB", "TB", "PB", "EB"];
    if s < 10 {
        return format!("{s} B");
    }
    let e = ((s as f64).ln() / 1000f64.ln()).floor();
    let suffix = UNITS[e as usize];
    let val = ((s as f64) / 1000f64.powf(e) * 10.0 + 0.5).floor() / 10.0;
    if val < 10.0 {
        format!("{val:.1} {suffix}")
    } else {
        format!("{val:.0} {suffix}")
    }
}

/// Bytes in words with binary units (go-humanize's `IBytes`: `1.4 GiB`, `82 MiB`, `5 B`).
pub fn ibytes(s: u64) -> String {
    const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    if s < 10 {
        return format!("{s} B");
    }
    let e = ((s as f64).ln() / 1024f64.ln()).floor();
    let suffix = UNITS[e as usize];
    let val = ((s as f64) / 1024f64.powf(e) * 10.0 + 0.5).floor() / 10.0;
    if val < 10.0 {
        format!("{val:.1} {suffix}")
    } else {
        format!("{val:.0} {suffix}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// go-humanize's own byte tests (bytes_test.go), the ones autobrr relies on.
    #[test]
    fn reads_sizes() {
        for (s, want) in [
            ("42", 42),
            ("42MB", 42_000_000),
            ("42MiB", 44_040_192),
            ("42mb", 42_000_000),
            ("42mib", 44_040_192),
            ("42MIB", 44_040_192),
            ("42 MB", 42_000_000),
            ("42 MiB", 44_040_192),
            ("42 mb", 42_000_000),
            ("42 mib", 44_040_192),
            ("42 MIB", 44_040_192),
            ("42.5MB", 42_500_000),
            ("42.5MiB", 44_564_480),
            ("42.5 MB", 42_500_000),
            ("42.5 MiB", 44_564_480),
            ("42M", 42_000_000),
            ("42Mi", 44_040_192),
            ("42m", 42_000_000),
            ("42mi", 44_040_192),
            ("42MI", 44_040_192),
            ("42 M", 42_000_000),
            ("42 Mi", 44_040_192),
            ("42 m", 42_000_000),
            ("42 mi", 44_040_192),
            ("42 MI", 44_040_192),
            ("42.5M", 42_500_000),
            ("42.5Mi", 44_564_480),
            ("42.5 M", 42_500_000),
            ("42.5 Mi", 44_564_480),
            ("1,005.03 MB", 1_005_030_000),
            ("1,005,000 B", 1_005_000),
        ] {
            assert_eq!(parse_bytes(s), Ok(want), "{s}");
        }
        for s in ["", "B", "1 BB", "1 MiBs", "1.5.5 MB", "abc"] {
            assert!(parse_bytes(s).is_err(), "{s}");
        }
    }

    #[test]
    fn writes_sizes() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(9), "9 B");
        assert_eq!(bytes(1000), "1.0 kB");
        assert_eq!(bytes(82_854_982), "83 MB");
        assert_eq!(bytes(3_834_225_472), "3.8 GB");
    }
}
