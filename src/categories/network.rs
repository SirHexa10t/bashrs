//! Network-lookup commands (`net_*`) — a thin report shell over [`crate::support::net`]'s
//! probing engines (DNS, whois, live connections), the way `gg` sits over `treegrep`.
//!
//! `net_excavate` is the one command so far: everything worth knowing about a host or an address,
//! in one pass. The default run is entirely read-only — DNS queries, a whois record, and the same
//! two requests a browser would make (TLS handshake, HTTP GET). The two that reach further —
//! `--trace` (traceroute) and `--ports` (a TCP connect sweep) — are opt-in: they're slow, and a
//! port sweep is an active probe, so asking for one should be a decision rather than a side effect
//! of looking up a domain.

#[bashrs_macros::category(command = NetworkCommand, prefix = "net_")]
mod commands {
    use crate::support::doc_style::{_header, approved, notice, problematic};
    use crate::support::exec;
    use crate::support::net::{self, dns, probe, rdap, whois, Target};
    use clap::Args;
    use std::net::IpAddr;

    /// Dig up everything about a host or address: DNS records, who owns the addresses, the
    /// certificate, and where HTTP leads — in one pass
    #[trailing_newline]
    pub fn excavate(args: ExcavateArgs) {
        let target = match net::parse(&args.target) {
            Ok(target) => target,
            Err(msg) => {
                eprintln!("net_excavate: {msg}");
                std::process::exit(1);
            }
        };
        println!("{}", _header(&format!("── {} ──", target.label())));

        // A host's own records first; an address skips straight to what owns it.
        let addresses = match &target {
            Target::Host(host) => {
                _dns_section(host);
                _authority_section(host);
                _domain_registration(host);
                _resolved_addresses(host)
            }
            Target::Ip(ip) => vec![*ip],
        };
        _address_section(&addresses);

        // The service side. A bare address gets it too — plenty of hosts serve HTTPS by IP —
        // using the address as the connect target and the SNI name.
        let service = target.label();
        _tls_section(&service);
        _http_section(&service);

        if args.ports {
            _ports_section(&addresses);
        }
        if args.trace {
            _trace_section(&service);
        }
    }

    #[derive(Args)]
    pub struct ExcavateArgs {
        /// Host, address, or URL — `example.com`, `8.8.8.8`, `https://example.com/path` all work
        pub target: String,
        /// Also sweep the common TCP ports (an active probe of the target; a few seconds)
        #[arg(long)]
        pub ports: bool,
        /// Also trace the route there (needs `traceroute`; can take ~30s)
        #[arg(long)]
        pub trace: bool,
    }

    /// How many resolved addresses get the full ownership treatment. A big site answers with a
    /// dozen equivalent front-ends; looking up every one is slow and says the same thing each time.
    const DETAILED_ADDRESSES: usize = 3;

    /// The records a lookup wants, in the order the report reads best: what it is, who serves it,
    /// then the policy records.
    const RECORDS: &[dns::Kind] = &[
        dns::Kind::A,
        dns::Kind::Aaaa,
        dns::Kind::Ns,
        dns::Kind::Mx,
        dns::Kind::Txt,
        dns::Kind::Soa,
        dns::Kind::Caa,
    ];

    /// The DNSSEC pair — asked for alongside [`RECORDS`], but reported as one yes/no rather than
    /// as records (the key material itself tells a lookup nothing).
    const SIGNING: &[dns::Kind] = &[dns::Kind::Ds, dns::Kind::Dnskey];

