//! Network-lookup commands (`net_*`) — a thin report shell over [`crate::support::net`]'s
//! probing engines (DNS, whois, live connections), the way `gg` sits over `treegrep`.
//!
//! Two commands, pointed opposite ways.
//!
//! `net_excavate` takes one host or address and reports everything worth knowing about it: a big
//! `dig`, which is where the name comes from. The default run is entirely read-only — DNS queries,
//! a whois record, and the same two requests a browser would make (TLS handshake, HTTP GET). The
//! two that reach further — `--trace` (traceroute) and `--ports` (a TCP connect sweep) — are
//! opt-in: they're slow, and a port sweep is an active probe, so asking for one should be a
//! decision rather than a side effect of looking up a domain.
//!
//! `net_sonar` takes one measurement — how long the TCP handshake takes — against many of the
//! internet's busiest hosts at once, and is named for what that is: a ping sent in every
//! direction, read by how long each one takes to come back. Reachable as `net_health` too, which
//! is what you are more likely to think of typing when something feels broken.
//!
//! Where excavate answers "what is this host?", sonar answers
//! "is it me, or is it them?": if every provider on every continent has gone slow together, the
//! common factor is this end of the wire.

#[bashrs_macros::category(command = NetworkCommand, prefix = "net_")]
mod commands {
    use crate::support::doc_style::{self, _header, approved, notice, problematic};
    use crate::support::theme::{Basic, Weight};
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

    // --- sonar -------------------------------------------------------------

    /// Time the handshake to the internet's busiest hosts — several endpoints per provider,
    /// probed in parallel — so "is it me, or is it them?" is answered in one screen
    #[trailing_newline]
    #[alias("net_health")]
    pub fn sonar(args: SonarArgs) {
        let timeout = std::time::Duration::from_secs(args.timeout.max(1));
        let workers = args.workers.max(1);
        let measured = match _live_layout(PROBE_TARGETS) {
            Some(layout) => _probe_live(PROBE_TARGETS, workers, timeout, &layout),
            None => _probe_quietly(PROBE_TARGETS, workers, timeout),
        };
        println!("{}", _sonar_summary(&measured));
    }

    /// Probe with the table already on screen, filling each response in as it arrives.
    ///
    /// The whole list is printed first, every response cell a dim placeholder, so the wait is
    /// spent looking at what is being asked rather than at nothing. Each answer then overwrites
    /// its own cell in place.
    fn _probe_live(
        targets: &[ProbeTarget],
        workers: usize,
        timeout: std::time::Duration,
        layout: &LiveLayout,
    ) -> Vec<probe::Latency> {
        use std::io::Write;
        print!("{}", layout.skeleton);
        let _ = std::io::stdout().flush();
        // Repainting is one cursor move, one write and one move back; two threads interleaving
        // those would land a cell on the wrong row, so the whole sequence is taken as a unit.
        let pen = std::sync::Mutex::new(());
        let rows = targets.len();
        _probe_targets(targets, workers, timeout, |index, measured| {
            let up = rows - index; // the cursor rests one line below the last row
            let paint = format!(
                "\x1b[{up}A\x1b[{col}G\x1b[K{cell}\x1b[{up}B\r",
                col = layout.response_column,
                cell = _latency_cell(measured),
            );
            let _guard = pen.lock();
            print!("{paint}");
            let _ = std::io::stdout().flush();
        })
    }

    /// Probe with no display at all, then print the finished table in one piece — for a pipe, a
    /// file, or a window too narrow to hold a row without wrapping (wrapped lines would desync
    /// the cursor arithmetic above and scatter answers onto the wrong rows).
    fn _probe_quietly(
        targets: &[ProbeTarget],
        workers: usize,
        timeout: std::time::Duration,
    ) -> Vec<probe::Latency> {
        let measured = _probe_targets(targets, workers, timeout, |_, _| {});
        let mut lines = vec![_header("Provider\tEndpoint\tHost\tResponse")];
        lines.extend(targets.iter().zip(&measured).map(|(target, latency)| {
            format!("{}\t{}\t{}\t{}", target.provider, target.endpoint, target.host, _latency_cell(*latency))
        }));
        for line in table_formatter::format_table(&lines, &_sonar_table()).unwrap_or(lines) {
            println!("{line}");
        }
        measured
    }

