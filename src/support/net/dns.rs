//! A minimal DNS client over UDP — the engine behind `net_excavate`'s record sections.
//!
//! In-process rather than shelling out to `dig`, for the same reason the search commands run the
//! `grep` crate instead of `grep(1)`: the answers come back as typed [`Record`]s to render, not as
//! text to re-parse, and the command works on a machine with no `dig` installed. Only what a
//! lookup tool needs is implemented — query one name for one type, follow the answer section —
//! which is a small, well-specified slice of RFC 1035.
//!
//! Deliberately NOT a resolver: no recursion of its own, no cache, no DNSSEC validation. It asks
//! the system's configured nameservers (`/etc/resolv.conf`, falling back to public resolvers) with
//! the recursion-desired bit set and reports what comes back, which is exactly what a diagnostic
//! wants — the answer the user's own network gives.

use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, UdpSocket};
use std::time::{Duration, Instant};

/// How long to wait for one nameserver's reply before trying the next. Short on purpose: a report
/// asks a dozen questions, and `/etc/resolv.conf` often lists servers that no longer answer, so a
/// generous timeout is paid repeatedly. [`lookup_batch`] then overlaps whatever remains.
const TIMEOUT: Duration = Duration::from_secs(2);

/// Public resolvers used only when `/etc/resolv.conf` names none — so the command still works on a
/// container or a misconfigured host instead of reporting a flat "no answer".
const FALLBACK_SERVERS: &[IpAddr] =
    &[IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))];

/// The record types this client can ask for and decode. The numeric values are the wire codes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    A = 1,
    Ns = 2,
    Cname = 5,
    Soa = 6,
    Ptr = 12,
    Mx = 15,
    Txt = 16,
    Aaaa = 28,
    Ds = 43,
    Dnskey = 48,
    Caa = 257,
}

impl Kind {
    /// The label shown in the report (and in `dig`-style output).
    pub(crate) fn label(self) -> &'static str {
        match self {
            Kind::A => "A",
            Kind::Ns => "NS",
            Kind::Cname => "CNAME",
            Kind::Soa => "SOA",
            Kind::Ptr => "PTR",
            Kind::Mx => "MX",
            Kind::Txt => "TXT",
            Kind::Aaaa => "AAAA",
            Kind::Ds => "DS",
            Kind::Dnskey => "DNSKEY",
            Kind::Caa => "CAA",
        }
    }
}

/// One decoded answer. `Other` covers a type we asked about but don't decode in detail (the DNSSEC
/// records, where mere presence is the signal) — it still proves the record exists.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum Record {
    A(Ipv4Addr),
    Aaaa(Ipv6Addr),
    Name(String),                             // CNAME / NS / PTR — a single target name
    Mx { preference: u16, host: String },     // sorted by preference for display
    Txt(String),                              // the character-strings, concatenated
    /// The zone's authority record. The timers matter as much as the serial: they're what a
    /// migration is planned around (how long a secondary caches, how long until it gives up).
    Soa { primary: String, mailbox: String, serial: u32, refresh: u32, retry: u32, expire: u32, minimum: u32 },
    Caa { flags: u8, tag: String, value: String },
    Other,
}

impl Record {
    /// The record rendered the way the report prints it.
    pub(crate) fn render(&self) -> String {
        match self {
            Record::A(ip) => ip.to_string(),
            Record::Aaaa(ip) => ip.to_string(),
            Record::Name(name) => name.clone(),
            // An MX pointing at the root is the "null MX" of RFC 7505 — an explicit statement
            // that the domain receives no mail, which reads as a blank field if taken literally.
            Record::Mx { preference, host } if host.is_empty() => {
                format!("{preference} .  (null MX — this domain accepts no mail)")
            }
            Record::Mx { preference, host } => format!("{preference} {host}"),
            Record::Txt(text) => text.clone(),
            Record::Soa { primary, mailbox, serial, refresh, retry, expire, minimum } => format!(
                "{primary} {mailbox}  serial {serial}, refresh {refresh}s, retry {retry}s, \
                 expire {expire}s, min-ttl {minimum}s"
            ),
            Record::Caa { flags, tag, value } => format!("{flags} {tag} \"{value}\""),
            Record::Other => "(present)".to_string(),
        }
    }

    /// The IP of an A/AAAA record — for the address-derived sections (rDNS, ASN, whois).
    pub(crate) fn ip(&self) -> Option<IpAddr> {
        match self {
            Record::A(ip) => Some(IpAddr::V4(*ip)),
            Record::Aaaa(ip) => Some(IpAddr::V6(*ip)),
            _ => None,
        }
    }
}

/// One answer: the record, and how long a resolver may cache it. TTLs are the first thing a DNS
/// change is planned around — throwing them away made the report useless for exactly the
/// maintenance work it should help with.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Answer {
    pub(crate) ttl: u32,
    pub(crate) record: Record,
}

/// One completed query: the answers, which server produced them, and how long it took. The
/// timing is not decoration — a resolver's latency is the cheapest read on whether a service is
/// healthy or a path is congested, and comparing it ACROSS a zone's nameservers is how an
/// overloaded or distant one gives itself away.
pub(crate) struct Response {
    pub(crate) answers: Vec<Answer>,
    pub(crate) server: IpAddr,
    pub(crate) elapsed: Duration,
    /// Whether the answer came over TCP — because UDP was blocked, or the reply didn't fit in a
    /// datagram. Worth surfacing: it explains both a slower time and a path that works when the
    /// usual one doesn't.
    pub(crate) via_tcp: bool,
}

/// Ask the configured nameservers for `name`'s `kind` records, taking the first that answers.
/// `Ok` with no answers means the server replied and there simply are none (NODATA/NXDOMAIN — a
/// fact worth printing); `Err` means no server could be reached or every reply was unusable.
pub(crate) fn lookup(name: &str, kind: Kind) -> Result<Response, String> {
    let mut last_err = String::from("no nameserver configured");
    for server in servers() {
        match lookup_via(server, name, kind) {
            Ok(response) => return Ok(response),
            Err(err) => last_err = err,
        }
    }
    Err(last_err)
}