    /// The DNS picture: the CNAME chain first (it explains every answer below it), then each
    /// record type, then whether the zone is signed. Every record type is asked for at once —
    /// they're independent queries, and a resolver that has stopped answering would otherwise
    /// make this section pay its timeout once per type.
    fn _dns_section(host: &str) {
        println!("\n{}", _header("DNS"));
        _cname_chain(host);
        let wanted: Vec<(String, dns::Kind)> =
            RECORDS.iter().chain(SIGNING).map(|kind| (host.to_string(), *kind)).collect();
        let answers = dns::lookup_batch(&wanted);
        for (kind, answer) in RECORDS.iter().zip(&answers) {
            match answer.as_ref().map(|response| &response.answers) {
                Ok(records) if records.is_empty() && *kind == dns::Kind::Caa => {
                    // CAA absent is a finding, not silence: ANY certificate authority may issue.
                    println!("  {:<6} {:<8} none — any certificate authority may issue for this name", "CAA", "");
                }
                Ok(records) => {
                    // TTL alongside every record: the number a migration is planned around, and
                    // the first thing to check when a change "hasn't taken effect yet".
                    for answer in records {
                        println!(
                            "  {:<6} {:<8} {}",
                            kind.label(),
                            format!("{}s", answer.ttl),
                            answer.record.render()
                        );
                    }
                }
                Err(err) => println!("  {:<6} {:<8} {}", kind.label(), "", problematic(err)),
            }
        }
        let signed = answers[RECORDS.len()..]
            .iter()
            .any(|answer| matches!(answer, Ok(response) if !response.answers.is_empty()));
        println!(
            "  {:<6} {:<8} {}",
            "DNSSEC",
            "",
            if signed { approved("signed (DS/DNSKEY present)") } else { "unsigned".to_string() }
        );
        // How the resolver itself performed. Slow or wildly uneven answers here are the first
        // sign of a congested path or a struggling resolver, before anything else looks wrong.
        let times: Vec<String> = RECORDS
            .iter()
            .zip(&answers)
            .filter_map(|(kind, answer)| {
                let response = answer.as_ref().ok()?;
                Some(format!("{} {:.0}ms", kind.label(), response.elapsed.as_secs_f64() * 1000.0))
            })
            .collect();
        if let Some(server) = answers.iter().find_map(|a| a.as_ref().ok()).map(|r| r.server) {
            // A resolver on loopback is a local cache; saying so keeps sub-millisecond numbers
            // from being read as "the network is fast" when they mean "this never left the box".
            let stub = if server.is_loopback() { " — local cache, not network latency" } else { "" };
            println!("  {:<6} {:<8} {} (via {server}{stub})", "times", "", times.join(" · "));
        }
        _cold_lookup(host);
    }

    /// One deliberately uncacheable lookup under `host`, which answers two questions at once.
    ///
    /// Its *timing* is the real cost of reaching this zone — the number the cached figures above
    /// can't give, and the one that actually reflects availability and congestion. Its *result*
    /// is a wildcard check: a name nobody registered should not resolve, so if it does, the zone
    /// answers for everything under it (which changes what "this subdomain exists" means).
    fn _cold_lookup(host: &str) {
        let probe = format!("{}.{host}", dns::uncacheable_label());
        match dns::lookup(&probe, dns::Kind::A) {
            Ok(response) => {
                println!(
                    "  {:<6} {:<8} {:.0}ms for an uncached name (the true round-trip to this zone)",
                    "cold",
                    "",
                    response.elapsed.as_secs_f64() * 1000.0
                );
                if !response.answers.is_empty() {
                    let rendered = response.answers[0].record.render();
                    println!(
                        "  {:<6} {:<8} {}",
                        "wildcard",
                        "",
                        notice(&format!(
                            "a random name under this zone resolves ({rendered}) — there is a wildcard record"
                        ))
                    );
                }
            }
            Err(err) => println!("  {:<6} {:<8} {}", "cold", "", problematic(&err)),
        }
    }