    /// The pre-printed table and where to write into it.
    struct LiveLayout {
        /// Header plus one line per target, every response cell still a placeholder.
        skeleton: String,
        /// 1-based column the response cells start at.
        response_column: usize,
    }

    /// Lay the table out up front, or decline to.
    ///
    /// Columns are measured here rather than by `table_formatter` because the live table has to be
    /// printed before a single response exists — and it can be, since the three left columns are
    /// known from the target list and the response is last, so nothing shifts as answers land.
    ///
    /// `None` means don't try: not a terminal (a pipe would collect the escape sequences as text),
    /// or a window too narrow for a row, where wrapping would put the cursor arithmetic a line out
    /// on every repaint.
    fn _live_layout(targets: &[ProbeTarget]) -> Option<LiveLayout> {
        use std::io::IsTerminal;
        std::io::stdout()
            .is_terminal()
            .then(|| _layout(targets, table_formatter::terminal_width()))
            .flatten()
    }

    /// The layout arithmetic on its own, given the window width — split out so it can be tested
    /// without a terminal. It is worth pinning: a response column off by one writes every answer
    /// into the hostname beside it, and the result still looks like a table.
    fn _layout(targets: &[ProbeTarget], window: usize) -> Option<LiveLayout> {
        let widest = |pick: fn(&ProbeTarget) -> &str| {
            targets.iter().map(|target| pick(target).chars().count()).max().unwrap_or(0)
        };
        let (provider, endpoint, host) =
            (widest(|t| t.provider), widest(|t| t.endpoint), widest(|t| t.host));
        let gap = TABLE_GAP;
        let response_column = provider + gap + endpoint + gap + host + gap + 1;
        // The widest thing a response cell ever holds, so a full row still fits unwrapped.
        if response_column + WIDEST_RESPONSE > window {
            return None;
        }
        let mut skeleton = _header(&format!(
            "{:<provider$}{blank:gap$}{:<endpoint$}{blank:gap$}{:<host$}{blank:gap$}Response",
            "Provider", "Endpoint", "Host", blank = "",
        ));
        skeleton.push('\n');
        for target in targets {
            skeleton.push_str(&format!(
                "{:<provider$}{blank:gap$}{:<endpoint$}{blank:gap$}{:<host$}{blank:gap$}{}\n",
                target.provider,
                target.endpoint,
                target.host,
                doc_style::_scoped(&doc_style::_wrap(&[&Weight::Dark]), PENDING),
                blank = "",
            ));
        }
        Some(LiveLayout { skeleton, response_column })
    }

    /// Spaces between columns — `table_formatter`'s default, matched by hand so the live table and
    /// the piped one line up identically.
    const TABLE_GAP: usize = 2;

    /// Shown in a response cell that hasn't answered yet.
    const PENDING: &str = "…";

    /// Longest a response cell gets (`no address`), used to check a row will fit unwrapped.
    const WIDEST_RESPONSE: usize = 10;

    #[derive(Args)]
    pub struct SonarArgs {
        /// How many probes run at once. The default already reaches the floor: probing serially
        /// takes about seven times as long, and raising it further measures the same figures in
        /// the same time
        #[arg(short = 'j', long, default_value_t = PROBE_WORKERS)]
        pub workers: usize,
        /// Seconds to wait for a handshake before calling the host unreachable
        #[arg(short = 't', long, default_value_t = 3)]
        pub timeout: u64,
    }