/// Ask ONE named server directly, bypassing `/etc/resolv.conf`. This is what makes an
/// authority check possible: putting the same question to each of a zone's own nameservers and
/// comparing what they say (and how fast they say it).
pub(crate) fn lookup_via(server: IpAddr, name: &str, kind: Kind) -> Result<Response, String> {
    let query = build_query(next_id(), name, kind)?;
    let started = Instant::now();
    // UDP first — it's one round-trip and how DNS is normally spoken. TCP is the fallback for the
    // two cases that otherwise end the lookup: a reply too large for a datagram (the TC bit, which
    // RFC 1035 says to retry over TCP), and a network that permits DNS only to its own resolver —
    // blocking UDP/53 outbound while leaving TCP/53 open is a common shape, and it is exactly what
    // stops a zone's own authorities from being queried directly.
    let (reply, via_tcp) = match ask(server, &query) {
        Ok(reply) if !truncated(&reply) => (reply, false),
        udp => match ask_tcp(server, &query) {
            Ok(reply) => (reply, true),
            Err(tcp_err) => {
                return Err(match udp {
                    Ok(_) => format!("{server}: reply truncated, and the TCP retry failed: {tcp_err}"),
                    Err(udp_err) => format!("{server}: {udp_err} (TCP too: {tcp_err})"),
                })
            }
        },
    };
    let elapsed = started.elapsed();
    Ok(Response { answers: parse_answers(&reply, kind)?, server, elapsed, via_tcp })
}

/// Whether a reply has the TC (truncated) bit set — the server's way of saying "this didn't fit;
/// ask again over TCP".
fn truncated(reply: &[u8]) -> bool {
    reply.len() >= 4 && u16::from_be_bytes([reply[2], reply[3]]) & 0x0200 != 0
}

/// The same query over TCP. The wire format is identical, preceded by a two-byte big-endian
/// length — the only difference between DNS over the two transports.
fn ask_tcp(server: IpAddr, query: &[u8]) -> io::Result<Vec<u8>> {
    let length = u16::try_from(query.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "query too long for TCP framing"))?;
    let mut stream = TcpStream::connect_timeout(&SocketAddr::new(server, 53), TIMEOUT)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(query)?;
    stream.flush()?;
    let mut header = [0u8; 2];
    stream.read_exact(&mut header)?;
    let mut reply = vec![0u8; usize::from(u16::from_be_bytes(header))];
    stream.read_exact(&mut reply)?;
    Ok(reply)
}

/// Run several lookups at once, returning their answers in the order asked.
///
/// The queries are independent network round-trips, so running them in sequence pays every
/// timeout in turn — with a handful of record types and a resolver that has stopped answering,
/// that's the difference between a report appearing in seconds and one that looks hung. One
/// thread per query: each spends its whole life blocked on a socket.
pub(crate) fn lookup_batch(queries: &[(String, Kind)]) -> Vec<Result<Response, String>> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = queries
            .iter()
            .map(|(name, kind)| scope.spawn(move || lookup(name, *kind)))
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap_or_else(|_| Err("lookup thread panicked".to_string())))
            .collect()
    })
}

/// A label no cache can already hold — for measuring what a lookup really costs.
///
/// A resolver on loopback (systemd-resolved's `127.0.0.53`, dnsmasq) answers a popular name from
/// memory in well under a millisecond, so timing that says nothing about the network. Asking for
/// a name that cannot exist forces the full path: stub → recursor → the zone's authorities.
/// Derived from the clock and the pid, so consecutive runs don't reuse a label either.
pub(crate) fn uncacheable_label() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.subsec_nanos())
        .unwrap_or_default();
    format!("bashrs-probe-{:x}{:x}", std::process::id(), nanos)
}

