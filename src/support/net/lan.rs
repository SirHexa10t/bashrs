//! Discovering what else is on this machine's local network — the engine behind `net_local`.
//!
//! # Finding devices without privileges
//!
//! The reliable way to enumerate a LAN is ARP: every device that holds an address must answer for
//! it, firewall or not. Sending ARP yourself needs a raw socket (root), which a shell helper has
//! no business demanding — but it isn't necessary, because *the kernel already does it*. Any IP
//! packet aimed at an on-link address makes the kernel resolve that address first, and the answer
//! lands in the neighbour table at `/proc/net/arp`, which is world-readable.
//!
//! So the sweep is: attempt one ordinary TCP connection to each address in the subnet (unprivileged,
//! and the connection is expected to fail), then read the neighbour table. An address that gained a
//! complete entry is a device that answered ARP — proof of presence that does not depend on the
//! device answering anything at the TCP layer. Verified: poking three addresses on a test subnet
//! produced neighbour entries for the two that existed and none for the one that didn't.
//!
//! That gives three independent grades of evidence, in descending strength ([`Evidence`]): a
//! resolved MAC, a TCP reset (alive, nothing on that port), and an accepted connection.
//!
//! # What is deliberately NOT reported
//!
//! **Uptime.** There is no portable unprivileged way to ask a stranger on the LAN how long it has
//! been up. The TCP timestamp option carries a clock that could be extrapolated, but reading it
//! means reading raw packet headers (root again); SNMP's `sysUpTime` needs SNMP enabled and a
//! community string; anything else is per-vendor HTTP scraping. A number derived from "when this
//! command first saw the device" would describe our own observation, not the device, so it is left
//! out rather than presented as something it isn't.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::time::Duration;

/// The kernel's neighbour table.
const ARP_TABLE: &str = "/proc/net/arp";
/// The kernel's IPv4 routing table — where the on-link subnets are named.
const ROUTE_TABLE: &str = "/proc/net/route";

/// `ATF_COM` in the neighbour table's flags: the entry is *complete* — a real MAC came back.
/// An incomplete entry (`0x0`) is a resolution that failed, which is the negative answer.
const ARP_COMPLETE: u32 = 0x2;
/// `RTF_UP` / `RTF_GATEWAY` in the route table's flags.
const RTF_UP: u32 = 0x1;
const RTF_GATEWAY: u32 = 0x2;

/// The widest subnet worth sweeping, as a host count. A home LAN is a /24 (254 hosts); a /16 —
/// which container and some corporate networks hand out — is 65 534, and probing all of them is
/// neither quick nor neighbourly. Past this, the scan is refused with the number, so the user
/// decides rather than waiting on something that looked instant.
pub(crate) const MAX_SWEEP_HOSTS: u32 = 1024;

/// One on-link IPv4 network this machine can reach directly (no gateway hop).
#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) struct Network {
    pub device: String,
    pub base: Ipv4Addr,
    pub prefix: u8,
}

impl Network {
    /// How many addresses a sweep would have to try — the usable hosts, excluding the network and
    /// broadcast addresses that no device holds.
    pub(crate) fn host_count(&self) -> u32 {
        match self.prefix {
            32 => 1,
            31 => 2, // point-to-point: both addresses are usable (RFC 3021)
            bits => (1u32 << (32 - bits)).saturating_sub(2),
        }
    }

    /// Every usable host address in the network, in order.
    pub(crate) fn hosts(&self) -> impl Iterator<Item = Ipv4Addr> + use<> {
        let first = u32::from(self.base) + u32::from(self.prefix < 31);
        (0..self.host_count()).map(move |offset| Ipv4Addr::from(first + offset))
    }
}

/// How we know a device is there, strongest first. The distinction is the report's most useful
/// column: a device can be plainly present and answer nothing at all.
#[derive(Debug, PartialEq, Eq, Clone, Copy, PartialOrd, Ord)]
pub(crate) enum Evidence {
    /// It answered ARP — it holds that address on this network. Nothing above the link layer
    /// needed to cooperate, so a fully firewalled device still shows up.
    Arp,
    /// It sent a TCP reset on the knock port: alive at the IP layer, with nothing bound there.
    Refused,
    /// It accepted a connection.
    Open,
}

impl Evidence {
    /// What the column says. Named for what each observation actually WAS, because "responds"
    /// alone invites the question "responds to what?" — and the answer (a reset on the single port
    /// the sweep knocks on) is the difference between a live host and an open service. The port is
    /// read from [`KNOCK_PORT`] rather than written here, so the label cannot outlive its truth.
    pub(crate) fn label(self) -> String {
        match self {
            Evidence::Arp => "arp reply".to_string(),
            Evidence::Refused => format!("refused :{KNOCK_PORT}"),
            Evidence::Open => "serving".to_string(),
        }
    }
}

/// The port every address is knocked on to provoke ARP.
///
/// 80 is the likeliest to be *something* on a LAN device — routers, printers and cameras nearly
/// all serve a web UI — so the knock doubles as a real answer more often than an arbitrary port
/// would. Its actual job is the ARP resolution the kernel performs before the packet leaves, which
/// happens whatever the port. Lives here, with the [`Evidence`] that names it.
pub(crate) const KNOCK_PORT: u16 = 80;

