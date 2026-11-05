// Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core/Configuration/AllowedHostsParser.cs,
// src/NzbDrone.Common/Network/IPNetworkParser.cs, src/NzbDrone.Core/Validation/RuleBuilderExtensions.cs,
// src/NzbDrone.Core/Validation/IpValidation.cs, src/Prowlarr.Http/Validation/RuleBuilderExtensions.cs), GPL-3.0.

//! The checks Prowlarr runs on a resource before it saves it, and the answer it gives when one fails: a 400 with
//! a list of failures, each naming the field (`propertyName`) and saying what is wrong (`errorMessage`), the way
//! FluentValidation 9 writes them. Prowlarr's web app puts each message next to its field.

use serde_json::{json, Map, Value};
use std::net::IpAddr;

/// One failed check, as FluentValidation's `ValidationFailure` is written to JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    /// The field, as named in the resource class (`Port`, `UILanguage`).
    pub property: String,
    /// What is wrong, in words.
    pub message: String,
    /// The value the request gave.
    pub attempted: Value,
    /// The check that failed (`NotEmptyValidator`, `PredicateValidator`, …).
    pub code: String,
    /// The values the message was made from (`PropertyName`, `PropertyValue`, `From`, `To`, …).
    pub placeholders: Map<String, Value>,
}

impl Failure {
    /// A failure of `code` on `property` (whose value was `attempted`) saying `message`.
    pub fn new(property: &str, attempted: Value, code: &str, message: impl Into<String>) -> Failure {
        let mut placeholders = Map::new();
        placeholders.insert("PropertyName".into(), json!(display_name(property)));
        placeholders.insert("PropertyValue".into(), attempted.clone());
        Failure { property: property.to_string(), message: message.into(), attempted, code: code.to_string(), placeholders }
    }

    /// The same failure with one more placeholder value.
    pub fn with(mut self, key: &str, value: Value) -> Failure {
        self.placeholders.insert(key.into(), value);
        self
    }

    /// The failure as Prowlarr answers it (null fields left out, as Prowlarr's JSON writer does).
    pub fn json(&self) -> Value {
        let mut v = json!({
            "propertyName": self.property,
            "errorMessage": self.message,
            "severity": "error",
            "formattedMessageArguments": [],
            "formattedMessagePlaceholderValues": self.placeholders,
        });
        if !self.attempted.is_null() {
            v["attemptedValue"] = self.attempted.clone();
        }
        if !self.code.is_empty() {
            v["errorCode"] = json!(self.code);
        }
        v
    }
}

/// How FluentValidation names a property in its messages: `SslPort` → `Ssl Port`, `UILanguage` → `UI Language`.
pub fn display_name(property: &str) -> String {
    let c: Vec<char> = property.chars().collect();
    let mut out = String::new();
    for (i, ch) in c.iter().enumerate() {
        if i > 0 && ch.is_uppercase() {
            let prev_lower = c[i - 1].is_lowercase() || c[i - 1].is_ascii_digit();
            let next_lower = c.get(i + 1).is_some_and(|n| n.is_lowercase());
            if prev_lower || (c[i - 1].is_uppercase() && next_lower) {
                out.push(' ');
            }
        }
        out.push(*ch);
    }
    out
}

/// FluentValidation's `NotEmpty` on text: `'<Name>' must not be empty.` when it is null, empty or only spaces.
pub fn not_empty(property: &str, value: Option<&str>) -> Option<Failure> {
    value
        .is_none_or(|v| v.trim().is_empty())
        .then(|| Failure::new(property, json!(value), "NotEmptyValidator", format!("'{}' must not be empty.", display_name(property))))
}

/// FluentValidation's `NotNull`: `'<Name>' must not be empty.` when it is null.
pub fn not_null(property: &str, value: &Value) -> Option<Failure> {
    value
        .is_null()
        .then(|| Failure::new(property, Value::Null, "NotNullValidator", format!("'{}' must not be empty.", display_name(property))))
}

