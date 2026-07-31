//! A whois client — plain TCP on port 43, in-process rather than shelling out to `whois(1)`.
//!
//! The protocol is one line out, free text back ([RFC 3912]), so a client is smaller than the
//! wrapper around an external one would be — and it works where `whois` isn't installed. Two
//! things make it useful rather than merely present:
//!
//! - **Referral chasing.** `whois.iana.org` knows only which registry owns a TLD or address
//!   block; that registry may in turn name the registrar holding the real record. [`lookup`]
//!   follows those hops (bounded), so the answer is the authoritative one rather than a pointer.
//! - **Distillation.** A raw response is mostly legal boilerplate — the ~120 lines a `sed -n
//!   '1,120p'` would print to surface six useful fields. [`distill`] pulls the fields a lookup
//!   actually wants, under one canonical label each, whichever registry spelling was used.
//!
//! [RFC 3912]: https://datatracker.ietf.org/doc/html/rfc3912

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

/// Generous on purpose: a lookup is a CHAIN of TCP round-trips (IANA, then the registry, maybe
/// then the registrar), each to a server that may be far away and rate-limited. A tight timeout
/// here doesn't fail fast — it silently loses the registry record, which is the whole answer.
const TIMEOUT: Duration = Duration::from_secs(12);
/// Where every lookup starts: IANA refers to the registry that actually holds the record.
const ROOT: &str = "whois.iana.org";
/// How many `refer:`/`ReferralServer:` hops to follow. Three covers IANA → registry → registrar;
/// the cap is what keeps a misconfigured (or looping) referral chain from running forever.
const MAX_REFERRALS: usize = 3;

/// The fields worth pulling out of a response, each as `(label, wire keys)`. Registries spell the
/// same fact differently — `inetnum` at RIPE/APNIC, `NetRange` at ARIN, `descr` vs `org-name` vs
/// `Organization` — so one label collects every spelling. Order here is the display order.
const FIELDS: &[(&str, &[&str])] = &[
    ("Range", &["inetnum", "inet6num", "netrange"]),
    ("CIDR", &["cidr", "route", "route6"]),
    ("Network", &["netname"]),
    ("Organization", &["org-name", "organization", "orgname", "owner", "descr"]),
    ("Country", &["country"]),
    // The postal address often names the city a netblock actually serves, when `country` only
    // records where the holder is incorporated.
    ("Address", &["address", "orgaddress", "city"]),
    ("ASN", &["origin", "originas", "aut-num"]),
    ("Registrar", &["registrar"]),
    ("Registered", &["creation date", "created", "registered on", "regdate"]),
    ("Updated", &["updated date", "last-modified", "changed", "updated"]),
    ("Expires", &["registry expiry date", "expiry date", "expires on", "paid-till"]),
    ("Status", &["domain status", "status"]),
    ("DNSSEC", &["dnssec"]),
    ("Abuse contact", &["abuse-mailbox", "orgabuseemail", "registrar abuse contact email"]),
];

/// Ask the whois system about `term` — a domain or an IP — following referrals to the
/// authoritative server. Returns the final response text, or `None` if no server answered.
pub(crate) fn lookup(term: &str) -> Option<String> {
    let mut server = ROOT.to_string();
    let mut seen: Vec<String> = Vec::new();
    let mut answer = None;
    for _ in 0..=MAX_REFERRALS {
        if seen.iter().any(|been| been.eq_ignore_ascii_case(&server)) {
            break; // a referral loop — keep the answer already in hand
        }
        seen.push(server.clone());
        let Some(text) = ask(&server, &query_for(&server, term)) else { break };
        let next = referral(&text);
        answer = Some(text);
        match next {
            Some(next) => server = next,
            None => break,
        }
    }
    answer
}