/// One device found on the LAN.
#[derive(Debug, Clone)]
pub(crate) struct Device {
    pub ip: Ipv4Addr,
    /// Its hardware address, when the kernel resolved one.
    pub mac: Option<String>,
    pub evidence: Evidence,
    /// Whether this address is one of ours.
    pub is_self: bool,
    /// Ports found accepting connections, filled by the detail pass.
    pub open_ports: Vec<(u16, &'static str)>,
    /// Reverse-DNS name, when the network's resolver knows one.
    pub hostname: Option<String>,
    /// What it appears to be, filled in once the detail pass knows its ports ([`classify`]).
    pub role: Role,
}

/// The on-link IPv4 networks from the routing table — the subnets whose addresses this machine
/// reaches directly, which are exactly the ones ARP can enumerate. Routes through a gateway are
/// skipped (their far side is not on our link), as is loopback.
pub(crate) fn local_networks() -> Vec<Network> {
    parse_routes(&std::fs::read_to_string(ROUTE_TABLE).unwrap_or_default())
}

/// Parse `/proc/net/route`. Split out from the read so the parsing is testable against captured
/// tables rather than whatever this machine happens to be plugged into.
fn parse_routes(text: &str) -> Vec<Network> {
    let mut found = Vec::new();
    for line in text.lines().skip(1) {
        // Iface Destination Gateway Flags RefCnt Use Metric Mask …
        let fields: Vec<&str> = line.split_whitespace().collect();
        let (Some(device), Some(destination), Some(flags), Some(mask)) = (
            fields.first(),
            fields.get(1).and_then(|hex| hex32(hex)),
            fields.get(3).and_then(|hex| u32::from_str_radix(hex, 16).ok()),
            fields.get(7).and_then(|hex| hex32(hex)),
        ) else {
            continue;
        };
        if flags & RTF_UP == 0 || flags & RTF_GATEWAY != 0 {
            continue; // down, or a route through a router — not our own link
        }
        let base = Ipv4Addr::from(destination);
        if base.is_loopback() || *device == "lo" {
            continue;
        }
        let prefix = u32::from_be_bytes(mask).count_ones() as u8;
        if prefix == 0 {
            continue; // the default route, which names no subnet
        }
        found.push(Network { device: (*device).to_string(), base, prefix });
    }
    found.sort_by_key(|network| (network.device.clone(), network.base));
    found.dedup();
    found
}

/// A `/proc/net` hex address field to the address bytes.
///
/// The kernel prints these as the in-memory `u32`, so on a little-endian host (every target this
/// project builds for) the parsed number's little-endian bytes ARE the address octets:
/// `000011AC` → `AC 11 00 00` → `172.17.0.0`.
fn hex32(hex: &str) -> Option<[u8; 4]> {
    Some(u32::from_str_radix(hex, 16).ok()?.to_le_bytes())
}

/// The default gateway address(es) — the routers this machine sends off-link traffic to. The
/// single most decisive clue in [`classify`]: whatever answers here IS the router, whoever made it.
pub(crate) fn default_gateways() -> Vec<Ipv4Addr> {
    parse_gateways(&std::fs::read_to_string(ROUTE_TABLE).unwrap_or_default())
}

/// Pull the gateways out of a route table: the entries that route *everything* (a zero
/// destination) through a router.
fn parse_gateways(text: &str) -> Vec<Ipv4Addr> {
    let mut found: Vec<Ipv4Addr> = text
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let destination = hex32(fields.get(1)?)?;
            let gateway = hex32(fields.get(2)?)?;
            let flags = u32::from_str_radix(fields.get(3)?, 16).ok()?;
            let is_default = destination == [0, 0, 0, 0] && flags & RTF_GATEWAY != 0;
            is_default.then(|| Ipv4Addr::from(gateway))
        })
        .filter(|ip| !ip.is_unspecified())
        .collect();
    found.sort_unstable();
    found.dedup();
    found
}

/// Parse a `A.B.C.D/bits` subnet, as `--network` accepts it — the escape hatch for a network too
/// wide to sweep whole, and the only way to look at one this machine isn't attached to.
///
/// The address is masked down to its network base, so `192.168.1.42/24` means the /24 that
/// contains it rather than being rejected for having host bits set: naming a subnet by an address
/// inside it is how people actually think of one.
pub(crate) fn parse_cidr(text: &str) -> Result<Network, String> {
    let (address, bits) = text
        .split_once('/')
        .ok_or_else(|| format!("`{text}` is not a subnet — write it as ADDRESS/BITS, e.g. 192.168.1.0/24"))?;
    let address: Ipv4Addr = address
        .trim()
        .parse()
        .map_err(|_| format!("`{address}` is not an IPv4 address"))?;
    let prefix: u8 = bits.trim().parse().map_err(|_| format!("`{bits}` is not a prefix length"))?;
    if prefix > 32 {
        return Err(format!("/{prefix} is not a prefix length — 0 to 32"));
    }
    let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix) };
    Ok(Network {
        device: "given".to_string(),
        base: Ipv4Addr::from(u32::from(address) & mask),
        prefix,
    })
}