    /// Put the same question to each of the zone's OWN nameservers and compare the answers.
    ///
    /// This is the check that can't be made through a resolver: a resolver picks one authority
    /// and caches it, so a zone that is mid-propagation — or split-brain, with one server serving
    /// a stale copy — looks perfectly consistent from outside. Asking each in turn surfaces both
    /// the disagreement and the per-server latency (a slow authority is a real availability
    /// problem for everyone whose resolver happens to choose it).
    fn _authority_section(host: &str) {
        let Ok(response) = dns::lookup(host, dns::Kind::Ns) else { return };
        let servers: Vec<String> = response
            .answers
            .iter()
            .map(|answer| answer.record.render().trim_end_matches('.').to_string())
            .collect();
        if servers.is_empty() {
            return;
        }
        println!("\n{}", _header("Authoritative servers"));
        // Resolve each nameserver's own address, then ask it directly — all at once, so the
        // section costs one round-trip rather than one per server.
        let addresses = dns::lookup_batch(
            &servers.iter().map(|ns| (ns.clone(), dns::Kind::A)).collect::<Vec<_>>(),
        );
        // `Err` distinguishes the two ways this fails, which need different responses: a
        // nameserver whose OWN name doesn't resolve is a delegation bug; one that resolves but
        // stays silent is unreachable (possibly only from here).
        type Probe = Result<dns::Response, &'static str>;
        let probes: Vec<(String, Probe)> = std::thread::scope(|scope| {
            let handles: Vec<_> = servers
                .iter()
                .zip(&addresses)
                .map(|(name, address)| {
                    let ip = address.as_ref().ok().and_then(|response| {
                        response.answers.iter().find_map(|answer| answer.record.ip())
                    });
                    scope.spawn(move || {
                        let probe = match ip {
                            None => Err("its own name doesn't resolve"),
                            Some(ip) => dns::lookup_via(ip, name, dns::Kind::Soa)
                                .map_err(|_| "no answer"),
                        };
                        (name.clone(), probe)
                    })
                })
                .collect();
            handles.into_iter().filter_map(|handle| handle.join().ok()).collect()
        });

        let mut serials: Vec<u32> = Vec::new();
        for (name, probe) in &probes {
            match probe {
                Ok(response) => {
                    let serial = response.answers.iter().find_map(|answer| match answer.record {
                        dns::Record::Soa { serial, .. } => Some(serial),
                        _ => None,
                    });
                    match serial {
                        Some(serial) => {
                            serials.push(serial);
                            println!(
                                "  {:<22} {:<16} serial {serial:<12} {:.0}ms{}",
                                name,
                                response.server.to_string(),
                                response.elapsed.as_secs_f64() * 1000.0,
                                // Naming the transport matters here: an answer that needed TCP
                                // says UDP/53 is blocked on this path, not that the server is fine.
                                if response.via_tcp { " (over TCP)" } else { "" }
                            );
                        }
                        None => println!("  {name:<22} {}", problematic("answered, but with no SOA")),
                    }
                }
                Err(reason) => println!("  {name:<22} {}", problematic(reason)),
            }
        }
        // Every authority silent while the local resolver answers fine is far more likely to be
        // this network blocking direct DNS than a whole zone going dark — say so, rather than
        // implying the zone is broken.
        if probes.iter().all(|(_, probe)| probe.is_err()) {
            println!(
                "  {}",
                notice(
                    "no authority answered on UDP or TCP, though the local resolver did — this \
                     network only permits DNS to its own resolver"
                )
            );
        }
        match serials.first() {
            None => {}
            Some(first) if serials.iter().all(|serial| serial == first) => {
                println!("  {}", approved(&format!("all {} servers agree on serial {first}", serials.len())));
            }
            Some(_) => {
                let (low, high) = (serials.iter().min(), serials.iter().max());
                println!(
                    "  {}",
                    problematic(&format!(
                        "serials DISAGREE ({:?} … {:?}) — the zone is mid-propagation, or a server is stale",
                        low.copied().unwrap_or_default(),
                        high.copied().unwrap_or_default()
                    ))
                );
            }
        }
    }