/// The query line for `term` at `server`. ARIN needs `n + <term>` to return the network record
/// rather than a menu of matches; every other server takes the bare term. (RIPE-style servers
/// accept flags too, but the bare form is what they document.)
fn query_for(server: &str, term: &str) -> String {
    if server.eq_ignore_ascii_case("whois.arin.net") {
        format!("n + {term}")
    } else {
        term.to_string()
    }
}

/// One whois exchange: connect, send `query` + CRLF, read the response to EOF.
fn ask(server: &str, query: &str) -> Option<String> {
    let address = (server, 43u16).to_socket_addrs().ok()?.next()?;
    let mut stream = TcpStream::connect_timeout(&address, TIMEOUT).ok()?;
    stream.set_read_timeout(Some(TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(TIMEOUT)).ok()?;
    stream.write_all(format!("{query}\r\n").as_bytes()).ok()?;
    // Read to EOF: the server closes the connection when the record ends, which is the protocol's
    // only terminator. Non-UTF-8 bytes appear in some records — lossy, never a hard failure.
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).ok()?;
    Some(String::from_utf8_lossy(&raw).into_owned())
}

/// The next server a response points at, if any.
fn referral(text: &str) -> Option<String> {
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        let key = key.trim().to_ascii_lowercase();
        if key == "refer" || key == "referralserver" || key == "whois" || key == "registrar whois server" {
            // ARIN's ReferralServer is a URL (`whois://whois.ripe.net`); the rest are bare hosts.
            let host = value.trim().rsplit('/').next().unwrap_or("").trim();
            let host = host.split(':').next().unwrap_or(host); // drop any :43
            if !host.is_empty() {
                return Some(host.to_string());
            }
        }
    }
    None
}

/// The interesting fields of a response, in [`FIELDS`] order, deduplicated. Comment lines (`%`,
/// `#`) and the "no match" boilerplate are skipped; a field appearing several times (name servers,
/// statuses) keeps every distinct value, joined.
pub(crate) fn distill(text: &str) -> Vec<(&'static str, String)> {
    let mut found: Vec<(&'static str, Vec<String>)> = FIELDS.iter().map(|(l, _)| (*l, Vec::new())).collect();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('%') || line.starts_with('#') || line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else { continue };
        let (key, value) = (key.trim().to_ascii_lowercase(), value.trim());
        if value.is_empty() {
            continue;
        }
        for (slot, (_, keys)) in found.iter_mut().zip(FIELDS) {
            if keys.contains(&key.as_str()) && !slot.1.iter().any(|had| had == value) {
                slot.1.push(value.to_string());
            }
        }
    }
    found
        .into_iter()
        .filter(|(_, values)| !values.is_empty())
        .map(|(label, values)| (label, values.join(", ")))
        .collect()
}

/// The nameservers a domain response lists — kept apart from [`distill`] because they're a set to
/// compare against the live NS records, not a one-line fact.
pub(crate) fn nameservers(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let Some((key, value)) = line.trim().split_once(':') else { continue };
        let key = key.trim().to_ascii_lowercase();
        if key == "name server" || key == "nserver" || key == "nameserver" {
            let host = value.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
            if !host.is_empty() && !out.contains(&host) {
                out.push(host);
            }
        }
    }
    out
}

/// Whether a response is one of the "we have no record" replies, which registries phrase in their
/// own words — worth saying plainly instead of printing an empty field list.
pub(crate) fn is_no_match(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    ["no match for", "not found", "no entries found", "no data found", "object does not exist"]
        .iter()
        .any(|phrase| lowered.contains(phrase))
}

/// The whois server for an address family's registry lookup is found by referral like any other,
/// so an IP needs no special casing — this exists only to keep the caller's intent readable.
pub(crate) fn for_ip(ip: IpAddr) -> Option<String> {
    lookup(&ip.to_string())
}