/// The kernel's neighbour table as `address → MAC`, complete entries only.
pub(crate) fn neighbours() -> BTreeMap<Ipv4Addr, String> {
    parse_neighbours(&std::fs::read_to_string(ARP_TABLE).unwrap_or_default())
}

/// Parse `/proc/net/arp`; incomplete entries (a resolution that got no answer) are dropped, since
/// their presence is precisely the evidence that nothing is there.
fn parse_neighbours(text: &str) -> BTreeMap<Ipv4Addr, String> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let ip: Ipv4Addr = fields.first()?.parse().ok()?;
            let flags = u32::from_str_radix(fields.get(2)?.trim_start_matches("0x"), 16).ok()?;
            let mac = *fields.get(3)?;
            (flags & ARP_COMPLETE != 0 && mac != "00:00:00:00:00:00")
                .then(|| (ip, mac.to_lowercase()))
        })
        .collect()
}

/// This machine's own address on `network`, discovered by asking the kernel which source address
/// it would use to reach it. A UDP `connect` sends nothing — it only fixes the route — so this
/// costs no packets and needs no privileges.
pub(crate) fn local_address(network: &Network) -> Option<Ipv4Addr> {
    let probe = network.hosts().next()?;
    let socket = UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    socket.connect(SocketAddr::from((probe, 9))).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) => Some(ip),
        IpAddr::V6(_) => None,
    }
}

/// Knock once on `ip` to make the kernel resolve it, and report what TCP said.
///
/// The connection is *expected* to fail; its purpose is the ARP resolution the kernel performs
/// first. A reset still tells us something real (the host is up), so it's kept.
fn knock(ip: Ipv4Addr, port: u16, timeout: Duration) -> Option<Evidence> {
    match TcpStream::connect_timeout(&SocketAddr::from((ip, port)), timeout) {
        Ok(_) => Some(Evidence::Open),
        // `ConnectionRefused` is a reset: something is there and said no.
        Err(err) if err.kind() == std::io::ErrorKind::ConnectionRefused => Some(Evidence::Refused),
        Err(_) => None,
    }
}

/// Sweep `network`: knock on every host address (bounded worker pool), then read the neighbour
/// table for who answered ARP. Returns the devices found, ordered by address.
///
/// `report` is called once per address tried, for a progress line.
pub(crate) fn sweep(
    network: &Network,
    port: u16,
    timeout: Duration,
    workers: usize,
    report: impl Fn() + Sync,
) -> Vec<Device> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let addresses: Vec<Ipv4Addr> = network.hosts().collect();
    let cursor = AtomicUsize::new(0);
    let (cursor, report) = (&cursor, &report);
    // Knock on everything. Each worker parks on a socket for the timeout, so the pool can be far
    // wider than the core count without contention — the wall clock is (addresses / workers) *
    // timeout, not their sum.
    let knocked: Vec<Vec<(Ipv4Addr, Evidence)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers.min(addresses.len().max(1)))
            .map(|_| {
                let addresses = &addresses;
                scope.spawn(move || {
                    let mut mine = Vec::new();
                    loop {
                        let index = cursor.fetch_add(1, Ordering::Relaxed);
                        let Some(&ip) = addresses.get(index) else { break mine };
                        if let Some(evidence) = knock(ip, port, timeout) {
                            mine.push((ip, evidence));
                        }
                        report();
                    }
                })
            })
            .collect();
        handles.into_iter().map(|handle| handle.join().unwrap_or_default()).collect()
    });

    // The neighbour table now holds whoever answered ARP during the knocking — including every
    // device that ignored the connection entirely, which is the whole point of provoking it.
    let neighbours = neighbours();
    let mine = local_address(network);
    let mut devices: BTreeMap<Ipv4Addr, Device> = BTreeMap::new();
    for (ip, evidence) in knocked.into_iter().flatten() {
        devices.insert(
            ip,
            Device {
                ip,
                mac: neighbours.get(&ip).cloned(),
                evidence,
                is_self: mine == Some(ip),
                open_ports: Vec::new(),
                hostname: None,
                role: Role::Unknown,
            },
        );
    }
    for (&ip, mac) in &neighbours {
        if !network.contains(ip) {
            continue;
        }
        devices.entry(ip).or_insert_with(|| Device {
            ip,
            mac: Some(mac.clone()),
            evidence: Evidence::Arp,
            is_self: mine == Some(ip),
            open_ports: Vec::new(),
            hostname: None,
            role: Role::Unknown,
        });
    }
    // This machine holds an address on the network but never ARPs for itself, and cannot connect
    // to itself through the sweep either — so it is added explicitly rather than left missing.
    if let Some(ip) = mine {
        devices.entry(ip).or_insert_with(|| Device {
            ip,
            mac: None,
            evidence: Evidence::Arp,
            is_self: true,
            open_ports: Vec::new(),
            hostname: None,
            role: Role::Unknown,
        });
    }
    devices.into_values().collect()
}

impl Network {
    /// Whether `ip` falls inside this network.
    pub(crate) fn contains(&self, ip: Ipv4Addr) -> bool {
        let mask = if self.prefix == 0 { 0 } else { u32::MAX << (32 - self.prefix) };
        u32::from(ip) & mask == u32::from(self.base) & mask
    }
}