/// FluentValidation's `InclusiveBetween`: `'<Name>' must be between <from> and <to>. You entered <value>.`
pub fn between(property: &str, value: i64, from: i64, to: i64) -> Option<Failure> {
    (value < from || value > to).then(|| {
        Failure::new(
            property,
            json!(value),
            "InclusiveBetweenValidator",
            format!("'{}' must be between {from} and {to}. You entered {value}.", display_name(property)),
        )
        .with("From", json!(from))
        .with("To", json!(to))
    })
}

/// FluentValidation's `GreaterThan`: `'<Name>' must be greater than '<than>'.`
pub fn greater_than(property: &str, value: i64, than: i64) -> Option<Failure> {
    (value <= than).then(|| {
        Failure::new(property, json!(value), "GreaterThanValidator", format!("'{}' must be greater than '{than}'.", display_name(property)))
            .with("ComparisonValue", json!(than))
    })
}

/// FluentValidation's `Must` without its own message: `The specified condition was not met for '<Name>'.`
pub fn condition(property: &str, value: Value) -> Failure {
    let msg = format!("The specified condition was not met for '{}'.", display_name(property));
    Failure::new(property, value, "PredicateValidator", msg)
}

/// Prowlarr's `ValidPort`: between 1 and 65535, and above 1024 unless it is 80 or 443.
pub fn valid_port(property: &str, value: i64) -> Option<Failure> {
    if let Some(f) = between(property, value, 1, 65535) {
        return Some(f);
    }
    (value <= 1024 && value != 80 && value != 443).then(|| condition(property, json!(value)))
}

/// Prowlarr's `ValidId` (greater than 0), checked on every PUT.
pub fn valid_id(value: i64) -> Option<Failure> {
    greater_than("Id", value, 0)
}

/// Prowlarr's `IsValidIpAddress`: an IPv4 or IPv6 address, but not 255.255.255.255 or an IPv6 multicast address.
pub fn is_valid_ip_address(value: &str) -> bool {
    match value.parse::<IpAddr>() {
        Ok(IpAddr::V4(a)) => !a.is_broadcast(),
        Ok(IpAddr::V6(a)) => !a.is_multicast(),
        Err(_) => false,
    }
}

/// .NET's `Uri.CheckHostName`, as far as the allowed hosts need it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKind {
    /// Not a host name.
    Unknown,
    /// A DNS name (`prowlarr.local`).
    Dns,
    /// An IPv4 address.
    IPv4,
    /// An IPv6 address, with or without brackets.
    IPv6,
}

/// What kind of host name `host` is, like .NET's `Uri.CheckHostName`.
pub fn check_host_name(host: &str) -> HostKind {
    if host.is_empty() {
        return HostKind::Unknown;
    }
    if host.parse::<std::net::Ipv4Addr>().is_ok() {
        return HostKind::IPv4;
    }
    let bare = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    if bare.parse::<std::net::Ipv6Addr>().is_ok() {
        return HostKind::IPv6;
    }
    let name = host.strip_suffix('.').unwrap_or(host);
    let label_ok = |l: &str| {
        !l.is_empty() && l.len() <= 63 && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') && !l.starts_with('-')
    };
    if host.len() <= 255 && name.split('.').all(label_ok) {
        HostKind::Dns
    } else {
        HostKind::Unknown
    }
}

/// Prowlarr's `AllowedHostsParser`: the allowed hosts setting as a list.
pub mod allowed_hosts {
    use super::{check_host_name, is_valid_ip_address, HostKind};

    /// The addresses that mean "every address" and are no host names.
    const BIND_ADDRESSES: [&str; 2] = ["0.0.0.0", "[::]"];

    /// The hosts in `value`, split on `,` and `;`, trimmed, IPv6 addresses in brackets, without empty entries or
    /// repeats.
    pub fn parse(value: Option<&str>) -> Vec<String> {
        let Some(value) = value.filter(|v| !v.trim().is_empty()) else { return Vec::new() };
        let mut out: Vec<String> = Vec::new();
        for h in value.split([',', ';']).map(|h| normalize(h.trim())) {
            if !h.trim().is_empty() && !out.contains(&h) {
                out.push(h);
            }
        }
        out
    }