    /// Follow CNAMEs to the end, printing each hop. Bounded and cycle-checked: a mutual CNAME pair
    /// is a real misconfiguration, and following one naively never returns.
    fn _cname_chain(host: &str) {
        let mut current = host.to_string();
        let mut seen = vec![current.to_ascii_lowercase()];
        for _ in 0..10 {
            let Ok(records) = dns::lookup(&current, dns::Kind::Cname) else { return };
            let Some(answer) = records.answers.into_iter().next() else { return };
            let dns::Record::Name(next) = answer.record else { return };
            println!("  {:<6} {:<8} {current} → {next}", "CNAME", format!("{}s", answer.ttl));
            if seen.contains(&next.to_ascii_lowercase()) {
                println!("  {:<6} {:<8} {}", "", "", problematic("CNAME loop — the chain points back at itself"));
                return;
            }
            seen.push(next.to_ascii_lowercase());
            current = next;
        }
        println!("  {:<6} {:<8} {}", "", "", problematic("CNAME chain deeper than 10 hops — giving up"));
    }

    /// The addresses a host resolves to, IPv4 first (the order most things will actually use).
    fn _resolved_addresses(host: &str) -> Vec<IpAddr> {
        let queries =
            [(host.to_string(), dns::Kind::A), (host.to_string(), dns::Kind::Aaaa)];
        dns::lookup_batch(&queries)
            .into_iter()
            .flatten()
            .flat_map(|response| response.answers)
            .filter_map(|answer| answer.record.ip())
            .collect()
    }

    /// Per address: what kind of address it is, its reverse name, the network announcing it, and
    /// the registry's record for that block.
    fn _address_section(addresses: &[IpAddr]) {
        if addresses.is_empty() {
            println!("\n{}", _header("Addresses"));
            println!("  {}", problematic("no addresses — the name doesn't resolve"));
            return;
        }
        println!("\n{}", _header("Addresses"));
        // Every reverse lookup at once, up front — one round-trip's wait for the whole list.
        let reverses: Vec<(String, dns::Kind)> =
            addresses.iter().map(|ip| (dns::reverse_name(*ip), dns::Kind::Ptr)).collect();
        let reverses = dns::lookup_batch(&reverses);
        let owners = _owners(addresses);
        for (index, ip) in addresses.iter().enumerate() {
            println!("\n  {ip}");
            if let Some(note) = net::address_note(*ip) {
                println!("    {:<12} {}", "kind", notice(note));
                continue; // private/reserved space has no registry record and no origin AS
            }
            match reverses.get(index) {
                Some(Ok(response)) if !response.answers.is_empty() => {
                    println!("    {:<12} {}", "reverse", response.answers[0].record.render());
                }
                _ => println!("    {:<12} (none)", "reverse"),
            }
            let Some(owner) = owners.iter().find(|owner| owner.ip == *ip) else {
                println!("    {:<12} (ownership lookups limited to the first {DETAILED_ADDRESSES})", "…");
                continue;
            };
            match &owner.asn {
                Some(asn) => {
                    match &asn.name {
                        Some(name) => println!("    {:<12} AS{} {name}", "announced by", asn.number),
                        // The name comes from a SECOND lookup; when it doesn't answer, say so
                        // rather than printing the number with a stray blank after it.
                        None => println!(
                            "    {:<12} AS{}  {}",
                            "announced by",
                            asn.number,
                            notice("(name lookup didn't answer)")
                        ),
                    }
                    // This country is where the AS is REGISTERED, not where the address is used —
                    // a UK-registered host announcing a US datacentre is entirely normal.
                    println!("    {:<12} {}  (AS registered in {})", "prefix", asn.prefix, asn.country);
                }
                None => println!("    {:<12} {}", "announced by", problematic("no origin AS found")),
            }
            match &owner.whois {
                // A failed lookup must SAY so: printing nothing reads as "nothing to report",
                // and the registry record is where the netblock's own country and name live.
                None => println!(
                    "    {:<12} {}",
                    "registry",
                    problematic(
                        "no registry record — RDAP (https/443) and whois (tcp/43) both failed"
                    )
                ),
                Some(fields) if fields.is_empty() => {
                    println!("    {:<12} {}", "registry", notice("no usable fields in the record"));
                }
                Some(fields) => {
                    for (label, value) in fields {
                        println!("    {:<12} {value}", label.to_ascii_lowercase());
                    }
                    println!("    {:<12} {}", "via", owner.source);
                    _location_note(owner);
                }
            }
        }
    }