/// The manufacturer of `mac`, or a note that no manufacturer can be named.
///
/// Three sources, and the order is the point: a *fresher* registry this machine may have — a
/// system copy, or one this project fetched — wins, and `software_inventory`'s embedded listing is
/// the floor that always answers. Only when every registry is silent does the local bit explain
/// why: a self-assigned address belongs to nobody, which is the honest answer for a phone that
/// randomises its MAC per network rather than a gap in coverage.
pub(crate) fn vendor(mac: &str) -> Option<&'static str> {
    if let Some(name) = super::oui::vendor(mac) {
        return Some(name);
    }
    if let Some(name) = software_inventory::mac_vendors::vendor(mac) {
        return Some(name);
    }
    is_self_assigned(mac).then_some("(self-assigned)")
}

/// Whether `mac` is one the device chose for itself AND no registry claims it — the signature of
/// a randomising phone or laptop. A claimed local address (QEMU's `52:54:00`) is not this.
pub(crate) fn is_self_assigned(mac: &str) -> bool {
    software_inventory::mac_vendors::is_locally_administered(mac)
        && super::oui::vendor(mac).is_none()
        && software_inventory::mac_vendors::vendor(mac).is_none()
}

/// What this MAC's manufacturer implies about the device — the listing's semantic half.
pub(crate) use software_inventory::mac_vendors::{hint, Hint};

/// What a device appears to *be* — the answer to "and what is that?".
///
/// A guess, and labelled as one where it is: a role whose evidence is circumstantial prints with a
/// trailing `?` ([`Role::label`]). What a device serves is strong evidence of its job; who made it
/// is strong evidence of its identity; being the gateway is proof of one specific job.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum Role {
    ThisMachine,
    /// It is the default gateway — whatever else it may be, that is its job here.
    Router,
    /// Networking hardware that isn't this network's gateway: an access point, a switch, a
    /// second router.
    NetworkGear,
    Printer,
    Camera,
    Nas,
    MediaServer,
    Hypervisor,
    VirtualMachine,
    SingleBoard,
    IotDevice,
    WindowsPc,
    Computer,
    /// A randomised MAC serving nothing — the signature of a phone or laptop, and as far as the
    /// network alone can honestly take it.
    MobileDevice,
    Unknown,
}

impl Role {
    /// The column text. A trailing `?` marks a role inferred from circumstance rather than
    /// something the device actually told us.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Role::ThisMachine => "this machine",
            Role::Router => "router",
            Role::NetworkGear => "network gear?",
            Role::Printer => "printer",
            Role::Camera => "camera",
            Role::Nas => "NAS",
            Role::MediaServer => "media server",
            Role::Hypervisor => "hypervisor",
            Role::VirtualMachine => "virtual machine",
            Role::SingleBoard => "Raspberry Pi",
            Role::IotDevice => "IoT device",
            Role::WindowsPc => "Windows PC",
            Role::Computer => "computer",
            Role::MobileDevice => "phone/laptop?",
            Role::Unknown => "—",
        }
    }
}

/// Work out what a device is, from the three things we know: whether it routes for us, what it
/// serves, and who made it.
///
/// Ordered by how much the evidence proves, not by how specific the answer sounds. Being the
/// gateway is a fact about this network and outranks everything. A serving port is next: a device
/// answering on 9100 is a printer whoever built it. The manufacturer decides last, and only where
/// it means something ([`Hint`]) — which is why an Apple MAC with no open ports lands on
/// "phone/laptop?" rather than a confident guess between a Mac and an iPhone.
///
/// Two of the categories people ask for are deliberately absent. A **modem** in bridge mode isn't
/// on this network at all, and one that is behaves exactly like a router — nothing distinguishes
/// them from here. **Android specifically** cannot be told from iOS or a laptop: all three
/// randomise their MAC and serve nothing, so `MobileDevice` is as far as the evidence goes.
pub(crate) fn classify(device: &Device, gateways: &[Ipv4Addr]) -> Role {
    if device.is_self {
        return Role::ThisMachine;
    }
    if gateways.contains(&device.ip) {
        return Role::Router;
    }
    let serves = |port: u16| device.open_ports.iter().any(|(open, _)| *open == port);
    let hint = device.mac.as_deref().map(hint).unwrap_or(Hint::Any);

    // What it serves — a device's job, stated by the device.
    if serves(9100) || serves(515) || serves(631) {
        return Role::Printer;
    }
    if serves(554) {
        return Role::Camera;
    }
    if serves(8006) {
        return Role::Hypervisor;
    }
    if serves(32400) {
        return Role::MediaServer;
    }
    if serves(1883) {
        return Role::IotDevice;
    }
    // Who made it, where that settles the matter on its own.
    match hint {
        Hint::Printer => return Role::Printer,
        Hint::SingleBoard => return Role::SingleBoard,
        Hint::Virtual => return Role::VirtualMachine,
        Hint::Nas => return Role::Nas,
        Hint::Iot => return Role::IotDevice,
        Hint::Media => return Role::MediaServer,
        // Networking hardware that ISN'T our gateway — an access point or a spare router.
        Hint::NetworkGear => return Role::NetworkGear,
        Hint::Any | Hint::Computer => {}
    }
    // A general-purpose machine, narrowed by what it happens to run.
    if serves(3389) {
        return Role::WindowsPc;
    }
    if serves(445) || serves(139) {
        return Role::Nas; // SMB with nothing else — a file server or a NAS appliance
    }
    if serves(22) || hint == Hint::Computer {
        return Role::Computer;
    }
    // Nothing served and nobody claims the address: a device that only ever talks outward.
    match device.mac.as_deref().is_some_and(is_self_assigned) {
        true => Role::MobileDevice,
        false => Role::Unknown,
    }
}