    /// Whether `host` is a host name or address (`*.example.com` for every name under it).
    pub fn is_valid_host(host: Option<&str>) -> bool {
        let Some(host) = host.filter(|h| !h.trim().is_empty()) else { return false };
        if let Some(rest) = host.strip_prefix("*.") {
            return check_host_name(rest) == HostKind::Dns;
        }
        check_host_name(host) != HostKind::Unknown
    }

    /// Whether `host` is 0.0.0.0 or `[::]`.
    pub fn is_bind_address(host: &str) -> bool {
        BIND_ADDRESSES.contains(&host)
    }

    /// An IPv6 address in brackets, anything else as it is.
    fn normalize(host: &str) -> String {
        let bracketed = host.starts_with('[') && host.ends_with(']');
        if !bracketed && host.contains(':') && is_valid_ip_address(host) {
            format!("[{host}]")
        } else {
            host.to_string()
        }
    }
}

/// Prowlarr's `IPNetworkParser`: IP addresses and networks in CIDR notation (`10.0.0.0/8`).
pub mod ip_network {
    use std::net::IpAddr;

    /// The address and prefix length of `value` (a bare address is a single host); `None` when it is not one, or
    /// when the address has bits set past the prefix (`10.1.2.3/8`).
    pub fn try_parse(value: Option<&str>) -> Option<(IpAddr, u8)> {
        let value = value.filter(|v| !v.trim().is_empty())?;
        let parts: Vec<&str> = value.split('/').collect();
        if parts.len() > 2 {
            return None;
        }
        let address_part = parts[0].trim();
        let address: IpAddr = address_part.parse().ok()?;
        if address.is_ipv4() && address_part.matches('.').count() != 3 {
            return None;
        }
        let max = if address.is_ipv6() { 128 } else { 32 };
        if parts.len() == 1 {
            return Some((address, max));
        }
        let prefix: i32 = parts[1].trim().parse().ok()?;
        if prefix < 1 || prefix > max as i32 {
            return None;
        }
        if has_host_bits_set(&address, prefix) {
            return None;
        }
        Some((address, prefix as u8))
    }

    /// Whether every non-empty entry of a comma separated list is an address or network (an empty list is).
    pub fn is_valid_list(value: Option<&str>) -> bool {
        let Some(value) = value.filter(|v| !v.trim().is_empty()) else { return true };
        value.split(',').filter(|e| !e.trim().is_empty()).all(|e| try_parse(Some(e)).is_some())
    }

    /// The list with its entries trimmed and empty ones left out, joined with `, `.
    pub fn normalize_list(value: Option<&str>) -> String {
        let Some(value) = value.filter(|v| !v.trim().is_empty()) else { return String::new() };
        value.split(',').map(str::trim).filter(|e| !e.is_empty()).collect::<Vec<_>>().join(", ")
    }

    /// Whether `address` has bits set past its first `prefix` bits.
    fn has_host_bits_set(address: &IpAddr, prefix: i32) -> bool {
        let bytes: Vec<u8> = match address {
            IpAddr::V4(a) => a.octets().to_vec(),
            IpAddr::V6(a) => a.octets().to_vec(),
        };
        bytes.iter().enumerate().any(|(i, b)| {
            let in_byte = prefix - (i as i32 * 8);
            if in_byte >= 8 {
                return false;
            }
            let host_mask: u8 = if in_byte <= 0 { 0xFF } else { 0xFF >> in_byte };
            b & host_mask != 0
        })
    }
}

/// Prowlarr's `ValidUrlBase`: a path, not an address (`/prowlarr`, never `http://host/prowlarr`).
pub fn is_valid_url_base(value: &str) -> bool {
    let re = regex::Regex::new(r"(?i)^/?https?://[-_a-z0-9.]+").expect("a fixed pattern");
    !re.is_match(value)
}

/// Prowlarr's `IsValidPath` on Linux: an absolute path without a NUL character.
pub fn is_valid_path(value: &str) -> bool {
    !value.trim().is_empty() && !value.contains('\0') && value.starts_with('/')
}

