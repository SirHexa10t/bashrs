//! The live-connection probes: which TCP ports answer, what certificate a TLS port presents, and
//! where an HTTP URL redirects to.
//!
//! The port check is in-process — a TCP connect is [`TcpStream::connect_timeout`], which is
//! exactly what a connect-scan does, so it needs neither `nmap` nor root. TLS and HTTP are the two
//! places this module shells out: a TLS 1.3 handshake and a full HTTP client are not things to
//! hand-roll, and `openssl`/`curl` are already the tools this project trusts for them (`curl` is
//! how [`crate::categories::download`] fetches). Both are read-only requests to a service that is
//! there to answer them.

use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::support::exec;

/// How long a single TCP connect may take before the port counts as filtered/closed. Short by
/// design: the scan is a "what's obviously open" sweep, not an exhaustive audit.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(700);

/// The ports worth trying, with what they usually mean. Chosen for a diagnostic's question —
/// "what is this host actually serving?" — not for coverage.
pub(crate) const COMMON_PORTS: &[(u16, &str)] = &[
    (21, "ftp"), (22, "ssh"), (23, "telnet"), (25, "smtp"), (53, "dns"), (80, "http"),
    (110, "pop3"), (143, "imap"), (443, "https"), (445, "smb"), (465, "smtps"), (587, "submission"),
    (993, "imaps"), (995, "pop3s"), (1433, "mssql"), (2049, "nfs"), (3000, "dev-http"),
    (3306, "mysql"), (3389, "rdp"), (5432, "postgres"), (5900, "vnc"), (6379, "redis"),
    (8000, "http-alt"), (8080, "http-proxy"), (8443, "https-alt"), (9000, "http-alt"),
    (9200, "elasticsearch"), (27017, "mongodb"),
];

/// Try every port in `ports` against `ip`, in parallel, returning those that accepted a
/// connection. One thread per port: the list is short and each thread spends its life blocked on
/// a socket, so the whole sweep costs about one timeout rather than the sum of them.
pub(crate) fn open_ports(ip: IpAddr, ports: &[(u16, &'static str)]) -> Vec<(u16, &'static str)> {
    let mut open: Vec<(u16, &'static str)> = std::thread::scope(|scope| {
        let handles: Vec<_> = ports
            .iter()
            .map(|&(port, service)| {
                scope.spawn(move || {
                    let address = SocketAddr::new(ip, port);
                    TcpStream::connect_timeout(&address, CONNECT_TIMEOUT)
                        .is_ok()
                        .then_some((port, service))
                })
            })
            .collect();
        handles.into_iter().filter_map(|handle| handle.join().ok().flatten()).collect()
    });
    open.sort_unstable();
    open
}

/// How a port answered a connection attempt. The distinction matters diagnostically: a REFUSED
/// port proves the host is alive and simply isn't serving there, while a FILTERED one means the
/// packets vanished — a firewall, or a host that isn't up at all. Reporting both as "nothing
/// listening" throws away the more informative half.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Reach {
    Open,
    /// The host sent a RST: alive, nothing bound to that port.
    Refused,
    /// No answer within the timeout — dropped by a firewall, or the host is down.
    Filtered,
    /// The name didn't resolve to anything to connect to.
    Unresolved,
}

impl Reach {
    /// The phrase the report uses for a port, given what an ICMP probe said about the host.
    ///
    /// The wording is careful about WHERE the blocking is: a timeout means the packets went out
    /// and nothing came back, which is a fact about the far end (or the path), not about the
    /// reader's own machine — an earlier draft said "filtered by a firewall" and was read, quite
    /// reasonably, as an accusation against a local firewall that wasn't even running.
    pub(crate) fn explain(self, port: u16, alive: Option<bool>) -> String {
        match self {
            Reach::Open => format!("port {port} is open"),
            Reach::Refused => {
                format!("port {port}: connection refused — the host is up and nothing is listening there")
            }
            Reach::Filtered => match alive {
                Some(true) => format!(
                    "port {port}: no response, but the host answers ping — the port is dropped at \
                     the far end (a firewall there, not yours)"
                ),
                Some(false) => format!(
                    "port {port}: no response, and the host doesn't answer ping either — it is \
                     down, or drops everything unsolicited"
                ),
                None => format!(
                    "port {port}: no response — dropped at the far end or along the path, or the \
                     host is down (`ping` isn't available to tell those apart)"
                ),
            },
            Reach::Unresolved => "the name doesn't resolve to any address".to_string(),
        }
    }
}