    /// How many probes run at once by default.
    ///
    /// A run cannot finish sooner than its slowest single probe, so the wall clock behaves as
    /// `max(total waiting ÷ workers, slowest probe)` and the useful range ends where those two
    /// meet. Measured over ten pool sizes the model held to about ten percent (process startup):
    /// 2428 ms of total waiting with a 340 ms worst host predicted a knee near seven workers and a
    /// floor of 0.34 s, against measurements of 2.70 s serial, 0.37 s at eight, and 0.35 s at
    /// everything from ten to all thirty-seven at once.
    ///
    /// So the knee belongs to the *target list and the route*, not to the machine — it moves with
    /// the ratio above. It is not the core count; the CPU is idle throughout, six milliseconds of
    /// user time against a second of wall clock, because every thread is asleep in a connect.
    /// Twelve sits comfortably past the knee measured here with room for a network whose spread is
    /// narrower (which pushes the knee higher), and sleeping threads cost nothing to keep.
    ///
    /// Two things that sound plausible and are not true here. The readings do not inflate with
    /// concurrency: median and slowest came back the same whether one probe ran or all of them.
    /// And DNS stalls are not provoked by parallelism — a dropped resolver packet costs glibc's
    /// full ten-second retry, but it strikes about as often at one lookup at a time as at
    /// thirty-seven, because what governs it is how many names are asked for, not how many at
    /// once. Resolution stays out of every reported figure ([`probe::latency`]), so a stalled run
    /// is slow without being wrong.
    ///
    /// **To re-derive this number** — worth doing whenever [`PROBE_TARGETS`] changes, and on a
    /// route very unlike the one it was measured on. A single run answers it, since the knee is
    /// just total ÷ slowest:
    ///
    /// ```text
    /// bashrs net_sonar -j 1 | sed 's/\x1b\[[0-9;]*m//g' | grep -v answered |
    ///   grep -oE '[0-9]+ ms$' |
    ///   awk '{s+=$1; if($1>m) m=$1} END{printf "knee ~ %.0f workers\n", s/m}'
    /// ```
    ///
    /// To verify that rather than trust it, sweep `-j` over 1 2 4 8 12 16 24 and take the
    /// **minimum** of several runs at each — never the mean or the median. A dropped DNS packet
    /// adds its flat ten seconds to whichever run it lands on, and averaging lets that masquerade
    /// as a slow pool size; it fooled a first pass here into reporting noise as a trend.
    const PROBE_WORKERS: usize = 12;

    /// Port to knock on. 443 everywhere: every one of these providers terminates TLS, and it is
    /// the port least likely to be filtered between here and them.
    const PROBE_PORT: u16 = 443;

    /// One endpoint worth timing.
    struct ProbeTarget {
        /// Who runs it — the grouping the report reads by.
        provider: &'static str,
        /// Which piece of their estate this is, in their own terms.
        endpoint: &'static str,
        /// The name to resolve and connect to.
        host: &'static str,
    }