/// The ports a scan of one's own network tries by default — the curated set from
/// `software_inventory`, which owns the question "what listens where?".
pub(crate) fn lan_ports() -> Vec<(u16, &'static str)> {
    software_inventory::ports::LAN.to_vec()
}

/// The service registered to a port, shaped for a table cell: the listing's answer, or empty.
pub(crate) fn service_name(port: u16) -> &'static str {
    software_inventory::ports::service(port).unwrap_or("")
}

/// The ports a `--ports` spec names: comma-separated numbers and `low-high` ranges
/// (`22`, `1-1024`, `22,80,8000-8100`), deduplicated and ordered.
///
/// Capped, because the spec is the one place a user can ask for something enormous by accident:
/// `1-65535` across a dozen devices is three quarters of a million connections. The cap refuses
/// with the number rather than quietly scanning a prefix of what was asked for.
pub(crate) fn parse_ports(spec: &str) -> Result<Vec<(u16, &'static str)>, String> {
    let mut wanted: Vec<u16> = Vec::new();
    for piece in spec.split(',').map(str::trim).filter(|piece| !piece.is_empty()) {
        let port = |text: &str| -> Result<u16, String> {
            text.trim().parse().map_err(|_| format!("`{text}` is not a port number"))
        };
        match piece.split_once('-') {
            Some((low, high)) => {
                let (low, high) = (port(low)?, port(high)?);
                if low > high {
                    return Err(format!("`{piece}` runs backwards — write it low-high"));
                }
                wanted.extend(low..=high);
            }
            None => wanted.push(port(piece)?),
        }
    }
    wanted.retain(|port| *port != 0); // port 0 is not a port; a range starting at 0 means 1
    wanted.sort_unstable();
    wanted.dedup();
    if wanted.is_empty() {
        return Err(format!("`{spec}` names no ports"));
    }
    if wanted.len() > MAX_SCANNED_PORTS {
        return Err(format!(
            "{} ports is past the {MAX_SCANNED_PORTS} this scans at once — narrow the range",
            wanted.len()
        ));
    }
    Ok(wanted.into_iter().map(|port| (port, service_name(port))).collect())
}

/// The most ports one sweep will try per device (see [`parse_ports`]).
pub(crate) const MAX_SCANNED_PORTS: usize = 4096;