/// Whether `host` answers an ICMP echo — the tie-breaker between "the port is filtered" and "the
/// host is gone". `None` when `ping` isn't installed, so the caller can say it couldn't tell
/// rather than guessing. Shelled out because unprivileged ICMP needs either a setuid `ping` or a
/// kernel that permits ping sockets, and the system binary already handles whichever applies.
pub(crate) fn host_alive(host: &str) -> Option<bool> {
    if !exec::on_path("ping") {
        return None;
    }
    // One echo, two-second deadline: this is a tie-breaker, not a reachability study.
    Some(exec::succeeds_quietly("ping", ["-c", "1", "-W", "2", host]))
}

/// How `host:port` answers, within [`CONNECT_TIMEOUT`]. The gate in front of the TLS and HTTP
/// probes: `openssl s_client` has no timeout of its own and will sit on an unresponsive port until
/// the OS gives up (minutes), so this cheap check comes first.
pub(crate) fn reach(host: &str, port: u16) -> Reach {
    use std::net::ToSocketAddrs;
    let Ok(addresses) = (host, port).to_socket_addrs() else { return Reach::Unresolved };
    let mut verdict = Reach::Unresolved;
    for address in addresses.take(4) {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(_) => return Reach::Open,
            // A refusal is a definite answer about a live host, so it outranks a silent drop
            // when a name has several addresses.
            Err(err) if err.kind() == std::io::ErrorKind::ConnectionRefused => verdict = Reach::Refused,
            Err(_) if verdict != Reach::Refused => verdict = Reach::Filtered,
            Err(_) => {}
        }
    }
    verdict
}

/// What a TLS port presented.
pub(crate) struct Tls {
    pub(crate) protocol: Option<String>,
    pub(crate) cipher: Option<String>,
    pub(crate) subject: Option<String>,
    pub(crate) issuer: Option<String>,
    /// Every name the certificate is valid for — the field that answers "is this cert actually
    /// for the host I asked about?", and the one the shell script omitted.
    pub(crate) names: Vec<String>,
    pub(crate) not_before: Option<String>,
    pub(crate) not_after: Option<String>,
    /// Serial, signature algorithm, and public-key type/size — the certificate's identity and
    /// the strength of what signed it.
    pub(crate) serial: Option<String>,
    pub(crate) signature: Option<String>,
    pub(crate) key: Option<String>,
    /// The chain the server actually presented, leaf first, as `(subject, issuer)`. A server
    /// that omits its intermediate validates on some clients and fails on others — visible only
    /// by looking at what it sent, not at the leaf alone.
    pub(crate) chain: Vec<(String, String)>,
    /// Days until expiry; negative once expired. `None` when the date couldn't be read.
    pub(crate) days_left: Option<i64>,
    /// openssl's own chain verdict (`ok`, or the reason it refused).
    pub(crate) verify: Option<String>,
}

/// Connect to `host:port` with SNI and report the certificate. `None` when `openssl` isn't
/// installed or the handshake produced nothing to read.
pub(crate) fn tls(host: &str, port: u16) -> Option<Tls> {
    if !exec::on_path("openssl") || reach(host, port) != Reach::Open {
        return None;
    }
    let connect = format!("{host}:{port}");
    // `-servername` is the SNI a virtual host needs to send the right certificate at all.
    let handshake = exec::capture_without_input(
        "openssl",
        ["s_client", "-connect", &connect, "-servername", host, "-showcerts"],
    )?;
    if !handshake.contains("BEGIN CERTIFICATE") {
        return None;
    }
    // The leaf certificate is the first PEM block; feed it back to `openssl x509` for the fields
    // `s_client` doesn't print (validity dates, subjectAltName).
    let leaf = pem_block(&handshake)?;
    let path = std::env::temp_dir().join(format!("bashrs_probe_cert_{}.pem", std::process::id()));
    std::fs::write(&path, &leaf).ok()?;
    let text = exec::capture_without_input(
        "openssl",
        [
            "x509".as_ref(), "-noout".as_ref(), "-subject".as_ref(), "-issuer".as_ref(),
            "-dates".as_ref(), "-serial".as_ref(), "-ext".as_ref(), "subjectAltName".as_ref(),
            // `-text` carries what no dedicated flag prints: the signature algorithm and the
            // public key's type and size.
            "-text".as_ref(), "-in".as_ref(), path.as_os_str(),
        ],
    );
    let _ = std::fs::remove_file(&path);
    let text = text.unwrap_or_default();
    let not_after = field(&text, "notAfter=");
    Some(Tls {
        protocol: labelled(&handshake, "Protocol"),
        cipher: labelled(&handshake, "Cipher"),
        subject: field(&text, "subject="),
        issuer: field(&text, "issuer="),
        names: subject_alt_names(&text),
        not_before: field(&text, "notBefore="),
        serial: field(&text, "serial="),
        signature: indented(&text, "Signature Algorithm:"),
        key: public_key(&text),
        chain: chain(&handshake),
        days_left: not_after.as_deref().and_then(days_until),
        not_after,
        verify: labelled(&handshake, "Verify return code")
            .or_else(|| handshake.lines().find_map(|l| l.strip_prefix("    Verify return code: ").map(str::to_string))),
    })
}