    /// The hosts probed, grouped by provider and listed in that order.
    ///
    /// Chosen for spread rather than count: within a provider the entries are deliberately
    /// different *kinds* of endpoint (an API, a CDN edge, object storage) or different regions,
    /// because that is what separates "this provider is down" from "my route to one continent is
    /// bad". AWS carries the regional spread, one bucket endpoint per inhabited continent — S3
    /// answers on a per-region name, which nothing else here does as cleanly.
    const PROBE_TARGETS: &[ProbeTarget] = &[
        ProbeTarget { provider: "AWS", endpoint: "S3 · global", host: "s3.amazonaws.com" },
        ProbeTarget { provider: "AWS", endpoint: "S3 · N. America", host: "s3.us-west-2.amazonaws.com" },
        ProbeTarget { provider: "AWS", endpoint: "S3 · S. America", host: "s3.sa-east-1.amazonaws.com" },
        ProbeTarget { provider: "AWS", endpoint: "S3 · Europe", host: "s3.eu-central-1.amazonaws.com" },
        ProbeTarget { provider: "AWS", endpoint: "S3 · Africa", host: "s3.af-south-1.amazonaws.com" },
        ProbeTarget { provider: "AWS", endpoint: "S3 · Asia", host: "s3.ap-southeast-1.amazonaws.com" },
        ProbeTarget { provider: "AWS", endpoint: "S3 · Oceania", host: "s3.ap-southeast-2.amazonaws.com" },
        ProbeTarget { provider: "Cloudflare", endpoint: "edge", host: "cloudflare.com" },
        ProbeTarget { provider: "Cloudflare", endpoint: "resolver (1.1.1.1)", host: "one.one.one.one" },
        ProbeTarget { provider: "Cloudflare", endpoint: "cdnjs", host: "cdnjs.cloudflare.com" },
        ProbeTarget { provider: "Cloudflare", endpoint: "R2 storage", host: "r2.cloudflarestorage.com" },
        ProbeTarget { provider: "Google", endpoint: "search", host: "www.google.com" },
        ProbeTarget { provider: "Google", endpoint: "public DNS", host: "dns.google" },
        ProbeTarget { provider: "Google", endpoint: "Cloud Storage", host: "storage.googleapis.com" },
        ProbeTarget { provider: "Google", endpoint: "YouTube", host: "www.youtube.com" },
        ProbeTarget { provider: "GitHub", endpoint: "web", host: "github.com" },
        ProbeTarget { provider: "GitHub", endpoint: "API", host: "api.github.com" },
        ProbeTarget { provider: "GitHub", endpoint: "raw content", host: "raw.githubusercontent.com" },
        ProbeTarget { provider: "GitHub", endpoint: "release assets", host: "objects.githubusercontent.com" },
        ProbeTarget { provider: "Microsoft", endpoint: "Azure portal", host: "portal.azure.com" },
        ProbeTarget { provider: "Microsoft", endpoint: "identity", host: "login.microsoftonline.com" },
        ProbeTarget { provider: "Microsoft", endpoint: "Office", host: "www.office.com" },
        ProbeTarget { provider: "Meta", endpoint: "Facebook", host: "www.facebook.com" },
        ProbeTarget { provider: "Meta", endpoint: "Instagram", host: "www.instagram.com" },
        ProbeTarget { provider: "Meta", endpoint: "WhatsApp", host: "web.whatsapp.com" },
        ProbeTarget { provider: "Netflix", endpoint: "web", host: "www.netflix.com" },
        ProbeTarget { provider: "Netflix", endpoint: "static assets", host: "assets.nflxext.com" },
        ProbeTarget { provider: "Netflix", endpoint: "help centre", host: "help.netflix.com" },
        ProbeTarget { provider: "Discord", endpoint: "web", host: "discord.com" },
        ProbeTarget { provider: "Discord", endpoint: "gateway", host: "gateway.discord.gg" },
        ProbeTarget { provider: "Discord", endpoint: "CDN", host: "cdn.discordapp.com" },
        ProbeTarget { provider: "Apple", endpoint: "web", host: "www.apple.com" },
        ProbeTarget { provider: "Apple", endpoint: "iCloud", host: "www.icloud.com" },
        ProbeTarget { provider: "Fastly", endpoint: "edge", host: "www.fastly.com" },
        ProbeTarget { provider: "Fastly", endpoint: "API", host: "api.fastly.com" },
        ProbeTarget { provider: "Akamai", endpoint: "edge", host: "www.akamai.com" },
        ProbeTarget { provider: "Akamai", endpoint: "control centre", host: "control.akamai.com" },
    ];

    /// Response-time bands, fastest first: each entry is the exclusive ceiling in milliseconds for
    /// its colour, and anything past the last one is red.
    ///
    /// The steps follow what the distances actually mean rather than round numbers: a CDN edge in
    /// your own city, a host in your country, one on your continent, and one an ocean away. Read
    /// the colour, not the figure — the point is to spot the row that doesn't match its neighbours.
    const LATENCY_BANDS: &[(u128, Basic)] = &[
        (20, Basic::Cyan),
        (60, Basic::Green),
        (150, Basic::Yellow),
        (300, Basic::Orange),
    ];