    /// Point out when the registry's country for the netblock disagrees with the AS's — the case
    /// that makes an address look like it's in one country while it is announced from another
    /// (and the reason a geolocation service and a whois lookup can both be right).
    fn _location_note(owner: &_Owner) {
        let (Some(asn), Some(fields)) = (&owner.asn, &owner.whois) else { return };
        let as_country = &asn.country;
        let Some((_, net_country)) = fields.iter().find(|(label, _)| *label == "Country") else {
            return;
        };
        if !net_country.eq_ignore_ascii_case(as_country) && !as_country.is_empty() {
            println!(
                "    {:<12} {}",
                "",
                notice(&format!(
                    "netblock is registered {net_country} but announced by an AS registered in \
                     {as_country} — registry country is not a geolocation"
                ))
            );
        }
    }

    /// What one address's ownership lookups found. Keyed by `ip` rather than by position: the
    /// looked-up set is filtered (private addresses are skipped) and capped, so a positional index
    /// into it would mis-attribute one address's owner to another.
    struct _Owner {
        ip: IpAddr,
        asn: Option<dns::Asn>,
        /// `None` = the whois lookup itself failed; `Some(vec![])` = it answered with nothing
        /// usable. The report says different things for the two.
        whois: Option<Vec<(&'static str, String)>>,
        /// Which protocol answered — worth printing, since the two reach different registries
        /// through different ports and a reader debugging a gap needs to know which was used.
        source: &'static str,
    }

    /// Who owns each of the first [`DETAILED_ADDRESSES`] public addresses — the AS announcing it
    /// and its registry record — looked up for all of them at once. Each address costs a DNS
    /// round-trip plus a whois referral chain of its own, so doing them in sequence made the
    /// section's wait the sum of them; overlapped, it's the slowest single address.
    fn _owners(addresses: &[IpAddr]) -> Vec<_Owner> {
        let detailed: Vec<IpAddr> = addresses
            .iter()
            .filter(|ip| net::address_note(**ip).is_none())
            .take(DETAILED_ADDRESSES)
            .copied()
            .collect();
        std::thread::scope(|scope| {
            let handles: Vec<_> = detailed
                .iter()
                .map(|ip| {
                    scope.spawn(move || {
                        // RDAP first (HTTPS, structured); whois only if it couldn't be reached —
                        // TCP/43 is blocked on a lot of networks, which is exactly when a report
                        // that silently omitted the registry record was at its least helpful.
                        let (fields, source) = match rdap::ip(*ip) {
                            Some(fields) if !fields.is_empty() => (Some(fields), "RDAP"),
                            _ => (whois::for_ip(*ip).map(|text| whois::distill(&text)), "whois"),
                        };
                        _Owner { ip: *ip, asn: dns::asn(*ip), whois: fields, source }
                    })
                })
                .collect();
            handles.into_iter().filter_map(|handle| handle.join().ok()).collect()
        })
    }