/// The first PEM certificate block of `text`, inclusive of its markers.
fn pem_block(text: &str) -> Option<String> {
    let start = text.find("-----BEGIN CERTIFICATE-----")?;
    let end = text[start..].find("-----END CERTIFICATE-----")? + start;
    Some(text[start..end + "-----END CERTIFICATE-----".len()].to_string())
}

/// The value after a `key=` prefix on its own line (`openssl x509`'s output shape).
fn field(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| line.trim().strip_prefix(key)).map(|v| v.trim().to_string())
}

/// The value of an `s_client` session line (`    Protocol  : TLSv1.3`).
fn labelled(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim().eq_ignore_ascii_case(key).then(|| value.trim().to_string())
    })
}

/// The value after an indented `Key:` label inside `openssl x509 -text` output (the first
/// occurrence — the certificate's own, not a repeat inside an extension).
fn indented(text: &str, key: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.trim().strip_prefix(key))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// The public key as `type, size` — e.g. `rsaEncryption, 2048 bit`. Both halves come from
/// `-text`'s `Subject Public Key Info` block, on separate lines.
fn public_key(text: &str) -> Option<String> {
    let algorithm = indented(text, "Public Key Algorithm:");
    // `Public-Key: (2048 bit)` — the parentheses are openssl's, not ours.
    let size = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("Public-Key:"))
        .map(|value| value.trim().trim_matches(['(', ')']).to_string());
    match (algorithm, size) {
        (Some(algorithm), Some(size)) => Some(format!("{algorithm}, {size}")),
        (Some(only), None) | (None, Some(only)) => Some(only),
        (None, None) => None,
    }
}

/// The certificate chain the server presented, from `s_client`'s listing:
/// ` 0 s:CN=example.com` / `   i:C=US, O=Let\'s Encrypt, CN=R3`, one pair per depth.
fn chain(handshake: &str) -> Vec<(String, String)> {
    let mut chain: Vec<(String, String)> = Vec::new();
    for line in handshake.lines() {
        let line = line.trim();
        if let Some(subject) = line.split_once(" s:").filter(|(depth, _)| depth.chars().all(|c| c.is_ascii_digit())) {
            chain.push((subject.1.to_string(), String::new()));
        } else if let Some(issuer) = line.strip_prefix("i:") {
            if let Some(last) = chain.last_mut() {
                last.1 = issuer.trim().to_string();
            }
        }
    }
    chain
}