/// Ask the LAN itself what an address calls itself — a one-shot **mDNS** reverse lookup.
///
/// This is the name protocol a home network actually speaks. Unicast DNS only knows a device if
/// the router happened to register a PTR record for its DHCP lease, which most do not; mDNS is how
/// Apple devices, printers, Chromecasts, NASes and anything running Avahi announce themselves, and
/// it needs no server at all — the device answers for itself.
///
/// Deliberately reusing this module's packet code: mDNS is DNS on the wire, byte for byte. Only
/// three things differ, and all three are why a plain [`lookup`] can't be pointed at it:
/// - it goes to the multicast group `224.0.0.251:5353` rather than a configured resolver;
/// - the question's class carries the **QU bit** (`0x8001`), asking the responder to reply
///   directly to our ephemeral port instead of multicasting the answer to the whole segment.
///   That also makes the answer trustworthy: an mDNS network is full of unsolicited
///   announcements, but those go to port 5353, which this socket is deliberately NOT bound to —
///   so the only packets it can receive are replies addressed to this query. The cost is that a
///   responder which ignores the QU bit and multicasts anyway is missed, which is a lost name
///   rather than a wrong one;
/// - recursion-desired is not set, because there is nobody to recurse.
pub(crate) fn mdns_reverse(ip: IpAddr, source: Option<Ipv4Addr>, timeout: Duration) -> Option<String> {
    let mut query = build_query(0, &reverse_name(ip), Kind::Ptr).ok()?;
    let length = query.len();
    query[2..4].copy_from_slice(&0u16.to_be_bytes()); // no recursion-desired: nobody to recurse
    query[length - 2..].copy_from_slice(&0x8001u16.to_be_bytes()); // class IN + the QU bit

    let socket = multicast_socket(source).ok()?;
    socket.set_read_timeout(Some(timeout)).ok()?;
    socket.send_to(&query, (MDNS_GROUP, MDNS_PORT)).ok()?;

    // Several devices may answer a multicast question; take the first reply that actually carries
    // a PTR for what we asked, and stop. A machine with nothing to say simply never answers, so
    // the timeout IS the negative result — it is kept short by the caller for that reason.
    let deadline = Instant::now() + timeout;
    let mut buffer = [0u8; 1500];
    while Instant::now() < deadline {
        let Ok((read, _)) = socket.recv_from(&mut buffer) else { return None };
        if let Ok(answers) = parse_answers(&buffer[..read], Kind::Ptr) {
            if let Some(Record::Name(name)) = answers.first().map(|answer| &answer.record) {
                let name = name.trim_end_matches('.');
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
    }
    None
}

/// Ask the whole segment to introduce itself, and keep every address→name pair that comes back.
///
/// [`mdns_reverse`] asks "who owns this address?", which only **Avahi** reliably answers. Embedded
/// responders — Chromecasts, printers, AV receivers, most smart speakers — never implement reverse
/// PTR at all. What they *do* answer is a service query, and by convention every mDNS reply
/// carries the responder's own `A` record alongside, so that a client learns the address without a
/// second round trip. Harvesting those `A` records is therefore how a name is actually obtained
/// from such a device, and it is what `avahi-browse` and `dns-sd -B` are doing underneath.
///
/// One burst of queries for the whole network, not one per address: a multicast question is heard
/// by everything, so asking per-device would be the same packets many times over.
pub(crate) fn mdns_browse(source: Option<Ipv4Addr>, timeout: Duration) -> Vec<(Ipv4Addr, String)> {
    let Ok(socket) = multicast_socket(source) else { return Vec::new() };
    if socket.set_read_timeout(Some(READ_SLICE)).is_err() {
        return Vec::new();
    }
    for service in BROWSE_SERVICES {
        let Ok(mut query) = build_query(0, service, Kind::Ptr) else { continue };
        let length = query.len();
        query[2..4].copy_from_slice(&0u16.to_be_bytes());
        query[length - 2..].copy_from_slice(&0x8001u16.to_be_bytes()); // QU: answer me directly
        let _ = socket.send_to(&query, (MDNS_GROUP, MDNS_PORT));
    }
    // Collect until the budget runs out. Devices answer at their own pace (and some deliberately
    // jitter to avoid a multicast storm), so this is a listening window rather than a round trip.
    let deadline = Instant::now() + timeout;
    let mut found: Vec<(Ipv4Addr, String)> = Vec::new();
    let mut buffer = [0u8; 4096];
    while Instant::now() < deadline {
        let Ok((read, _)) = socket.recv_from(&mut buffer) else { continue };
        harvest_addresses(&buffer[..read], &mut found);
    }
    found.sort();
    found.dedup();
    found
}

/// How long a single `recv` waits before the loop re-checks its deadline. Short enough that the
/// overall budget is honoured closely, long enough not to spin.
const READ_SLICE: Duration = Duration::from_millis(120);

/// The service types worth asking about — chosen for what actually sits on a home network and
/// answers. The first is the DNS-SD meta-query ("what service types exist here?"), which many
/// responders answer with their own address attached; the rest are the households names most
/// likely to be present.
const BROWSE_SERVICES: &[&str] = &[
    "_services._dns-sd._udp.local",
    "_googlecast._tcp.local",   // Chromecast, Google/Nest speakers and displays
    "_airplay._tcp.local",      // Apple TV, AirPlay receivers
    "_raop._tcp.local",         // AirPlay audio (also Sonos, many AV receivers)
    "_spotify-connect._tcp.local",
    "_ipp._tcp.local",          // printers
    "_printer._tcp.local",
    "_smb._tcp.local",          // file sharing
    "_afpovertcp._tcp.local",
    "_workstation._tcp.local",  // Avahi hosts announce themselves here
    "_device-info._tcp.local",
    "_hap._tcp.local",          // HomeKit accessories
];

/// Pull every `A` record out of a reply, whichever section it sits in.
///
/// Deliberately lenient: this reads unsolicited and third-party mDNS traffic, where a strict
/// parser's job (rejecting malformed messages) is the wrong instinct — a record that reads cleanly
/// is worth keeping even if something later in the datagram does not. Anything unparseable simply
/// ends the walk.
fn harvest_addresses(reply: &[u8], into: &mut Vec<(Ipv4Addr, String)>) {
    if reply.len() < 12 {
        return;
    }
    let count = |at: usize| u16::from_be_bytes([reply[at], reply[at + 1]]) as usize;
    let (questions, records) = (count(4), count(6) + count(8) + count(10));
    let mut pos = 12;
    for _ in 0..questions {
        let Ok(next) = skip_name(reply, pos) else { return };
        pos = next + 4; // QTYPE + QCLASS
    }
    for _ in 0..records {
        // The owner NAME is what we are after, so it is read rather than skipped.
        let Ok((name, next)) = read_name(reply, pos) else { return };
        let Ok((rtype, _ttl, rdlength, data_start)) = record_header(reply, next) else { return };
        let end = data_start + rdlength;
        if end > reply.len() {
            return;
        }
        // Type 1 is `A`; four bytes of address.
        if rtype == 1 && rdlength == 4 {
            let address = Ipv4Addr::new(
                reply[data_start],
                reply[data_start + 1],
                reply[data_start + 2],
                reply[data_start + 3],
            );
            let name = name.trim_end_matches('.').to_string();
            if !name.is_empty() {
                into.push((address, name));
            }
        }
        pos = end;
    }
}

/// A UDP socket that will send multicast out a CHOSEN interface.
///
/// This matters more than it looks. A multicast datagram leaves by exactly one interface, and with
/// nothing specified the kernel picks it from the route to `224.0.0.0/4` — one global choice, made
/// without reference to the network being scanned. On a machine with several interfaces (a laptop
/// with Docker bridges has six) the query can therefore go out a bridge and never touch the real
/// LAN, while the host's own responder still answers because it listens everywhere. The result is
/// a scan that names the local machine and nothing else, which looks like "the devices don't
/// support mDNS" and is not.
///
/// `IP_MULTICAST_IF` is the only way to say which interface; the standard library does not expose
/// it, so this is the one place the crate reaches for `setsockopt`. Binding the source address as
/// well keeps replies coming back to the same interface.
fn multicast_socket(source: Option<Ipv4Addr>) -> std::io::Result<UdpSocket> {
    let Some(source) = source else {
        // No known address on this network: fall back to the kernel's choice, which is at least
        // as good as it was before, and still works on a single-homed machine.
        return UdpSocket::bind(("0.0.0.0", 0));
    };
    let socket = UdpSocket::bind((source, 0))?;
    // `s_addr` holds the address in NETWORK byte order, which is exactly the octet order
    // `Ipv4Addr::octets` yields — so `from_ne_bytes` lays the right bytes in memory on either
    // endianness.
    let request = libc::in_addr { s_addr: u32::from_ne_bytes(source.octets()) };
    let set = unsafe {
        libc::setsockopt(
            std::os::fd::AsRawFd::as_raw_fd(&socket),
            libc::IPPROTO_IP,
            libc::IP_MULTICAST_IF,
            std::ptr::addr_of!(request).cast(),
            std::mem::size_of::<libc::in_addr>() as libc::socklen_t,
        )
    };
    match set == 0 {
        true => Ok(socket),
        false => Err(std::io::Error::last_os_error()),
    }
}

/// The IPv4 multicast group every mDNS responder listens on, and its port.
const MDNS_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const MDNS_PORT: u16 = 5353;

/// The reverse-DNS name for an address (`1.2.3.4` → `4.3.2.1.in-addr.arpa`, and the nibble form
/// for IPv6) — the name a PTR lookup asks about.
pub(crate) fn reverse_name(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, c, d] = v4.octets();
            format!("{d}.{c}.{b}.{a}.in-addr.arpa")
        }
        IpAddr::V6(v6) => {
            let mut out = String::with_capacity(72);
            for byte in v6.octets().iter().rev() {
                out.push_str(&format!("{:x}.{:x}.", byte & 0x0f, byte >> 4));
            }
            out.push_str("ip6.arpa");
            out
        }
    }
}