#[cfg(test)]
mod tests {
    // Ported from Prowlarr v2.7.0.5723 (src/NzbDrone.Core.Test/Configuration/AllowedHostsParserFixture.cs,
    // src/NzbDrone.Common.Test/NetworkTests/IPNetworkParserFixture.cs), GPL-3.0.
    use super::*;
    use std::net::IpAddr;

    #[test]
    fn allowed_hosts_empty_for_null_or_empty() {
        for v in [None, Some(""), Some("   ")] {
            assert!(allowed_hosts::parse(v).is_empty());
        }
    }

    #[test]
    fn allowed_hosts_split_on_comma_and_semicolon_and_trim_whitespace() {
        assert_eq!(allowed_hosts::parse(Some("prowlarr.local, 192.168.1.5; myhost")), ["prowlarr.local", "192.168.1.5", "myhost"]);
    }

    #[test]
    fn allowed_hosts_wrap_bare_ipv6_addresses_in_brackets() {
        assert_eq!(allowed_hosts::parse(Some("::1, 2001:db8::1, [fe80::1]")), ["[::1]", "[2001:db8::1]", "[fe80::1]"]);
    }

    #[test]
    fn allowed_hosts_not_duplicate_bare_and_bracketed_ipv6_addresses() {
        assert_eq!(allowed_hosts::parse(Some("::1,[::1]")), ["[::1]"]);
    }

    #[test]
    fn allowed_hosts_remove_empty_entries_and_duplicates() {
        assert_eq!(allowed_hosts::parse(Some("a,,b, a ")), ["a", "b"]);
    }

    #[test]
    fn allowed_hosts_valid_hosts() {
        for h in [
            "prowlarr.local",
            "prowlarr.example.co.uk",
            "my-host",
            "192.168.1.5",
            "[::1]",
            "[2001:db8::1]",
            "2001:db8::1",
            "*.example.com",
            "*.example.co.uk",
            "*.subdomain.example.com",
            "*.subdomain.example.co.uk",
        ] {
            assert!(allowed_hosts::is_valid_host(Some(h)), "{h}");
        }
    }

    #[test]
    fn allowed_hosts_invalid_hosts() {
        for h in [
            None,
            Some(""),
            Some("  "),
            Some("http://prowlarr.local"),
            Some("prowlarr.local:8989"),
            Some("prowlarr local"),
            Some("prowlarr.*"),
            Some("*"),
            Some("*."),
            Some("*.192.168.1.5"),
            Some("*.[::1]"),
            Some("*.*.example.com"),
            Some("[not-an-address]"),
            Some(".example.com"),
            Some(".example.co.uk"),
        ] {
            assert!(!allowed_hosts::is_valid_host(h), "{h:?}");
        }
    }

    #[test]
    fn allowed_hosts_bind_addresses() {
        for h in ["0.0.0.0", "::", "[::]"] {
            let p = allowed_hosts::parse(Some(h));
            assert_eq!(p.len(), 1);
            assert!(allowed_hosts::is_bind_address(&p[0]), "{h}");
        }
        for h in ["*", "sonarr.local", "*.example.com", "192.168.1.5", "[::1]"] {
            assert!(!allowed_hosts::is_bind_address(h), "{h}");
        }
    }

    #[test]
    fn ip_network_parse_cidr_notation() {
        for (v, a, p) in [
            ("10.0.0.0/8", "10.0.0.0", 8),
            ("172.16.0.0/12", "172.16.0.0", 12),
            ("192.168.0.0/16", "192.168.0.0", 16),
            ("10.0.0.5/32", "10.0.0.5", 32),
            ("fc00::/7", "fc00::", 7),
            ("fe80::/10", "fe80::", 10),
            ("2001:db8::1/128", "2001:db8::1", 128),
            ("172.17.0.1", "172.17.0.1", 32),
            ("2001:db8::1", "2001:db8::1", 128),
        ] {
            assert_eq!(ip_network::try_parse(Some(v)), Some((a.parse::<IpAddr>().unwrap(), p)), "{v}");
        }
        for v in [" 10.0.0.0/8 ", "10.0.0.0 / 8", "\t172.17.0.1"] {
            assert!(ip_network::try_parse(Some(v)).is_some(), "{v}");
        }
    }

