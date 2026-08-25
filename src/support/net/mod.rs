//! Network-probing primitives behind the `net_*` commands — the engines
//! [`crate::categories::network`] assembles into a report, kept apart from it the same way
//! [`crate::support::treegrep`] sits behind `gg`.
//!
//! - [`dns`] — a UDP DNS client: query a name for a type, decode typed records.
//! - [`rdap`] — the registry lookup proper: JSON over HTTPS, whois's designated successor.
//! - [`whois`] — the TCP/43 fallback for registries (and networks) where RDAP can't be reached.
//! - [`probe`] — live connections: which TCP ports answer, the TLS certificate, the HTTP chain.
//!
//! This module itself owns only what the three share: reading a user's argument into a
//! [`Target`], and saying what kind of address one is — the fact that explains, before any lookup
//! runs, why a private or reserved address will have no public registration and no route.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub(crate) mod bluetooth;
pub(crate) mod dns;
pub(crate) mod lan;
pub(crate) mod netbios;
pub(crate) mod oui;
pub(crate) mod probe;
pub(crate) mod rdap;
pub(crate) mod whois;

/// What the user asked about: an address answers a different set of questions than a name does
/// (no forward DNS, no certificate by name), so the distinction is made once, up front.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum Target {
    Ip(IpAddr),
    Host(String),
}

impl Target {
    /// The text to print for the target — and, for a host, the name every lookup uses.
    pub(crate) fn label(&self) -> String {
        match self {
            Target::Ip(ip) => ip.to_string(),
            Target::Host(host) => host.clone(),
        }
    }
}

/// Read a user's argument into a [`Target`], accepting the three shapes people actually paste: a
/// bare host or address, a `host:port`, and a full URL (scheme, credentials, port, path, query and
/// fragment all discarded — only the host is a lookup subject). Bracketed IPv6 (`[::1]:443`) is
/// unwrapped. `Err` carries what was wrong with it.
pub(crate) fn parse(input: &str) -> Result<Target, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("no target given".to_string());
    }
    // Strip a scheme, then anything from the first path/query/fragment separator, then userinfo.
    let after_scheme = trimmed.split_once("://").map_or(trimmed, |(_, rest)| rest);
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or(after_scheme);
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = match host_port.strip_prefix('[') {
        // Bracketed: everything inside the brackets is the address, whatever follows is the port.
        Some(rest) => rest.split_once(']').map_or(rest, |(inside, _)| inside),
        // A BARE IPv6 address is all colons and hex — `2001:db8::8888` would lose its last group
        // to naive port-stripping (`8888` is all digits). So an address that parses whole is
        // taken whole, and only then is a single trailing `:port` considered.
        None if host_port.parse::<IpAddr>().is_ok() => host_port,
        None => match host_port.rsplit_once(':') {
            Some((head, port))
                if !head.contains(':') && !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) =>
            {
                head
            }
            _ => host_port,
        },
    };
    if host.is_empty() {
        return Err(format!("`{input}` has no host part"));
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(Target::Ip(ip));
    }
    // A name that can't be a name at all — spaces, or nothing but punctuation — is a typo worth
    // catching before four lookups fail obscurely.
    if host.chars().any(char::is_whitespace) || !host.chars().any(|c| c.is_ascii_alphanumeric()) {
        return Err(format!("`{host}` doesn't look like a hostname or an address"));
    }
    Ok(Target::Host(host.trim_end_matches('.').to_ascii_lowercase()))
}

/// What kind of address this is, when it isn't ordinary public space — the one-line explanation
/// for an empty whois, an unrouteable traceroute, or a lookup that can only ever be local.
/// `None` means "public unicast": the normal case, where every section applies.
pub(crate) fn address_note(ip: IpAddr) -> Option<&'static str> {
    match ip {
        IpAddr::V4(v4) => v4_note(v4),
        IpAddr::V6(v6) => v6_note(v6),
    }
}