/// Who announces `ip` to the internet — the fact that answers "whose network is this really?",
/// which a hostname never tells you.
pub(crate) struct Asn {
    pub(crate) number: String,
    pub(crate) prefix: String,
    /// Where the AS is REGISTERED. Not a geolocation: a UK-registered network routinely
    /// announces addresses used on another continent.
    pub(crate) country: String,
    /// The AS's name, from a second lookup. `None` when that lookup didn't answer — worth
    /// distinguishing from an AS that genuinely has no name, and from silently printing nothing.
    pub(crate) name: Option<String>,
}

/// Look up [`Asn`] for `ip`.
///
/// Team Cymru publishes the routing table's origin data *as DNS TXT records*, so this needs no
/// second protocol and no API key — which is why it lives with the DNS client rather than beside
/// [`super::whois`]. Two lookups: the prefix's origin, then that AS's name.
pub(crate) fn asn(ip: IpAddr) -> Option<Asn> {
    let (labels, zone) = match ip {
        IpAddr::V4(v4) => {
            let [a, b, c, d] = v4.octets();
            (format!("{d}.{c}.{b}.{a}"), "origin.asn.cymru.com")
        }
        // The same nibble expansion PTR uses, minus the `ip6.arpa` suffix.
        IpAddr::V6(_) => {
            let full = reverse_name(ip);
            (full.trim_end_matches(".ip6.arpa").to_string(), "origin6.asn.cymru.com")
        }
    };
    let origin = first_txt(&format!("{labels}.{zone}"))?;
    // `15169 | 8.8.8.0/24 | US | arin | 1992-12-01`
    let parts: Vec<&str> = origin.split('|').map(str::trim).collect();
    let (number, prefix, country) = (
        parts.first().copied().unwrap_or_default().to_string(),
        parts.get(1).copied().unwrap_or_default().to_string(),
        parts.get(2).copied().unwrap_or_default().to_string(),
    );
    if number.is_empty() {
        return None;
    }
    // `15169 | US | arin | 1992-12-01 | GOOGLE, US` — the name is the last field. A failed or
    // empty answer stays `None` rather than becoming an empty string the report would print as
    // a stray blank after the AS number.
    let name = first_txt(&format!("AS{number}.asn.cymru.com"))
        .and_then(|text| text.rsplit('|').next().map(|name| name.trim().to_string()))
        .filter(|name| !name.is_empty());
    Some(Asn { number, prefix, country, name })
}

/// The first TXT record of `name`, if any — the shape every Cymru answer takes.
fn first_txt(name: &str) -> Option<String> {
    lookup(name, Kind::Txt).ok()?.answers.into_iter().find_map(|answer| match answer.record {
        Record::Txt(text) => Some(text),
        _ => None,
    })
}

/// The nameservers to ask: every `nameserver` line in `/etc/resolv.conf`, else [`FALLBACK_SERVERS`].
fn servers() -> Vec<IpAddr> {
    let configured: Vec<IpAddr> = std::fs::read_to_string("/etc/resolv.conf")
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split_once("nameserver")?.1.trim().split('%').next())
        .filter_map(|addr| addr.trim().parse().ok())
        .collect();
    if configured.is_empty() {
        FALLBACK_SERVERS.to_vec()
    } else {
        configured
    }
}

/// Send one query and read one reply. The socket binds to the wildcard address matching the
/// server's family, so IPv6-only resolvers work too.
fn ask(server: IpAddr, query: &[u8]) -> io::Result<Vec<u8>> {
    let bind: SocketAddr = if server.is_ipv4() { "0.0.0.0:0".parse() } else { "[::]:0".parse() }
        .expect("literal bind address");
    let socket = UdpSocket::bind(bind)?;
    socket.set_read_timeout(Some(TIMEOUT))?;
    socket.send_to(query, SocketAddr::new(server, 53))?;
    // 4096 covers any UDP reply an EDNS-less query can produce (512 by RFC, more in practice);
    // a longer answer sets the TC bit, which `parse_answers` reports rather than silently truncating.
    let mut buf = vec![0u8; 4096];
    let (read, _) = socket.recv_from(&mut buf)?;
    buf.truncate(read);
    Ok(buf)
}