    /// The domain's registration: RDAP first (structured, over HTTPS), whois as the fallback.
    /// Either way the registrar-side nameservers are compared against what DNS actually serves —
    /// a mismatch is the signature of a half-finished migration.
    fn _domain_registration(host: &str) {
        println!("\n{}", _header("Registration"));
        if let Some(fields) = rdap::domain(host).filter(|fields| !fields.is_empty()) {
            for (label, value) in &fields {
                println!("  {:<14} {value}", label.to_ascii_lowercase());
            }
            println!("  {:<14} RDAP", "via");
            let registered: Vec<String> = fields
                .iter()
                .find(|(label, _)| *label == "Nameservers")
                .map(|(_, value)| value.split(", ").map(str::to_string).collect())
                .unwrap_or_default();
            _nameserver_drift(host, &registered);
            return;
        }
        let Some(text) = whois::lookup(host) else {
            println!("  {}", problematic("no registration record — RDAP and whois both failed"));
            return;
        };
        if whois::is_no_match(&text) {
            println!("  {}", notice("no registration record — a subdomain, or an unregistered name"));
            return;
        }
        let fields = whois::distill(&text);
        if fields.is_empty() {
            println!("  {}", notice("a record exists, but none of the usual fields were in it"));
        }
        for (label, value) in fields {
            println!("  {:<14} {value}", label.to_ascii_lowercase());
        }
        println!("  {:<14} whois", "via");
        _nameserver_drift(host, &whois::nameservers(&text));
    }

    /// Compare the registry's nameservers with the ones DNS answers with. They should agree; when
    /// they don't, resolution depends on which server a resolver happens to ask.
    fn _nameserver_drift(host: &str, registered: &[String]) {
        if registered.is_empty() {
            return;
        }
        let live: Vec<String> = dns::lookup(host, dns::Kind::Ns)
            .map(|response| response.answers)
            .unwrap_or_default()
            .iter()
            .map(|answer| answer.record.render().trim_end_matches('.').to_ascii_lowercase())
            .collect();
        if !live.is_empty() && !registered.iter().all(|ns| live.contains(ns)) {
            println!("  {:<14} {}", "", problematic("registry and live NS records disagree"));
            println!("  {:<14} live: {}", "", live.join(", "));
        }
    }

    /// The certificate on 443: who it's for, who signed it, when it dies.
    fn _tls_section(host: &str) {
        println!("\n{}", _header("TLS (443)"));
        if !exec::on_path("openssl") {
            println!("  {}", problematic("`openssl` isn't installed — skipping"));
            return;
        }
        let Some(tls) = probe::tls(host, 443) else {
            // Say WHY, not just "no": refused proves the host is up and simply isn't serving
            // TLS, filtered means the packets never got an answer at all.
            let reach = probe::reach(host, 443);
            println!("  {}", notice(&reach.explain(443, probe::host_alive(host))));
            if reach == probe::Reach::Open {
                println!("  {}", notice("port is open but presented no certificate — not a TLS service"));
            }
            return;
        };
        for (label, value) in [
            ("protocol", tls.protocol.clone()),
            ("cipher", tls.cipher.clone()),
            ("subject", tls.subject.clone()),
            ("issuer", tls.issuer.clone()),
            ("key", tls.key.clone()),
            ("signature", tls.signature.clone()),
            ("serial", tls.serial.clone()),
        ] {
            if let Some(value) = value {
                println!("  {label:<10} {value}");
            }
        }
        if !tls.names.is_empty() {
            // Packed to the terminal width rather than one line per name (a Google certificate
            // carries ~65) or one endless line that the terminal wraps raggedly.
            let width = table_formatter::terminal_width().max(40);
            for (index, line) in _packed(&tls.names, width.saturating_sub(13)).iter().enumerate() {
                println!("  {:<10} {line}", if index == 0 { "valid for" } else { "" });
            }
            // The check the shell script couldn't make without SANs: does the cert cover the name
            // we actually asked for? Wildcards match one label, as TLS defines them.
            let covered = tls.names.iter().any(|name| _name_matches(name, host));
            if !covered {
                println!("  {:<10} {}", "", problematic(&format!("does NOT cover {host}")));
            }
        }
        if let Some(start) = &tls.not_before {
            println!("  {:<10} {start}", "issued");
        }
        if let Some(expiry) = &tls.not_after {
            let countdown = match tls.days_left {
                Some(days) if days < 0 => problematic(&format!("EXPIRED {} days ago", -days)),
                Some(days) if days <= 14 => problematic(&format!("expires in {days} days")),
                Some(days) if days <= 30 => notice(&format!("expires in {days} days")),
                Some(days) => approved(&format!("{days} days left")),
                None => String::new(),
            };
            println!("  {:<10} {expiry}  {countdown}", "expires");
        }
        // The chain as SENT. A server that omits its intermediate still validates for clients
        // that cached one, and fails for those that didn't — a bug invisible from the leaf alone.
        if tls.chain.len() > 1 {
            for (depth, (subject, issuer)) in tls.chain.iter().enumerate() {
                let arrow = if depth == 0 { "chain" } else { "" };
                println!("  {arrow:<10} {depth}: {subject}");
                if !issuer.is_empty() {
                    println!("  {:<10}    ← issued by {issuer}", "");
                }
            }
        } else if tls.chain.len() == 1 {
            println!(
                "  {:<10} {}",
                "chain",
                notice("leaf only — the server sent no intermediate certificate")
            );
        }
        match tls.verify.as_deref() {
            Some(verdict) if verdict.starts_with("0 ") || verdict == "ok" => {
                println!("  {:<10} {}", "chain", approved("verified"))
            }
            Some(verdict) => println!("  {:<10} {}", "chain", problematic(verdict)),
            None => {}
        }
    }