    #[test]
    fn ip_network_invalid_networks() {
        for v in [
            "",
            "   ",
            "not-an-address",
            "10.0.0.0/",
            "/8",
            "10.0.0.0/8/8",
            "10.0.0.0/eight",
            "10.0.0.0/-1",
            "10.0.0.0/33",
            "fc00::/-1",
            "fc00::/129",
            "10.0.0.0/128",
            "0.0.0.0/0",
            "::/0",
            "10.0.0.0/0",
            "10",
            "10/8",
            "10.1/8",
            "10.1.2/8",
            "10.1.2.3/8",
            "172.17.0.0/12",
            "192.168.1.1/24",
            "fc00::1/7",
            "2001:db8::1/32",
            "example.com",
            "example.com/24",
        ] {
            assert_eq!(ip_network::try_parse(Some(v)), None, "{v}");
        }
        assert_eq!(ip_network::try_parse(None), None);
    }

    #[test]
    fn ip_network_validate_list() {
        for (v, want) in [
            (Some("10.0.0.0/8"), true),
            (Some("10.0.0.0/8,192.168.0.0/16"), true),
            (Some("10.0.0.0/8, 172.17.0.1"), true),
            (None, true),
            (Some(""), true),
            (Some("   "), true),
            (Some("10.0.0.0/8,garbage"), false),
            (Some("garbage"), false),
            (Some("10.0.0.0/8,"), true),
            (Some(",10.0.0.0/8"), true),
            (Some("10.0.0.0/8, ,192.168.0.0/16"), true),
            (Some(",,"), true),
            (Some("0.0.0.0/0"), false),
            (Some("10.0.0.0/8,10.1.2.3/8"), false),
        ] {
            assert_eq!(ip_network::is_valid_list(v), want, "{v:?}");
        }
    }

    #[test]
    fn ip_network_normalize_list() {
        for (v, want) in [
            (Some("10.0.0.0/8,"), "10.0.0.0/8"),
            (Some(",10.0.0.0/8"), "10.0.0.0/8"),
            (Some("10.0.0.0/8,,192.168.0.0/16"), "10.0.0.0/8, 192.168.0.0/16"),
            (Some("  10.0.0.0/8 ,  172.17.0.1  "), "10.0.0.0/8, 172.17.0.1"),
            (Some("10.0.0.0/8"), "10.0.0.0/8"),
            (Some(",,"), ""),
            (None, ""),
            (Some(""), ""),
            (Some("   "), ""),
        ] {
            assert_eq!(ip_network::normalize_list(v), want, "{v:?}");
        }
    }

    #[test]
    fn names_properties_like_fluent_validation() {
        assert_eq!(display_name("SslPort"), "Ssl Port");
        assert_eq!(display_name("UILanguage"), "UI Language");
        assert_eq!(display_name("LogSizeLimit"), "Log Size Limit");
        assert_eq!(display_name("Id"), "Id");
    }

    #[test]
    fn ports_follow_prowlarr() {
        assert!(valid_port("Port", 9696).is_none());
        assert!(valid_port("Port", 80).is_none() && valid_port("Port", 443).is_none());
        assert_eq!(valid_port("Port", 0).unwrap().message, "'Port' must be between 1 and 65535. You entered 0.");
        assert_eq!(valid_port("Port", 22).unwrap().message, "The specified condition was not met for 'Port'.");
        assert_eq!(valid_port("SslPort", 70000).unwrap().code, "InclusiveBetweenValidator");
    }

    #[test]
    fn url_base_is_a_path() {
        assert!(is_valid_url_base("/prowlarr") && is_valid_url_base("") && is_valid_url_base("prowlarr"));
        assert!(!is_valid_url_base("http://host/prowlarr") && !is_valid_url_base("/https://host"));
    }
}