fn v4_note(ip: Ipv4Addr) -> Option<&'static str> {
    let [a, b, ..] = ip.octets();
    let pair = u16::from(b);
    Some(match () {
        _ if ip.is_unspecified() => "unspecified (0.0.0.0)",
        _ if ip.is_loopback() => "loopback — this machine",
        _ if ip.is_private() => "private (RFC 1918) — no public registration or route",
        _ if ip.is_link_local() => "link-local (169.254/16) — no DHCP was answered",
        _ if a == 100 && (64..128).contains(&pair) => "carrier-grade NAT (100.64/10)",
        _ if a == 192 && b == 0 && ip.octets()[2] == 2 => "documentation range (RFC 5737)",
        _ if a == 198 && b == 51 && ip.octets()[2] == 100 => "documentation range (RFC 5737)",
        _ if a == 203 && b == 0 && ip.octets()[2] == 113 => "documentation range (RFC 5737)",
        _ if a == 198 && (18..20).contains(&pair) => "benchmarking range (RFC 2544)",
        _ if ip.is_broadcast() => "broadcast",
        _ if ip.is_multicast() => "multicast — not a single host",
        _ if a >= 240 => "reserved (240/4)",
        _ => return None,
    })
}

fn v6_note(ip: Ipv6Addr) -> Option<&'static str> {
    let octets = ip.octets();
    Some(match () {
        _ if ip.is_unspecified() => "unspecified (::)",
        _ if ip.is_loopback() => "loopback — this machine",
        // fe80::/10 and fc00::/7 by their leading bits; std's own predicates for these are
        // still unstable, and the masks are the definition anyway.
        _ if octets[0] == 0xfe && octets[1] & 0xc0 == 0x80 => "link-local (fe80::/10)",
        _ if octets[0] & 0xfe == 0xfc => "unique local (fc00::/7) — no public registration",
        _ if octets[0] == 0x20 && octets[1] == 0x01 && octets[2] == 0x0d && octets[3] == 0xb8 => {
            "documentation range (2001:db8::/32)"
        }
        _ if ip.is_multicast() => "multicast — not a single host",
        _ if octets[..12] == [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff] => "IPv4-mapped",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_is_reduced_to_its_host() {
        for input in [
            "https://example.com/path?q=1#frag",
            "http://example.com",
            "example.com/path",
            "example.com:8443",
            "https://user:pw@example.com:443/x",
            "EXAMPLE.com.",
        ] {
            assert_eq!(parse(input).unwrap(), Target::Host("example.com".into()), "input: {input}");
        }
    }

    #[test]
    fn addresses_are_recognized_including_bracketed_and_ported_forms() {
        assert_eq!(parse("8.8.8.8").unwrap(), Target::Ip("8.8.8.8".parse().unwrap()));
        assert_eq!(parse("8.8.8.8:53").unwrap(), Target::Ip("8.8.8.8".parse().unwrap()));
        // A bare IPv6 keeps every colon; a bracketed one drops the port with the brackets.
        let v6: IpAddr = "2001:4860:4860::8888".parse().unwrap();
        assert_eq!(parse("2001:4860:4860::8888").unwrap(), Target::Ip(v6));
        assert_eq!(parse("https://[2001:4860:4860::8888]:443/x").unwrap(), Target::Ip(v6));
    }

    #[test]
    fn nonsense_is_refused_before_any_lookup_runs() {
        assert!(parse("").is_err(), "empty");
        assert!(parse("   ").is_err(), "whitespace only");
        assert!(parse("https://").is_err(), "no host part");
        assert!(parse("two words").is_err(), "a space can't be in a hostname");
        assert!(parse("...").is_err(), "punctuation isn't a name");
    }

    #[test]
    fn special_ranges_explain_themselves() {
        let note = |s: &str| address_note(s.parse().unwrap());
        assert!(note("10.0.0.1").unwrap().contains("private"));
        assert!(note("192.168.1.1").unwrap().contains("private"));
        assert!(note("172.16.0.1").unwrap().contains("private"));
        assert!(note("127.0.0.1").unwrap().contains("loopback"));
        assert!(note("169.254.10.1").unwrap().contains("link-local"));
        assert!(note("100.100.0.1").unwrap().contains("carrier-grade"));
        assert!(note("203.0.113.5").unwrap().contains("documentation"));
        assert!(note("224.0.0.1").unwrap().contains("multicast"));
        assert!(note("::1").unwrap().contains("loopback"));
        assert!(note("fe80::1").unwrap().contains("link-local"));
        assert!(note("fd00::1").unwrap().contains("unique local"));
        assert!(note("2001:db8::1").unwrap().contains("documentation"));
        // Public unicast is the unremarkable case — no note at all.
        assert_eq!(note("8.8.8.8"), None);
        assert_eq!(note("2606:4700:4700::1111"), None);
        // 172.32 is NOT private (the block ends at 172.31) — an easy off-by-one to get wrong.
        assert_eq!(note("172.32.0.1"), None);
    }
}