    /// Pack `items` into comma-separated lines of at most `width` visible columns — the compact
    /// form for a long list (certificate names) that would otherwise be one unreadable line.
    fn _packed(items: &[String], width: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let mut current = String::new();
        for item in items {
            let addition = if current.is_empty() { item.len() } else { current.len() + 2 + item.len() };
            if !current.is_empty() && addition > width {
                lines.push(std::mem::take(&mut current));
            }
            if !current.is_empty() {
                current.push_str(", ");
            }
            current.push_str(item);
        }
        if !current.is_empty() {
            lines.push(current);
        }
        lines
    }

    /// Whether a certificate name covers `host`, including a leading `*.` wildcard (which matches
    /// exactly one label — `*.example.com` covers `www.example.com`, not `a.b.example.com`).
    fn _name_matches(cert_name: &str, host: &str) -> bool {
        let (cert_name, host) = (cert_name.to_ascii_lowercase(), host.to_ascii_lowercase());
        match cert_name.strip_prefix("*.") {
            Some(suffix) => host.strip_suffix(suffix).is_some_and(|head| {
                head.ends_with('.') && !head.trim_end_matches('.').contains('.')
            }),
            None => cert_name == host,
        }
    }

    /// Where the URL actually leads, and what the final server declares about itself. HTTPS
    /// first, then plain HTTP — a host serving only port 80 (or redirecting 80 → 443 elsewhere)
    /// would otherwise report nothing at all.
    fn _http_section(host: &str) {
        println!("\n{}", _header("HTTP"));
        if !exec::on_path("curl") {
            println!("  {}", problematic("`curl` isn't installed — skipping"));
            return;
        }
        let attempt = ["https", "http"]
            .iter()
            .find_map(|scheme| probe::http_chain(&format!("{scheme}://{host}")).map(|r| (*scheme, r)));
        let Some((scheme, (hops, headers))) = attempt else {
            // One liveness probe for both ports — it's a fact about the host, not the port.
            let alive = probe::host_alive(host);
            for port in [443u16, 80] {
                println!("  {}", notice(&probe::reach(host, port).explain(port, alive)));
            }
            return;
        };
        println!("  {:<26} {scheme}://{host}", "requested");
        for (index, hop) in hops.iter().enumerate() {
            let arrow = hop.location.as_deref().map(|to| format!(" → {to}")).unwrap_or_default();
            println!("  {}. {}{arrow}", index + 1, hop.status);
        }
        for (name, value) in &headers {
            println!("  {name:<26} {value}");
        }
        // Timing, from the same request curl already made — the numbers a client author needs.
        if let Some(t) = probe::timing(&format!("{scheme}://{host}")) {
            println!(
                "  {:<26} HTTP/{} via {} · dns {:.0}ms · tcp {:.0}ms · tls {:.0}ms · first byte {:.0}ms · total {:.0}ms",
                "negotiated",
                t.version,
                t.remote,
                t.dns * 1000.0,
                (t.connect - t.dns).max(0.0) * 1000.0,
                (t.tls - t.connect).max(0.0) * 1000.0,
                t.first_byte * 1000.0,
                t.total * 1000.0
            );
        }
        // Absent PROTECTIONS are the finding — informational headers (cf-ray, via) are not
        // expected of anyone, so their absence says nothing.
        let missing: Vec<&str> = probe::EXPECTED_HEADERS
            .iter()
            .filter(|wanted| !headers.iter().any(|(got, _)| got == *wanted))
            .copied()
            .collect();
        if !missing.is_empty() {
            println!("  {:<26} {}", "not set", notice(&missing.join(", ")));
        }
    }