/// The DNS names from a printed `X509v3 Subject Alternative Name` extension, deduplicated.
///
/// The dedup is load-bearing, not defensive: the certificate is dumped with BOTH `-ext
/// subjectAltName` and `-text`, and each prints the extension — so a wildcard-heavy certificate
/// (Google's carries ~100 names) came out listed twice, at doubled length.
fn subject_alt_names(text: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for entry in text.lines().filter(|line| line.contains("DNS:")).flat_map(|line| line.split(',')) {
        if let Some(name) = entry.trim().strip_prefix("DNS:") {
            let name = name.trim().to_string();
            if !name.is_empty() && !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

/// Days from now until an openssl date (`Jul 31 12:00:00 2026 GMT`); negative once past. Done by
/// hand because the whole calendar need here is one subtraction — a date crate would be a
/// dependency for eleven lines.
fn days_until(stamp: &str) -> Option<i64> {
    const MONTHS: [&str; 12] =
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let mut parts = stamp.split_whitespace();
    let month_name = parts.next()?;
    let month = MONTHS.iter().position(|m| *m == month_name)? as i64 + 1;
    let day: i64 = parts.next()?.parse().ok()?;
    let _time = parts.next()?;
    let year: i64 = parts.next()?.parse().ok()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64 / 86_400;
    Some(days_from_civil(year, month, day) - now)
}

/// Days since 1970-01-01 for a civil date — Howard Hinnant's `days_from_civil`, the standard
/// branch-free formulation (valid for any Gregorian date, which certificate dates always are).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// One step of a redirect chain.
pub(crate) struct Hop {
    pub(crate) status: String,
    pub(crate) location: Option<String>,
}

/// What one URL's fetch revealed: the redirect chain it walked, and the final response's
/// notable headers as `(name, value)`.
pub(crate) type HttpReport = (Vec<Hop>, Vec<(String, String)>);

/// Follow `url`'s redirects and report the chain plus the final response's notable headers. Uses
/// GET with the body discarded (`-o /dev/null`) rather than HEAD, because a fair number of servers
/// answer HEAD differently — or refuse it — and the point is to see what a browser would get.
pub(crate) fn http_chain(url: &str) -> Option<HttpReport> {
    if !exec::on_path("curl") {
        return None;
    }
    // Same reachability gate as TLS: curl's own timeouts bound it, but waiting 20s to learn
    // nothing listens is time the connect check answers in well under one.
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let port = if scheme == "https" { 443 } else { 80 };
    if reach(host, port) != Reach::Open {
        return None;
    }
    let headers = exec::capture_without_input(
        "curl",
        [
            // 10s, not 5: curl's connect budget includes RESOLVING the name, and on a network
            // with a slow resolver a 5s cap fails the whole probe before a packet is even sent —
            // reporting "no HTTP" about a host that serves fine.
            "-sSL", "-o", "/dev/null", "-D", "-", "--max-redirs", "10",
            "--connect-timeout", "10", "--max-time", "25", url,
        ],
    )?;
    if headers.trim().is_empty() {
        return None;
    }
    Some((hops(&headers), notable_headers(&headers)))
}

/// The status line + `Location` of each response block in a `curl -D -` dump.
fn hops(dump: &str) -> Vec<Hop> {
    let mut hops: Vec<Hop> = Vec::new();
    for line in dump.lines().map(str::trim) {
        if line.starts_with("HTTP/") {
            hops.push(Hop { status: line.to_string(), location: None });
        } else if let Some((key, value)) = line.split_once(':') {
            if key.trim().eq_ignore_ascii_case("location") {
                if let Some(last) = hops.last_mut() {
                    last.location = Some(value.trim().to_string());
                }
            }
        }
    }
    hops
}

/// The headers of the FINAL response worth reporting: what's serving, and which protections it
/// declares. A missing security header is itself the finding, so the caller prints the absences.
pub(crate) const NOTABLE_HEADERS: &[&str] = &[
    "server", "content-type", "content-encoding",
    // What a client can actually negotiate: `alt-svc` is how a server advertises HTTP/3, and
    // the CORS header decides whether a browser-based client may read the response at all —
    // both are the first things someone writing a client needs to know.
    "alt-svc", "access-control-allow-origin",
    "strict-transport-security", "content-security-policy",
    "x-frame-options", "x-content-type-options", "referrer-policy",
    // CDN fingerprints: which edge served this, and whether it was a cache hit.
    "cf-ray", "x-amz-cf-id", "x-served-by", "x-cache", "via",
];

/// The security headers whose ABSENCE is worth reporting. The rest of [`NOTABLE_HEADERS`] are
/// informational — no one expects every server to send `cf-ray`.
pub(crate) const EXPECTED_HEADERS: &[&str] = &[
    "strict-transport-security", "content-security-policy", "x-frame-options",
    "x-content-type-options", "referrer-policy",
];

/// How the connection itself performed, from curl's own instrumentation: DNS, TCP, TLS and
/// time-to-first-byte. The numbers someone choosing between endpoints (or chasing a slow client)
/// actually needs, and free — curl measures them for every request it makes anyway.
pub(crate) struct Timing {
    pub(crate) dns: f64,
    pub(crate) connect: f64,
    pub(crate) tls: f64,
    pub(crate) first_byte: f64,
    pub(crate) total: f64,
    /// The protocol actually negotiated (`2` for HTTP/2, `1.1`, `3`).
    pub(crate) version: String,
    /// The address that actually served it — the only way to see whether the connection went
    /// over IPv4 or IPv6, which a dual-stack client bug depends on entirely.
    pub(crate) remote: String,
}

/// Time one request to `url`. Separate from [`http_chain`] so the header walk stays about
/// headers; both are cheap once the connection is warm.
pub(crate) fn timing(url: &str) -> Option<Timing> {
    let format = "%{time_namelookup} %{time_connect} %{time_appconnect} %{time_starttransfer} %{time_total} %{http_version} %{remote_ip}";
    let out = exec::capture_without_input(
        "curl",
        ["-sSL", "-o", "/dev/null", "-w", format, "--connect-timeout", "10", "--max-time", "25", url],
    )?;
    let parts: Vec<&str> = out.split_whitespace().collect();
    let number = |index: usize| parts.get(index)?.parse::<f64>().ok();
    Some(Timing {
        dns: number(0)?,
        connect: number(1)?,
        // `time_appconnect` is 0 on a plain-HTTP request — there was no handshake to measure.
        tls: number(2).unwrap_or_default(),
        first_byte: number(3)?,
        total: number(4)?,
        version: parts.get(5).copied().unwrap_or("?").to_string(),
        remote: parts.get(6).copied().unwrap_or("?").to_string(),
    })
}

/// Those of [`NOTABLE_HEADERS`] present in the last response block, in that order.
fn notable_headers(dump: &str) -> Vec<(String, String)> {
    let last_block = dump.rsplit("HTTP/").next().unwrap_or(dump);
    NOTABLE_HEADERS
        .iter()
        .filter_map(|wanted| {
            last_block.lines().find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.trim()
                    .eq_ignore_ascii_case(wanted)
                    .then(|| ((*wanted).to_string(), value.trim().to_string()))
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listening_port_is_found_and_a_closed_one_is_not() {
        // A real listener on an ephemeral port proves the connect path end to end; port 1 on
        // loopback stands in for "nothing there" (no privileged listener exists in a test).
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        let open = open_ports(ip, &[(port, "test"), (1, "tcpmux")]);
        assert!(open.contains(&(port, "test")), "the bound port must answer: {open:?}");
        assert!(!open.iter().any(|(p, _)| *p == 1), "nothing listens on port 1: {open:?}");
    }

    #[test]
    fn the_redirect_chain_reads_each_hop_and_its_destination() {
        let dump = "HTTP/1.1 301 Moved Permanently\r\n\
                    Location: https://example.com/\r\n\
                    Server: nginx\r\n\r\n\
                    HTTP/2 200 \r\n\
                    server: cloudflare\r\n\
                    content-type: text/html\r\n\r\n";
        let chain = hops(dump);
        assert_eq!(chain.len(), 2, "two responses, two hops");
        assert_eq!(chain[0].location.as_deref(), Some("https://example.com/"));
        assert!(chain[1].status.contains("200"));
        assert_eq!(chain[1].location, None, "the final hop goes nowhere further");
        // Notable headers come from the LAST block — cloudflare, not nginx.
        let headers = notable_headers(dump);
        assert_eq!(headers.iter().find(|(k, _)| k == "server").unwrap().1, "cloudflare");
    }

    #[test]
    fn certificate_fields_and_sans_parse_from_openssl_output() {
        let text = "subject=CN = example.com\n\
                    issuer=C = US, O = Let's Encrypt, CN = R3\n\
                    notBefore=Jan  1 00:00:00 2026 GMT\n\
                    notAfter=Apr  1 00:00:00 2026 GMT\n\
                    X509v3 Subject Alternative Name: \n\
                    \x20   DNS:example.com, DNS:www.example.com\n";
        assert_eq!(field(text, "subject=").as_deref(), Some("CN = example.com"));
        assert_eq!(subject_alt_names(text), ["example.com", "www.example.com"]);
        assert_eq!(labelled("    Protocol  : TLSv1.3", "Protocol").as_deref(), Some("TLSv1.3"));
    }

    #[test]
    fn the_expiry_countdown_is_signed_around_today() {
        // Anchored on the algorithm, not on wall-clock: two known dates a fixed distance apart.
        assert_eq!(days_from_civil(2026, 1, 1) - days_from_civil(2025, 1, 1), 365);
        assert_eq!(days_from_civil(2024, 3, 1) - days_from_civil(2024, 2, 1), 29, "2024 is a leap year");
        // A date long past reads negative, one far ahead positive — the sign is what the report uses.
        assert!(days_until("Jan 1 00:00:00 2000 GMT").unwrap() < 0, "an expired cert is negative");
        assert!(days_until("Jan 1 00:00:00 2999 GMT").unwrap() > 0);
        assert_eq!(days_until("not a date"), None);
    }

    #[test]
    fn the_leaf_certificate_is_the_first_pem_block() {
        let text = "junk\n-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\nmore\n\
                    -----BEGIN CERTIFICATE-----\nBBBB\n-----END CERTIFICATE-----\n";
        let leaf = pem_block(text).unwrap();
        assert!(leaf.contains("AAAA") && !leaf.contains("BBBB"), "only the leaf: {leaf}");
        assert!(leaf.ends_with("-----END CERTIFICATE-----"));
    }
}