    /// One response cell, coloured by [`LATENCY_BANDS`]. Anything that didn't answer is red and
    /// says which way it failed — a refusal, a silence and a name that resolves to nothing are
    /// three different problems.
    fn _latency_cell(latency: probe::Latency) -> String {
        let (text, colour) = match latency {
            probe::Latency::In(took) => {
                let ms = took.as_millis();
                let colour = LATENCY_BANDS
                    .iter()
                    .find(|(ceiling, _)| ms < *ceiling)
                    .map_or(Basic::Red, |(_, colour)| *colour);
                (if ms == 0 { "<1 ms".to_string() } else { format!("{ms} ms") }, colour)
            }
            probe::Latency::Refused => ("refused".to_string(), Basic::Red),
            probe::Latency::TimedOut => ("no answer".to_string(), Basic::Red),
            probe::Latency::Unresolved => ("no address".to_string(), Basic::Red),
        };
        doc_style::_scoped(&doc_style::_wrap(&[&colour]), &text)
    }

    /// The closing line: how many answered, and the middle of the distribution. The median rather
    /// than the mean, because one unreachable continent should not redraw the whole picture.
    fn _sonar_summary(measured: &[probe::Latency]) -> String {
        let mut answered: Vec<u128> = measured
            .iter()
            .filter_map(|l| match l {
                probe::Latency::In(took) => Some(took.as_millis()),
                _ => None,
            })
            .collect();
        answered.sort_unstable();
        let total = measured.len();
        match answered.get(answered.len() / 2) {
            Some(median) => format!("{} of {total} answered · median {median} ms", answered.len()),
            None => problematic(&format!("none of the {total} hosts answered — the problem is on this side")),
        }
    }

    /// Two-space columns, trailing padding trimmed — the same shape the other tables here use.
    fn _sonar_table() -> table_formatter::FormatOptions {
        table_formatter::FormatOptions { trim_trailing: true, ..Default::default() }
    }