/// Resolve a whois host to one socket address — split out so a caller can report an unreachable
/// server distinctly from an empty record.
#[allow(dead_code)] // used by the report's diagnostics path
pub(crate) fn resolves(server: &str) -> Option<SocketAddr> {
    (server, 43u16).to_socket_addrs().ok()?.next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn referrals_are_read_from_every_spelling_registries_use() {
        assert_eq!(referral("refer:        whois.verisign-grs.com").as_deref(), Some("whois.verisign-grs.com"));
        // ARIN answers with a URL, not a host.
        assert_eq!(referral("ReferralServer:  whois://whois.ripe.net").as_deref(), Some("whois.ripe.net"));
        assert_eq!(referral("Registrar WHOIS Server: whois.godaddy.com").as_deref(), Some("whois.godaddy.com"));
        // A port suffix is dropped; a record with no referral yields none.
        assert_eq!(referral("refer: whois.nic.io:43").as_deref(), Some("whois.nic.io"));
        assert_eq!(referral("Domain Name: example.com\nRegistrar: Someone"), None);
    }

    #[test]
    fn distilling_collapses_registry_spellings_onto_one_label() {
        // RIPE spells the network `inetnum`/`org-name`; ARIN spells it `NetRange`/`Organization`.
        // Both must land under the same labels, so the report reads the same either way.
        let ripe = "inetnum: 8.8.8.0 - 8.8.8.255\norg-name: Google LLC\ncountry: US\norigin: AS15169";
        let arin = "NetRange: 8.8.8.0 - 8.8.8.255\nOrganization: Google LLC\nCountry: US\nOriginAS: AS15169";
        let (a, b) = (distill(ripe), distill(arin));
        for fields in [&a, &b] {
            assert_eq!(fields.iter().find(|(l, _)| *l == "Range").unwrap().1, "8.8.8.0 - 8.8.8.255");
            assert_eq!(fields.iter().find(|(l, _)| *l == "Organization").unwrap().1, "Google LLC");
            assert_eq!(fields.iter().find(|(l, _)| *l == "ASN").unwrap().1, "AS15169");
        }
    }

    #[test]
    fn distilling_skips_comments_and_keeps_field_order() {
        // The `%`/`#` preamble every RIR ships must not become fields, and the output follows
        // FIELDS order (Range before Organization) regardless of the record's own ordering.
        let text = "% This is the RIPE Database query service.\n\
                    # terms apply\n\
                    org-name: Example Org\n\
                    inetnum: 192.0.2.0 - 192.0.2.255\n";
        let fields = distill(text);
        assert_eq!(fields.iter().map(|(l, _)| *l).collect::<Vec<_>>(), ["Range", "Organization"]);
    }

    #[test]
    fn repeated_fields_keep_every_distinct_value_once() {
        let text = "Domain Status: clientTransferProhibited\n\
                    Domain Status: clientDeleteProhibited\n\
                    Domain Status: clientTransferProhibited\n";
        let status = distill(text).into_iter().find(|(l, _)| *l == "Status").unwrap().1;
        assert_eq!(status, "clientTransferProhibited, clientDeleteProhibited");
    }

    #[test]
    fn nameservers_come_back_lowercased_and_deduplicated() {
        let text = "Name Server: NS1.EXAMPLE.COM\nName Server: ns2.example.com\nnserver: ns1.example.com";
        assert_eq!(nameservers(text), ["ns1.example.com", "ns2.example.com"]);
    }

    #[test]
    fn no_match_replies_are_recognized_across_registries() {
        assert!(is_no_match("No match for \"NOSUCHDOMAIN.COM\"."));
        assert!(is_no_match("%ERROR:101: no entries found"));
        assert!(!is_no_match("Domain Name: example.com"));
    }

    #[test]
    fn arin_gets_the_network_query_form_others_get_the_bare_term() {
        assert_eq!(query_for("whois.arin.net", "8.8.8.8"), "n + 8.8.8.8");
        assert_eq!(query_for("whois.ripe.net", "8.8.8.8"), "8.8.8.8");
    }
}