    /// The opt-in TCP sweep — the first address only: the ports are a property of the host, and
    /// sweeping every front-end of a load-balanced name repeats the same answer.
    fn _ports_section(addresses: &[IpAddr]) {
        println!("\n{}", _header("Open ports"));
        let Some(ip) = addresses.first() else {
            println!("  {}", notice("no address to scan"));
            return;
        };
        let open = probe::open_ports(*ip, probe::COMMON_PORTS);
        if open.is_empty() {
            println!("  none of the {} common ports answered on {ip}", probe::COMMON_PORTS.len());
        }
        for (port, service) in open {
            println!("  {port:<6} {service}");
        }
    }

    /// The opt-in route trace. Shelled out: a traceroute needs raw sockets or `SOCK_DGRAM` ICMP,
    /// which is a privilege question best left to the system's own setuid binary.
    fn _trace_section(host: &str) {
        println!("\n{}", _header("Route"));
        if !exec::on_path("traceroute") {
            println!("  {}", notice("`traceroute` isn't installed"));
            return;
        }
        exec::run("traceroute", ["-n", "-w", "2", "-q", "1", "-m", "20", host]);
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_long_name_list_packs_into_bounded_lines() {
            // Every name survives, in order, and no line exceeds the budget — the certificate
            // list is ~65 names on a Google host and must not become 65 rows OR one endless one.
            let names: Vec<String> = (0..20).map(|n| format!("host{n}.example.com")).collect();
            let lines = _packed(&names, 60);
            assert!(lines.len() > 1 && lines.len() < names.len(), "packed, not one-per-line: {lines:?}");
            for line in &lines {
                assert!(line.len() <= 60, "line over budget ({}): {line}", line.len());
            }
            let rejoined: Vec<&str> = lines.iter().flat_map(|l| l.split(", ")).collect();
            assert_eq!(rejoined, names, "packing must not lose or reorder a name");
            // A single item that exceeds the width still gets its own line rather than vanishing.
            assert_eq!(_packed(&["x".repeat(80)], 10), vec!["x".repeat(80)]);
            assert!(_packed(&[], 40).is_empty());
        }

        #[test]
        fn wildcard_certificate_names_cover_exactly_one_label() {
            assert!(_name_matches("example.com", "example.com"));
            assert!(_name_matches("EXAMPLE.com", "example.COM"), "matching is case-insensitive");
            assert!(_name_matches("*.example.com", "www.example.com"));
            // A wildcard covers one label only — neither the bare domain nor a deeper name.
            assert!(!_name_matches("*.example.com", "example.com"));
            assert!(!_name_matches("*.example.com", "a.b.example.com"));
            assert!(!_name_matches("example.com", "evil-example.com"));
            assert!(!_name_matches("*.example.com", "www.example.com.evil.test"));
        }
    }
}