/// A per-query transaction ID. Not cryptographic — this is a diagnostic client on the user's own
/// network, and the reply is matched by socket anyway; it only needs to vary between queries.
fn next_id() -> u16 {
    use std::sync::atomic::{AtomicU16, Ordering};
    static NEXT: AtomicU16 = AtomicU16::new(0);
    let seed = std::process::id() as u16;
    seed ^ NEXT.fetch_add(0x9e37, Ordering::Relaxed)
}

/// Encode a standard recursive query for one name and type.
fn build_query(id: u16, name: &str, kind: Kind) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(32);
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&0x0100u16.to_be_bytes()); // flags: standard query, recursion desired
    out.extend_from_slice(&1u16.to_be_bytes()); // one question
    out.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // no answer/authority/additional records
    encode_name(name, &mut out)?;
    out.extend_from_slice(&(kind as u16).to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes()); // class IN
    Ok(out)
}

/// A domain name as length-prefixed labels, terminated by a zero byte. Rejects the label lengths
/// the wire format can't express, so a malformed argument fails here rather than on the wire.
fn encode_name(name: &str, out: &mut Vec<u8>) -> Result<(), String> {
    for label in name.trim_end_matches('.').split('.').filter(|label| !label.is_empty()) {
        if label.len() > 63 {
            return Err(format!("label too long in `{name}` (max 63 bytes): {label}"));
        }
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    Ok(())
}

/// Decode the answer section, keeping the records of the type asked for. A truncated reply (TC) is
/// reported: the answer would need TCP, and a half-list is worse than an honest note.
fn parse_answers(reply: &[u8], kind: Kind) -> Result<Vec<Answer>, String> {
    if reply.len() < 12 {
        return Err("reply too short to be a DNS message".to_string());
    }
    let flags = u16::from_be_bytes([reply[2], reply[3]]);
    if flags & 0x0200 != 0 {
        // `lookup_via` retries over TCP before reaching this, so a truncated reply here means a
        // caller parsed one directly.
        return Err("reply truncated (needs TCP)".to_string());
    }
    match flags & 0x000f {
        0 | 3 => {} // NOERROR, or NXDOMAIN — both mean "asked and answered", possibly with nothing
        2 => return Err("server failure".to_string()),
        5 => return Err("query refused".to_string()),
        code => return Err(format!("server returned rcode {code}")),
    }
    let questions = u16::from_be_bytes([reply[4], reply[5]]);
    let answers = u16::from_be_bytes([reply[6], reply[7]]);
    let mut pos = 12;
    for _ in 0..questions {
        pos = skip_name(reply, pos)?;
        pos += 4; // QTYPE + QCLASS
    }
    let mut records: Vec<Answer> = Vec::new();
    for _ in 0..answers {
        pos = skip_name(reply, pos)?;
        let (rtype, ttl, rdlength, data_start) = record_header(reply, pos)?;
        let end = data_start + rdlength;
        if end > reply.len() {
            return Err("record data runs past the end of the reply".to_string());
        }
        if rtype == kind as u16 {
            if let Some(record) = decode_rdata(reply, data_start, end, kind)? {
                records.push(Answer { ttl, record });
            }
        }
        pos = end;
    }
    if kind == Kind::Mx {
        records.sort_by_key(|answer| match answer.record {
            Record::Mx { preference, .. } => preference,
            _ => u16::MAX,
        });
    }
    Ok(records)
}

/// `(type, ttl, rdlength, offset of the data)` from a record header sitting at `pos`.
fn record_header(reply: &[u8], pos: usize) -> Result<(u16, u32, usize, usize), String> {
    if pos + 10 > reply.len() {
        return Err("record header runs past the end of the reply".to_string());
    }
    let rtype = u16::from_be_bytes([reply[pos], reply[pos + 1]]);
    let ttl = u32::from_be_bytes([reply[pos + 4], reply[pos + 5], reply[pos + 6], reply[pos + 7]]);
    let rdlength = u16::from_be_bytes([reply[pos + 8], reply[pos + 9]]) as usize;
    Ok((rtype, ttl, rdlength, pos + 10))
}

/// Turn one record's RDATA into a [`Record`]. `Ok(None)` drops a record whose data doesn't fit its
/// type — a malformed answer shouldn't abort the whole lookup.
fn decode_rdata(reply: &[u8], start: usize, end: usize, kind: Kind) -> Result<Option<Record>, String> {
    let data = &reply[start..end];
    Ok(match kind {
        Kind::A => <[u8; 4]>::try_from(data).ok().map(|o| Record::A(Ipv4Addr::from(o))),
        Kind::Aaaa => <[u8; 16]>::try_from(data).ok().map(|o| Record::Aaaa(Ipv6Addr::from(o))),
        Kind::Cname | Kind::Ns | Kind::Ptr => Some(Record::Name(read_name(reply, start)?.0)),
        Kind::Mx => {
            if data.len() < 3 {
                None
            } else {
                let preference = u16::from_be_bytes([data[0], data[1]]);
                Some(Record::Mx { preference, host: read_name(reply, start + 2)?.0 })
            }
        }
        // TXT rdata is a sequence of length-prefixed strings; a long record arrives split into
        // 255-byte chunks that mean nothing individually, so they're rejoined.
        Kind::Txt => {
            let mut text = String::new();
            let mut at = 0;
            while at < data.len() {
                let len = data[at] as usize;
                at += 1;
                if at + len > data.len() {
                    break;
                }
                text.push_str(&String::from_utf8_lossy(&data[at..at + len]));
                at += len;
            }
            Some(Record::Txt(text))
        }
        Kind::Soa => {
            let (primary, after_primary) = read_name(reply, start)?;
            let (mailbox, at) = read_name(reply, after_primary)?;
            // Five u32s follow the two names: serial, refresh, retry, expire, minimum.
            let word = |index: usize| {
                reply
                    .get(at + index * 4..at + index * 4 + 4)
                    .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
                    .map(u32::from_be_bytes)
                    .unwrap_or_default()
            };
            Some(Record::Soa {
                primary,
                mailbox,
                serial: word(0),
                refresh: word(1),
                retry: word(2),
                expire: word(3),
                minimum: word(4),
            })
        }
        Kind::Caa => {
            if data.len() < 2 {
                None
            } else {
                let tag_len = data[1] as usize;
                (2 + tag_len <= data.len()).then(|| Record::Caa {
                    flags: data[0],
                    tag: String::from_utf8_lossy(&data[2..2 + tag_len]).into_owned(),
                    value: String::from_utf8_lossy(&data[2 + tag_len..]).into_owned(),
                })
            }
        }
        // Presence is the whole signal for these (the DNSSEC pair) — the key material itself
        // isn't something a lookup report can use.
        Kind::Ds | Kind::Dnskey => Some(Record::Other),
    })
}

/// Read a (possibly compressed) name at `pos`, returning it and the offset just past the name AS
/// ENCODED HERE — a pointer occupies two bytes no matter how long the name it names.
fn read_name(reply: &[u8], pos: usize) -> Result<(String, usize), String> {
    let mut labels: Vec<String> = Vec::new();
    let mut at = pos;
    let mut after: Option<usize> = None;
    // A compression pointer may point to another pointer; a corrupt reply could make that a cycle,
    // so the walk is bounded by the message length (each hop must move strictly backwards in a
    // well-formed message, and can never legitimately exceed one hop per byte).
    for _ in 0..reply.len() {
        match reply.get(at) {
            None => return Err("name runs past the end of the reply".to_string()),
            Some(0) => {
                let end = after.unwrap_or(at + 1);
                return Ok((labels.join("."), end));
            }
            Some(&len) if len & 0xc0 == 0xc0 => {
                let Some(&low) = reply.get(at + 1) else {
                    return Err("truncated compression pointer".to_string());
                };
                after.get_or_insert(at + 2);
                at = (((len & 0x3f) as usize) << 8) | low as usize;
            }
            Some(&len) => {
                let len = len as usize;
                let Some(label) = reply.get(at + 1..at + 1 + len) else {
                    return Err("label runs past the end of the reply".to_string());
                };
                labels.push(String::from_utf8_lossy(label).into_owned());
                at += 1 + len;
            }
        }
    }
    Err("compression pointer loop".to_string())
}

/// The offset just past the name at `pos`, without decoding it.
fn skip_name(reply: &[u8], pos: usize) -> Result<usize, String> {
    read_name(reply, pos).map(|(_, end)| end)
}

#[cfg(test)]
mod tests {
    /// The record harvest is what names a device that never answers a reverse lookup. It must
    /// read `A` records out of ANY section — a Chromecast puts its address in ADDITIONAL, beside
    /// the service record that was actually asked for.
    #[test]
    fn addresses_are_harvested_from_every_section_of_a_reply() {
        use super::*;
        fn encode(name: &str) -> Vec<u8> {
            let mut out = Vec::new();
            for label in name.split('.') {
                out.push(label.len() as u8);
                out.extend_from_slice(label.as_bytes());
            }
            out.push(0);
            out
        }
        // A reply shaped like a real one: one PTR answer, one A record in ADDITIONAL.
        let mut reply = vec![0x00, 0x00, 0x84, 0x00];
        reply.extend_from_slice(&0u16.to_be_bytes()); // questions
        reply.extend_from_slice(&1u16.to_be_bytes()); // answers
        reply.extend_from_slice(&0u16.to_be_bytes()); // authority
        reply.extend_from_slice(&1u16.to_be_bytes()); // ADDITIONAL — where the address hides
        let instance = encode("Front-Room._googlecast._tcp.local");
        reply.extend_from_slice(&encode("_googlecast._tcp.local"));
        reply.extend_from_slice(&12u16.to_be_bytes()); // PTR
        reply.extend_from_slice(&1u16.to_be_bytes());
        reply.extend_from_slice(&120u32.to_be_bytes());
        reply.extend_from_slice(&(instance.len() as u16).to_be_bytes());
        reply.extend_from_slice(&instance);
        reply.extend_from_slice(&encode("Front-Room.local"));
        reply.extend_from_slice(&1u16.to_be_bytes()); // A
        reply.extend_from_slice(&1u16.to_be_bytes());
        reply.extend_from_slice(&120u32.to_be_bytes());
        reply.extend_from_slice(&4u16.to_be_bytes());
        reply.extend_from_slice(&[10, 0, 0, 4]);

        let mut found = Vec::new();
        harvest_addresses(&reply, &mut found);
        assert_eq!(found, [(Ipv4Addr::new(10, 0, 0, 4), "Front-Room.local".to_string())]);

        // Unsolicited and damaged traffic arrives on the same socket; neither may panic, and a
        // truncation must simply end the walk with whatever was already read cleanly.
        harvest_addresses(&[], &mut Vec::new());
        harvest_addresses(&[0; 12], &mut Vec::new());
        for cut in 0..reply.len() {
            let mut partial = Vec::new();
            harvest_addresses(&reply[..cut], &mut partial);
            assert!(partial.len() <= 1, "a truncated reply cannot invent records");
        }
    }

    /// The three bytes that make a DNS query an mDNS query. Checked on the wire, because each one
    /// is silently survivable: without the QU bit the answer is multicast and never reaches this
    /// socket, and with recursion-desired set some responders ignore the packet outright.
    #[test]
    fn an_mdns_query_is_a_dns_query_with_three_changes() {
        use super::*;
        let ip: IpAddr = "192.168.1.42".parse().unwrap();
        let name = reverse_name(ip);
        assert_eq!(name, "42.1.168.192.in-addr.arpa", "the question is a reverse PTR");

        // Rebuild exactly what `mdns_reverse` sends.
        let mut query = build_query(0, &name, Kind::Ptr).expect("query");
        let length = query.len();
        query[2..4].copy_from_slice(&0u16.to_be_bytes());
        query[length - 2..].copy_from_slice(&0x8001u16.to_be_bytes());

        assert_eq!(&query[0..2], &[0, 0], "mDNS one-shot queries carry no transaction id");
        assert_eq!(&query[2..4], &[0, 0], "recursion-desired must be clear — nobody to recurse");
        assert_eq!(&query[4..6], &[0, 1], "exactly one question");
        assert_eq!(&query[length - 4..length - 2], &[0, 12], "type PTR (12)");
        assert_eq!(&query[length - 2..], &[0x80, 0x01], "class IN with the QU bit set");
        // And the name really is encoded in it, label by label.
        assert!(query.windows(7).any(|w| w == b"in-addr"), "the question name is present");
    }

    use super::*;

    /// Assemble a DNS reply: 12-byte header (one question, `answers` answers), the question, then
    /// the raw answer bytes — the shape every parser test needs.
    fn reply(answers: u16, question: &[u8], records: &[u8]) -> Vec<u8> {
        let mut out = vec![0x12, 0x34, 0x81, 0x80]; // id, then QR+RD+RA, rcode 0
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&answers.to_be_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(question);
        out.extend_from_slice(records);
        out
    }

    /// Just the records of a parsed reply — the TTL is asserted separately, so the shape tests
    /// stay about the decoding.
    fn records(reply: &[u8], kind: Kind) -> Vec<Record> {
        parse_answers(reply, kind).unwrap().into_iter().map(|answer| answer.record).collect()
    }

    /// `example.com` encoded, followed by QTYPE/QCLASS — the standard question body.
    fn question(kind: Kind) -> Vec<u8> {
        let mut out = Vec::new();
        encode_name("example.com", &mut out).unwrap();
        out.extend_from_slice(&(kind as u16).to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out
    }

    /// One answer record: a compression pointer to the question's name, then type/class/ttl/rdata.
    fn answer(kind: Kind, rdata: &[u8]) -> Vec<u8> {
        let mut out = vec![0xc0, 0x0c]; // pointer to offset 12 — the question's name
        out.extend_from_slice(&(kind as u16).to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&300u32.to_be_bytes());
        out.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        out.extend_from_slice(rdata);
        out
    }

    #[test]
    fn a_query_is_a_standard_recursive_question() {
        let query = build_query(0x1234, "example.com.", Kind::A).unwrap();
        assert_eq!(&query[0..2], &[0x12, 0x34], "the transaction id leads");
        assert_eq!(&query[2..4], &[0x01, 0x00], "recursion desired, nothing else set");
        assert_eq!(&query[4..6], &[0x00, 0x01], "exactly one question");
        // The trailing root dot must not produce an empty label.
        assert_eq!(&query[12..], b"\x07example\x03com\x00\x00\x01\x00\x01");
    }

    #[test]
    fn an_over_long_label_is_refused_before_the_wire() {
        let long = "x".repeat(64);
        assert!(build_query(1, &long, Kind::A).is_err(), "64-byte label can't be encoded");
    }

    #[test]
    fn addresses_decode_from_their_rdata() {
        let msg = reply(1, &question(Kind::A), &answer(Kind::A, &[93, 184, 216, 34]));
        assert_eq!(records(&msg, Kind::A), [Record::A(Ipv4Addr::new(93, 184, 216, 34))]);
        let v6 = Ipv6Addr::new(0x2606, 0x2800, 0x220, 1, 0x248, 0x1893, 0x25c8, 0x1946);
        let msg = reply(1, &question(Kind::Aaaa), &answer(Kind::Aaaa, &v6.octets()));
        assert_eq!(records(&msg, Kind::Aaaa), [Record::Aaaa(v6)]);
    }

    #[test]
    fn compressed_names_resolve_through_their_pointer() {
        // CNAME rdata is a pointer back to the question's `example.com` — the case a naive parser
        // renders as mojibake, and the reason `read_name` follows pointers at all.
        let msg = reply(1, &question(Kind::Cname), &answer(Kind::Cname, &[0xc0, 0x0c]));
        assert_eq!(records(&msg, Kind::Cname), [Record::Name("example.com".into())]);
    }

    #[test]
    fn a_pointer_cycle_is_refused_rather_than_spun_on() {
        // A record whose name points at itself: the bounded walk must give up, not hang. (The
        // shell script this replaces had the same class of bug in its CNAME-following loop.)
        let mut msg = reply(1, &question(Kind::A), &[]);
        let here = msg.len();
        let hi = 0xc0 | (here >> 8) as u8;
        msg.extend_from_slice(&[hi, here as u8]); // a name pointing at its own offset
        assert!(read_name(&msg, here).is_err(), "a self-referential pointer must error");
    }

    #[test]
    fn txt_chunks_rejoin_into_one_string() {
        // A long TXT record arrives as 255-byte character-strings; the halves of an SPF record
        // mean nothing apart, so they're concatenated.
        let rdata = [&[5u8][..], b"v=spf", &[6u8][..], b"1 -all"].concat();
        let msg = reply(1, &question(Kind::Txt), &answer(Kind::Txt, &rdata));
        assert_eq!(records(&msg, Kind::Txt), [Record::Txt("v=spf1 -all".into())]);
    }

    #[test]
    fn mx_records_come_back_in_preference_order() {
        let mut backup = 20u16.to_be_bytes().to_vec();
        encode_name("backup.example.com", &mut backup).unwrap();
        let mut primary = 10u16.to_be_bytes().to_vec();
        encode_name("mail.example.com", &mut primary).unwrap();
        let rdata = [answer(Kind::Mx, &backup), answer(Kind::Mx, &primary)].concat();
        let msg = reply(2, &question(Kind::Mx), &rdata);
        let got = records(&msg, Kind::Mx);
        assert_eq!(got[0].render(), "10 mail.example.com", "lowest preference first: {got:?}");
        assert_eq!(got[1].render(), "20 backup.example.com");
    }

    #[test]
    fn caa_splits_into_flags_tag_and_value() {
        let rdata = [&[0u8, 5u8][..], b"issue", b"letsencrypt.org"].concat();
        let msg = reply(1, &question(Kind::Caa), &answer(Kind::Caa, &rdata));
        let got = records(&msg, Kind::Caa);
        assert_eq!(got[0].render(), "0 issue \"letsencrypt.org\"");
    }

    #[test]
    fn an_empty_answer_is_a_fact_not_an_error() {
        // NXDOMAIN and NODATA both mean "the server answered, there's nothing" — the report says
        // so, rather than treating it as a failed lookup.
        let mut msg = reply(0, &question(Kind::A), &[]);
        msg[3] = 0x83; // rcode 3 = NXDOMAIN
        assert_eq!(records(&msg, Kind::A), []);
    }

    #[test]
    fn a_truncated_reply_says_so_instead_of_half_answering() {
        let mut msg = reply(1, &question(Kind::A), &answer(Kind::A, &[1, 2, 3, 4]));
        msg[2] |= 0x02; // TC
        assert!(parse_answers(&msg, Kind::A).unwrap_err().contains("truncated"));
    }

    #[test]
    fn records_of_other_types_in_the_answer_are_skipped() {
        // A CNAME alongside the A record is normal; asking for A must yield only the address.
        let rdata = [answer(Kind::Cname, &[0xc0, 0x0c]), answer(Kind::A, &[10, 0, 0, 1])].concat();
        let msg = reply(2, &question(Kind::A), &rdata);
        assert_eq!(records(&msg, Kind::A), [Record::A(Ipv4Addr::new(10, 0, 0, 1))]);
    }

    #[test]
    fn a_query_falls_back_to_tcp_and_reads_its_length_framing() {
        // A local server speaking DNS-over-TCP: read the 2-byte length, the query, then answer
        // with the same framing. Nothing listens on UDP, so this exercises exactly the path a
        // network that blocks UDP/53 forces — the one that makes the authority check possible.
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let canned = reply(1, &question(Kind::A), &answer(Kind::A, &[203, 0, 113, 9]));
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut header = [0u8; 2];
            stream.read_exact(&mut header).unwrap();
            let mut query = vec![0u8; usize::from(u16::from_be_bytes(header))];
            stream.read_exact(&mut query).unwrap();
            // Echo the client's transaction id back, as a real server would.
            let mut out = canned.clone();
            out[0] = query[0];
            out[1] = query[1];
            stream.write_all(&(out.len() as u16).to_be_bytes()).unwrap();
            stream.write_all(&out).unwrap();
        });
        // `ask_tcp` targets port 53; drive the framing directly against the test port instead.
        let query = build_query(0x4242, "example.com", Kind::A).unwrap();
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.write_all(&(query.len() as u16).to_be_bytes()).unwrap();
        stream.write_all(&query).unwrap();
        let mut header = [0u8; 2];
        stream.read_exact(&mut header).unwrap();
        let mut got = vec![0u8; usize::from(u16::from_be_bytes(header))];
        stream.read_exact(&mut got).unwrap();
        server.join().unwrap();
        assert_eq!(&got[0..2], &[0x42, 0x42], "the reply carries our transaction id");
        assert_eq!(records(&got, Kind::A), [Record::A(Ipv4Addr::new(203, 0, 113, 9))]);
    }

    #[test]
    fn a_truncated_reply_is_recognized_so_tcp_can_be_tried() {
        let mut msg = reply(1, &question(Kind::A), &answer(Kind::A, &[1, 2, 3, 4]));
        assert!(!truncated(&msg), "a normal reply is not truncated");
        msg[2] |= 0x02;
        assert!(truncated(&msg), "the TC bit must be seen — it's what triggers the TCP retry");
    }

    #[test]
    fn an_uncacheable_label_is_unique_per_call() {
        // Two probes in one run must not collide, or the second measures the first's cache entry.
        let (first, second) = (uncacheable_label(), uncacheable_label());
        assert_ne!(first, second, "consecutive labels must differ: {first}");
        // It has to be a legal DNS label: no dots, and short enough to prefix a name.
        assert!(!first.contains('.') && first.len() <= 63, "must be one legal label: {first}");
        assert!(first.starts_with("bashrs-probe-"), "recognizable in a query log: {first}");
    }

    #[test]
    fn the_ttl_comes_back_with_the_record() {
        // `answer()` writes 300 into the TTL field; a report that plans a migration needs it.
        let msg = reply(1, &question(Kind::A), &answer(Kind::A, &[1, 2, 3, 4]));
        let answers = parse_answers(&msg, Kind::A).unwrap();
        assert_eq!(answers[0].ttl, 300, "the wire TTL must survive parsing");
    }

    #[test]
    fn soa_carries_every_timer_not_just_the_serial() {
        let mut rdata = Vec::new();
        encode_name("ns1.example.com", &mut rdata).unwrap();
        encode_name("hostmaster.example.com", &mut rdata).unwrap();
        for value in [20_240_101u32, 900, 300, 1_209_600, 60] {
            rdata.extend_from_slice(&value.to_be_bytes());
        }
        let msg = reply(1, &question(Kind::Soa), &answer(Kind::Soa, &rdata));
        let rendered = records(&msg, Kind::Soa)[0].render();
        for expected in ["serial 20240101", "refresh 900s", "retry 300s", "expire 1209600s", "min-ttl 60s"] {
            assert!(rendered.contains(expected), "missing {expected}: {rendered}");
        }
    }

    #[test]
    fn reverse_names_follow_the_arpa_conventions() {
        assert_eq!(reverse_name("8.8.4.4".parse().unwrap()), "4.4.8.8.in-addr.arpa");
        let v6: IpAddr = "2001:db8::1".parse().unwrap();
        let name = reverse_name(v6);
        assert!(name.ends_with("ip6.arpa"), "{name}");
        assert!(name.starts_with("1.0.0.0."), "nibbles run least-significant first: {name}");
        // 32 nibbles, each followed by a dot, then the one inside `ip6.arpa`.
        assert_eq!(name.matches('.').count(), 33, "one dot per nibble, plus the suffix: {name}");
    }
}