    /// Probe every target with at most `workers` handshakes in flight, preserving input order.
    ///
    /// A shared cursor rather than a chunk per worker: the hosts differ in latency by more than an
    /// order of magnitude, so a fixed split would leave most threads idle while one waited on
    /// Sydney. Each worker keeps its own results and they are merged by index at the end, which
    /// needs no lock on the hot path.
    /// `report` is called with each result the moment it lands, from whichever worker got it —
    /// which is what lets the live table fill in as answers arrive rather than all at once. It
    /// runs on the worker's thread, so it must be cheap and must serialise its own output.
    fn _probe_targets(
        targets: &[ProbeTarget],
        workers: usize,
        timeout: std::time::Duration,
        report: impl Fn(usize, probe::Latency) + Sync,
    ) -> Vec<probe::Latency> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let cursor = AtomicUsize::new(0);
        let report = &report;
        let collected: Vec<Vec<(usize, probe::Latency)>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..workers.min(targets.len()))
                .map(|_| {
                    scope.spawn(|| {
                        let mut mine = Vec::new();
                        loop {
                            let index = cursor.fetch_add(1, Ordering::Relaxed);
                            let Some(target) = targets.get(index) else { break mine };
                            let measured = probe::latency(target.host, PROBE_PORT, timeout);
                            report(index, measured);
                            mine.push((index, measured));
                        }
                    })
                })
                .collect();
            handles.into_iter().map(|handle| handle.join().unwrap_or_default()).collect()
        });
        let mut ordered = vec![probe::Latency::Unresolved; targets.len()];
        for (index, latency) in collected.into_iter().flatten() {
            ordered[index] = latency;
        }
        ordered
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

        // ——— sonar ————————————————————————————————————————————————

        /// The colour IS the report — the figures are there to be scanned past. Each band must
        /// therefore claim exactly the range it documents, including its own upper edge going to
        /// the next colour rather than staying.
        #[test]
        fn each_latency_band_owns_its_range() {
            let coloured = |ms: u64| {
                _latency_cell(probe::Latency::In(std::time::Duration::from_millis(ms)))
            };
            let is = |cell: &str, colour: Basic| cell.contains(&doc_style::_wrap(&[&colour]));
            assert!(is(&coloured(0), Basic::Cyan) && is(&coloured(19), Basic::Cyan));
            assert!(is(&coloured(20), Basic::Green) && is(&coloured(59), Basic::Green));
            assert!(is(&coloured(60), Basic::Yellow) && is(&coloured(149), Basic::Yellow));
            assert!(is(&coloured(150), Basic::Orange) && is(&coloured(299), Basic::Orange));
            assert!(is(&coloured(300), Basic::Red) && is(&coloured(9_000), Basic::Red));
            // A sub-millisecond answer is real, not missing — say so rather than printing `0 ms`.
            assert!(coloured(0).contains("<1 ms"));
        }

        /// Every way of not answering is red, and each says which way — a refusal, a silence and a
        /// name that resolves to nothing are three different faults with three different fixes.
        #[test]
        fn failures_are_red_and_name_their_kind() {
            for (latency, wording) in [
                (probe::Latency::Refused, "refused"),
                (probe::Latency::TimedOut, "no answer"),
                (probe::Latency::Unresolved, "no address"),
            ] {
                let cell = _latency_cell(latency);
                assert!(cell.contains(wording), "{latency:?} should say `{wording}`: {cell}");
                assert!(cell.contains(&doc_style::_wrap(&[&Basic::Red])), "{latency:?} must be red");
            }
        }

        /// The live table is printed before any answer exists, then written into by cursor
        /// address. Every row must therefore open its response cell at exactly the column the
        /// repaint aims for — one off and each answer lands in the hostname beside it, which
        /// still looks like a table and is entirely wrong.
        #[test]
        fn every_live_row_opens_its_cell_at_the_repaint_column() {
            let layout = _layout(PROBE_TARGETS, 200).expect("a wide window lays out");
            assert_eq!(
                layout.skeleton.lines().count(),
                PROBE_TARGETS.len() + 1,
                "one header, then every target — the whole list is on screen before probing"
            );
            for line in layout.skeleton.lines().skip(1) {
                let plain = console::strip_ansi_codes(line);
                let opens_at = plain.chars().count() - PENDING.chars().count() + 1;
                assert_eq!(opens_at, layout.response_column, "cell starts elsewhere: {plain:?}");
            }
        }

        /// In a window too narrow for a row, the line wraps and every repaint after it is a line
        /// out — answers would scatter onto neighbouring rows. Better to decline and print once.
        #[test]
        fn a_narrow_window_declines_the_live_table() {
            assert!(_layout(PROBE_TARGETS, 40).is_none());
            assert!(_layout(PROBE_TARGETS, 200).is_some());
        }

        #[test]
        fn the_summary_reports_the_median_of_what_answered() {
            let ms = |n| probe::Latency::In(std::time::Duration::from_millis(n));
            // Five answers, one dead host: the median is of the five, and the total counts all six.
            let mixed = [ms(10), ms(400), ms(50), probe::Latency::TimedOut, ms(20), ms(30)];
            let summary = _sonar_summary(&mixed);
            assert!(summary.contains("5 of 6 answered"), "{summary}");
            assert!(summary.contains("median 30 ms"), "an unreachable host must not skew it: {summary}");
            // Nothing answered at all says where the fault probably is, rather than "median none".
            assert!(_sonar_summary(&[probe::Latency::TimedOut]).contains("this side"));
        }

        /// The table earns its keep by *spread* — several distinct endpoints per provider, so a
        /// bad route to one continent reads differently from a provider being down. A duplicate
        /// host would be two rows saying the same thing.
        #[test]
        fn the_target_list_is_well_formed_and_varied() {
            let mut hosts = std::collections::BTreeSet::new();
            let mut per_provider: std::collections::BTreeMap<&str, usize> = Default::default();
            for target in PROBE_TARGETS {
                assert!(!target.provider.is_empty() && !target.endpoint.is_empty());
                assert!(hosts.insert(target.host), "{} is listed twice", target.host);
                assert!(target.host.contains('.'), "{} is not a hostname", target.host);
                *per_provider.entry(target.provider).or_default() += 1;
            }
            for (provider, count) in &per_provider {
                assert!(*count >= 2, "{provider} has only {count} endpoint — one says too little");
            }
            assert!(per_provider.len() >= 6, "too few providers to tell a local fault from theirs");
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