/// What `--deep` scans: the whole privileged range, where nearly every standard service lives,
/// plus the curated high ports that don't fall in it (8080, 32400, …). Thorough without being
/// the full 65 535, which is a different kind of undertaking.
pub(crate) fn deep_ports() -> Vec<(u16, &'static str)> {
    let mut ports: Vec<u16> = (1..=1024).collect();
    ports.extend(
        software_inventory::ports::LAN
            .iter()
            .chain(software_inventory::ports::INTERNET_FACING)
            .map(|(port, _)| *port),
    );
    ports.sort_unstable();
    ports.dedup();
    ports.into_iter().map(|port| (port, service_name(port))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The route table as Linux writes it: hex, little-endian, one default route through a
    /// gateway and one on-link subnet. Only the second names a network we can ARP.
    const ROUTES: &str = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
        eth0\t00000000\t010011AC\t0003\t0\t0\t0\t00000000\t0\t0\t0\n\
        eth0\t000011AC\t00000000\t0001\t0\t0\t0\t0000FFFF\t0\t0\t0\n\
        lo\t0000007F\t00000000\t0001\t0\t0\t0\t000000FF\t0\t0\t0\n";

    #[test]
    fn routes_yield_only_the_on_link_subnets() {
        let networks = parse_routes(ROUTES);
        assert_eq!(networks.len(), 1, "the default route and loopback are not sweepable: {networks:?}");
        let network = &networks[0];
        assert_eq!(network.device, "eth0");
        assert_eq!(network.base, Ipv4Addr::new(172, 17, 0, 0), "little-endian hex decodes correctly");
        assert_eq!(network.prefix, 16, "0000FFFF is 255.255.0.0");
    }

    #[test]
    fn a_route_table_that_is_empty_or_garbage_yields_nothing_rather_than_panicking() {
        assert!(parse_routes("").is_empty());
        assert!(parse_routes("Iface\tDestination\n").is_empty());
        assert!(parse_routes("Iface\tDestination\nnonsense here\n").is_empty());
    }

    #[test]
    fn host_counts_and_ranges_exclude_network_and_broadcast() {
        let slash24 = Network { device: "eth0".into(), base: Ipv4Addr::new(192, 168, 1, 0), prefix: 24 };
        assert_eq!(slash24.host_count(), 254, "a /24 holds 254 usable addresses");
        let hosts: Vec<Ipv4Addr> = slash24.hosts().collect();
        assert_eq!(hosts.first(), Some(&Ipv4Addr::new(192, 168, 1, 1)), "the .0 network address is skipped");
        assert_eq!(hosts.last(), Some(&Ipv4Addr::new(192, 168, 1, 254)), "and the .255 broadcast");
        assert_eq!(hosts.len(), 254);

        // A /31 is a point-to-point link where both addresses are real (RFC 3021).
        let p2p = Network { device: "wg0".into(), base: Ipv4Addr::new(10, 0, 0, 0), prefix: 31 };
        assert_eq!(p2p.host_count(), 2);
        assert_eq!(p2p.hosts().collect::<Vec<_>>(), [Ipv4Addr::new(10, 0, 0, 0), Ipv4Addr::new(10, 0, 0, 1)]);

        // And a /16 is exactly the case the sweep cap exists for.
        let slash16 = Network { device: "eth0".into(), base: Ipv4Addr::new(172, 17, 0, 0), prefix: 16 };
        assert_eq!(slash16.host_count(), 65_534);
        assert!(slash16.host_count() > MAX_SWEEP_HOSTS, "a /16 must trip the cap");
    }

    #[test]
    fn a_given_subnet_parses_and_is_masked_to_its_base() {
        let network = parse_cidr("192.168.1.0/24").expect("a plain subnet");
        assert_eq!(network.base, Ipv4Addr::new(192, 168, 1, 0));
        assert_eq!(network.prefix, 24);
        // An address INSIDE the subnet names it too — that is how people write one.
        assert_eq!(parse_cidr("192.168.1.42/24").unwrap().base, Ipv4Addr::new(192, 168, 1, 0));
        assert_eq!(parse_cidr("10.1.2.3/8").unwrap().base, Ipv4Addr::new(10, 0, 0, 0));
        // Malformed input is refused with a message that says the shape wanted.
        assert!(parse_cidr("192.168.1.0").unwrap_err().contains("ADDRESS/BITS"));
        assert!(parse_cidr("nope/24").unwrap_err().contains("not an IPv4 address"));
        assert!(parse_cidr("192.168.1.0/x").unwrap_err().contains("not a prefix length"));
        assert!(parse_cidr("192.168.1.0/33").unwrap_err().contains("0 to 32"));
    }

    #[test]
    fn membership_follows_the_prefix() {
        let network = Network { device: "eth0".into(), base: Ipv4Addr::new(192, 168, 1, 0), prefix: 24 };
        assert!(network.contains(Ipv4Addr::new(192, 168, 1, 1)));
        assert!(network.contains(Ipv4Addr::new(192, 168, 1, 254)));
        assert!(!network.contains(Ipv4Addr::new(192, 168, 2, 1)), "the neighbouring /24 is not ours");
        assert!(!network.contains(Ipv4Addr::new(10, 0, 0, 1)));
    }

    /// An incomplete neighbour entry is a resolution that got NO answer — it is the evidence of
    /// absence, so treating it as a device would invert the meaning of the whole sweep.
    #[test]
    fn only_complete_neighbour_entries_count_as_devices() {
        let table = "IP address       HW type     Flags       HW address            Mask     Device\n\
            172.17.0.1       0x1         0x2         2E:C1:35:49:48:6A     *        eth0\n\
            172.17.0.2       0x1         0x2         3e:d4:59:9e:8c:c7     *        eth0\n\
            172.17.0.9       0x1         0x0         00:00:00:00:00:00     *        eth0\n";
        let found = parse_neighbours(table);
        assert_eq!(found.len(), 2, "the incomplete 0x0 entry is not a device: {found:?}");
        assert_eq!(
            found.get(&Ipv4Addr::new(172, 17, 0, 1)).map(String::as_str),
            Some("2e:c1:35:49:48:6a"),
            "MACs are normalised to lowercase so they compare and read consistently"
        );
        assert!(!found.contains_key(&Ipv4Addr::new(172, 17, 0, 9)));
        assert!(parse_neighbours("").is_empty(), "an empty table is no devices, not a panic");
    }



    /// A device fixture: address, optional MAC, and the ports it answers on.
    fn device(ip: &str, mac: Option<&str>, ports: &[(u16, &'static str)]) -> Device {
        Device {
            ip: ip.parse().unwrap(),
            mac: mac.map(str::to_string),
            evidence: Evidence::Arp,
            is_self: false,
            open_ports: ports.to_vec(),
            hostname: None,
            role: Role::Unknown,
        }
    }

    #[test]
    fn the_default_gateway_is_read_from_the_zero_destination_route() {
        assert_eq!(parse_gateways(ROUTES), [Ipv4Addr::new(172, 17, 0, 1)]);
        // An on-link route names no gateway, and a table without a default route yields none.
        assert!(parse_gateways("Iface\tDestination\tGateway\tFlags\neth0\t000011AC\t00000000\t0001\n").is_empty());
        assert!(parse_gateways("").is_empty());
    }

    /// Being the gateway is a fact about THIS network, so it outranks every other clue — including
    /// a manufacturer that would otherwise say something else entirely.
    #[test]
    fn the_gateway_is_the_router_whatever_else_it_looks_like() {
        let gateways = [Ipv4Addr::new(192, 168, 1, 1)];
        // Even wearing a Raspberry Pi's MAC and serving a printer port, the gateway is the router.
        let pi_router = device("192.168.1.1", Some("b8:27:eb:00:00:01"), &[(9100, "jetdirect")]);
        assert_eq!(classify(&pi_router, &gateways), Role::Router);
        // The same hardware at any other address is just a Raspberry Pi again.
        let pi = device("192.168.1.50", Some("b8:27:eb:00:00:01"), &[]);
        assert_eq!(classify(&pi, &gateways), Role::SingleBoard);
    }

    /// What a device serves is its job, stated by the device — it outranks who made it.
    #[test]
    fn a_serving_port_names_the_job_over_the_manufacturer() {
        let none: [Ipv4Addr; 0] = [];
        // An Intel NIC (a PC hint) answering on a print port is a printer.
        let printer = device("10.0.0.5", Some("00:1c:c0:00:00:01"), &[(9100, "jetdirect")]);
        assert_eq!(classify(&printer, &none), Role::Printer);
        for (port, service, expect) in [
            (554u16, "rtsp", Role::Camera),
            (8006, "proxmox", Role::Hypervisor),
            (32400, "plex", Role::MediaServer),
            (1883, "mqtt", Role::IotDevice),
            (3389, "rdp", Role::WindowsPc),
            (445, "smb", Role::Nas),
            (22, "ssh", Role::Computer),
        ] {
            let found = device("10.0.0.9", None, &[(port, service)]);
            assert_eq!(classify(&found, &none), expect, "port {port} should read as {expect:?}");
        }
    }

    /// Where the manufacturer settles it on its own, it does — but only for the makers whose name
    /// means one kind of thing.
    #[test]
    fn the_manufacturer_decides_only_where_it_is_unambiguous() {
        let none: [Ipv4Addr; 0] = [];
        for (mac, expect) in [
            ("b8:27:eb:00:00:01", Role::SingleBoard),   // Raspberry Pi
            ("08:00:27:00:00:01", Role::VirtualMachine), // VirtualBox
            ("52:54:00:00:00:01", Role::VirtualMachine), // QEMU, despite a local-bit MAC
            ("00:11:32:00:00:01", Role::Nas),            // Synology
            ("18:fe:34:00:00:01", Role::IotDevice),      // Espressif
            ("00:1a:79:00:00:01", Role::NetworkGear),    // Ubiquiti, but not OUR gateway
            ("00:80:77:00:00:01", Role::Printer),        // Brother
        ] {
            assert_eq!(classify(&device("10.0.0.7", Some(mac), &[]), &none), expect, "{mac}");
        }
        // Apple says nothing on its own — a Mac, an iPhone and an Apple TV share the prefix — so a
        // silent Apple device falls through to the honest guess rather than a confident wrong one.
        let apple = device("10.0.0.8", Some("a4:83:e7:00:00:01"), &[]);
        assert_eq!(classify(&apple, &none), Role::Unknown, "a globally-assigned OUI is not randomised");
        // ...but an Apple device that serves ssh is plainly a computer.
        let mac_mini = device("10.0.0.8", Some("a4:83:e7:00:00:01"), &[(22, "ssh")]);
        assert_eq!(classify(&mac_mini, &none), Role::Computer);
    }

    #[test]
    fn a_self_assigned_mac_with_nothing_open_is_the_phone_shaped_guess() {
        let none: [Ipv4Addr; 0] = [];
        let silent = device("10.0.0.20", Some("de:ad:be:ef:00:01"), &[]);
        assert_eq!(classify(&silent, &none), Role::MobileDevice);
        assert!(Role::MobileDevice.label().ends_with('?'), "an inferred role is marked as one");
        // A vendor-assigned MAC with nothing open is simply unknown — no randomisation to read.
        let quiet = device("10.0.0.21", Some("00:00:5e:00:00:01"), &[]);
        assert_eq!(classify(&quiet, &none), Role::Unknown);
        // And this machine is named as such before anything else is considered.
        let mut me = device("10.0.0.22", Some("de:ad:be:ef:00:02"), &[]);
        me.is_self = true;
        assert_eq!(classify(&me, &none), Role::ThisMachine);
    }

    /// Every role must carry a label, and only the inferred ones may wear the `?` that says so.
    #[test]
    fn only_circumstantial_roles_are_marked_as_guesses() {
        let certain = [
            Role::ThisMachine, Role::Router, Role::Printer, Role::Camera, Role::Nas,
            Role::MediaServer, Role::Hypervisor, Role::VirtualMachine, Role::SingleBoard,
            Role::IotDevice, Role::WindowsPc, Role::Computer,
        ];
        for role in certain {
            assert!(!role.label().is_empty(), "{role:?} has no label");
            assert!(!role.label().ends_with('?'), "{role:?} is evidenced, not guessed");
        }
        for role in [Role::NetworkGear, Role::MobileDevice] {
            assert!(role.label().ends_with('?'), "{role:?} is a guess and must say so");
        }
        assert_eq!(Role::Unknown.label(), "—", "no evidence prints as no answer");
    }

    #[test]
    fn a_port_spec_takes_numbers_and_ranges_and_refuses_nonsense() {
        let ports = |spec: &str| {
            parse_ports(spec).map(|found| found.into_iter().map(|(p, _)| p).collect::<Vec<_>>())
        };
        assert_eq!(ports("22").unwrap(), [22]);
        assert_eq!(ports("22,80,443").unwrap(), [22, 80, 443]);
        assert_eq!(ports("20-25").unwrap(), [20, 21, 22, 23, 24, 25]);
        assert_eq!(ports("80, 20-22 ,443").unwrap(), [20, 21, 22, 80, 443], "spaces are tolerated");
        // Overlaps collapse and the result is ordered, so the sweep never probes one port twice.
        assert_eq!(ports("22,20-23,22").unwrap(), [20, 21, 22, 23]);
        // Port 0 is not a port; a range written from it starts at 1.
        assert_eq!(ports("0-2").unwrap(), [1, 2]);

        assert!(ports("").unwrap_err().contains("no ports"));
        assert!(ports("http").unwrap_err().contains("not a port number"));
        assert!(ports("100-20").unwrap_err().contains("backwards"));
        assert!(ports("99999").unwrap_err().contains("not a port number"), "past u16");
        // The whole port space is refused with its size rather than silently truncated.
        let refused = ports("1-65535").unwrap_err();
        assert!(refused.contains(&MAX_SCANNED_PORTS.to_string()), "{refused}");
    }

    /// A scanned port should say what it is wherever that is known — the number alone makes the
    /// reader look it up, which is the work the tool exists to save.
    #[test]
    fn scanned_ports_carry_their_service_name_where_one_is_known() {
        let named = parse_ports("22,2222,9100,64321").unwrap();
        assert_eq!(named[0], (22, "ssh"));
        assert_eq!(named[1], (2222, "ssh-alt"));
        assert_eq!(named[2], (9100, "jetdirect"));
        assert_eq!(named[3].1, "", "an unnamed port carries no invented name");
        // Names come from all three tables, so a range scan labels what it crosses.
        assert_eq!(service_name(161), "snmp", "from the extras table");
        assert_eq!(service_name(3306), "mysql", "from the internet-facing list");
        assert_eq!(service_name(631), "ipp", "from the LAN list");
    }


    #[test]
    fn the_deep_sweep_covers_the_privileged_range_and_the_known_high_ports() {
        let deep = deep_ports();
        let listed: Vec<u16> = deep.iter().map(|(port, _)| *port).collect();
        assert_eq!(listed.first(), Some(&1));
        for port in [1u16, 22, 80, 443, 1024] {
            assert!(listed.contains(&port), "the privileged range must be whole: {port}");
        }
        for port in [8080u16, 9100, 32400] {
            assert!(listed.contains(&port), "known high ports ride along: {port}");
        }
        assert!(listed.windows(2).all(|pair| pair[0] < pair[1]), "sorted and deduplicated");
        assert!(deep.len() <= MAX_SCANNED_PORTS, "the built-in deep sweep stays within its own cap");
    }

    /// The whole vendor chain, end to end: a real MAC in, a manufacturer out — with no fetched
    /// cache and no system registry, which is the state a fresh install is in. These are the
    /// prefixes from a real scan that reported nothing before the listing existed.
    #[test]
    fn real_hardware_resolves_through_the_embedded_listing() {
        assert_eq!(vendor("cc:f4:11:a2:bf:ae"), Some("Google"));
        assert_eq!(vendor("88:d0:39:7f:72:20"), Some("Tonly Technology"));
        assert_eq!(vendor("00:b8:c2:64:81:e3"), Some("Heights Telecom T"), "the long tail");
        // A self-assigned address is named as such — it belongs to no manufacturer by design,
        // which is a different answer from "we don't know".
        assert_eq!(vendor("a2:0e:0a:70:c6:6f"), Some("(self-assigned)"));
        assert!(is_self_assigned("3e:d4:59:9e:8c:c7"));
        // A genuinely unassigned global prefix is simply unknown.
        assert_eq!(vendor("00:f0:21:0f:f0:02"), None);
        // And the hints still arrive, from the same listing.
        assert_eq!(hint("b8:27:eb:11:22:33"), Hint::SingleBoard);
    }

    #[test]
    fn evidence_orders_from_strongest_and_labels_read_plainly() {
        assert!(Evidence::Arp < Evidence::Refused && Evidence::Refused < Evidence::Open);
        // The refused label must NAME the port it refused on — the whole point of the change.
        assert_eq!(Evidence::Arp.label(), "arp reply");
        assert_eq!(Evidence::Refused.label(), format!("refused :{KNOCK_PORT}"));
        assert_eq!(Evidence::Open.label(), "serving");
        assert!(
            Evidence::Refused.label().contains(&KNOCK_PORT.to_string()),
            "the label tracks the knock port rather than restating it"
        );
    }

}
