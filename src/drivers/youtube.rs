//! Driving the bundled `yt-dlp`: URL classification, per-video subtitle resolution, download
//! argv assembly, the playlist unplayable-report, and the flag menu. The `dl` command
//! ([`crate::categories::download`]) stays a thin shell over this, the same way
//! [`super::python`] backs the `py_*` commands. It lives in `tools` rather than `support`
//! because it resolves and runs the bundled binaries — a layer `support` sits below.
//!
//! The module is YouTube-centric — the subtitle matrix, playlist reports, and channel tabs are
//! all YouTube-shaped — but it also exposes [`download_generic`], a bare single-video download
//! for any other site yt-dlp supports (`dl` routes there when the host isn't YouTube). The
//! generic path reuses everything site-agnostic here: [`common`]'s argv base, the failure
//! diagnosis + geo rescue, the ledger, and the download archive.
//!
//! Downloads are driven **one video at a time** (playlists and channel tabs are scanned first,
//! then each entry gets its own yt-dlp invocation). That costs one extra metadata request per
//! video, and buys the thing a single batch invocation cannot express: per-video subtitle
//! selection. YouTube's auto-caption table is a ~157-language translate matrix, so a static
//! `--sub-langs` is either too narrow (missing the uploader's own languages) or downloads the
//! whole matrix; [`sub_langs_for`] instead computes each video's exact list.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::support::doc_style;
use crate::support::exec::{capture_output, capture_stdout, run_reporting_code};
use crate::support::preferences;
use crate::support::theme_code;

/// What a YouTube URL points at — each kind downloads differently.
#[derive(Debug, PartialEq)]
pub(crate) enum Link {
    Video,
    Playlist { id: String },
    Channel { root: String },
}

/// Classify `url`. Channel forms (`/@handle`, `/channel/…`, `/c/…`, `/user/…`) normalize to
/// the channel root — any tab suffix is dropped, since tabs are enumerated at download time.
/// A `list=` parameter (or an explicit `/playlist`) means playlist, unless `single` opts
/// out. Everything else — non-YouTube URLs included, yt-dlp knows hundreds of sites — is
/// treated as a lone video.
pub(crate) fn classify(url: &str, single: bool) -> Link {
    for marker in ["/@", "/channel/", "/c/", "/user/"] {
        if let Some(start) = url.find(marker) {
            let name = start + marker.len();
            let end = url[name..].find(['/', '?', '#']).map(|i| name + i).unwrap_or(url.len());
            return Link::Channel { root: url[..end].to_string() };
        }
    }
    if !single {
        if let Some((_, rest)) = url.split_once("list=") {
            let id = rest.split(['&', '#']).next().unwrap_or_default();
            if !id.is_empty() {
                return Link::Playlist { id: id.to_string() };
            }
        }
    }
    Link::Video
}

/// The 11-char video id in a lone YouTube URL — `watch?v=ID`, `youtu.be/ID`, `/shorts/ID`,
/// `/embed/ID`. `None` for a shape we don't recognize (the caller falls back to a probe). Lets
/// [`download_video`] tell "already on disk" without a yt-dlp call, so it can skip the subtitle
/// probe + download for a video it already has.
fn id_from_url(url: &str) -> Option<String> {
    let rest = ["v=", "youtu.be/", "/shorts/", "/embed/", "/live/"]
        .iter()
        .find_map(|marker| url.split_once(marker).map(|(_, rest)| rest))?;
    let id: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    (id.len() == 11).then_some(id)
}

/// What every yt-dlp invocation of one `dl` run shares.
#[derive(Clone, Copy, Default)]
pub(crate) struct Env<'a> {
    /// The bundled ffmpeg's directory, when it exists — powers the subtitle embedding.
    pub(crate) ffmpeg_dir: Option<&'a Path>,
    /// The `--cookies` file, when the user supplied one (explicit; wins over the import).
    pub(crate) cookies: Option<&'a Path>,
    /// A `--cookies-from-browser` spec from a prior `--cookie-import` (`<browser>:<dir>`), used
    /// on every run unless an explicit `--cookies` file overrides it.
    pub(crate) cookies_from_browser: Option<&'a str>,
    /// Audio-only extraction (`-x`).
    pub(crate) audio: bool,
    /// Height cap, as a format-sort preference (`-S res:N`).
    pub(crate) res: Option<u32>,
    /// Let yt-dlp use IPv6. Off by default — every invocation adds `--force-ipv4`, because a
    /// broken or slow IPv6 route stalls each request ~5s on the happy-eyeballs fallback (measured);
    /// `--allow-ipv6` opts back in for an IPv6-only network.
    pub(crate) allow_ipv6: bool,
    /// Embed a cover-art thumbnail (`--thumbnail`). Off by default — a late, idempotent pass
    /// ([`embed_thumbnails`]) handles it, so the main download stays fast and a re-run can patch
    /// previously-downloaded videos.
    pub(crate) thumbnail: bool,
    /// Force the subtitle patch pass (`--subtitles`): scan a video (fresh or already on disk) for
    /// its expected subtitles and embed any that are missing ([`embed_subtitles`]). A fresh
    /// download already embeds subtitles inline, so this is a no-op there and a patch on re-runs.
    pub(crate) subtitles: bool,
    /// Raw passthrough args, appended after every default so they win any flag repeated.
    pub(crate) extra: &'a [String],
    /// The bundled deno, when it exists — yt-dlp's YouTube extractor wants a JS runtime (EJS)
    /// or some formats go missing.
    pub(crate) js_runtime: Option<&'a Path>,
}

/// The bundled ffmpeg's directory, when the bundle exists ([`super::resolve`] falls back to
/// the bare name otherwise, which means "let yt-dlp find a PATH one").
pub(crate) fn bundled_ffmpeg_dir() -> Option<PathBuf> {
    let resolved = crate::tools::resolve("ffmpeg");
    if resolved == "ffmpeg" {
        return None;
    }
    PathBuf::from(resolved).parent().map(Path::to_path_buf)
}

/// The bundled deno binary, when the bundle exists (else yt-dlp looks for a PATH deno itself).
pub(crate) fn bundled_deno() -> Option<PathBuf> {
    let resolved = crate::tools::resolve("deno");
    (resolved != "deno").then(|| PathBuf::from(resolved))
}

/// The `--js-runtimes deno:<path>` pair.
fn js_runtime_flag(deno: &Path) -> [OsString; 2] {
    let mut runtime = OsString::from("deno:");
    runtime.push(deno.as_os_str());
    ["--js-runtimes".into(), runtime]
}

/// The cookie flags for an invocation: an explicit `--cookies` file wins; otherwise a
/// `--cookies-from-browser` spec from a prior `--cookie-import`; otherwise none. Shared by the
/// download argv ([`common`]) and the metadata invocations ([`seeded`]) so gated content
/// (age-restricted, members-only) is readable at *every* phase, not just the final download.
fn cookie_args(env: Env) -> Vec<OsString> {
    if let Some(file) = env.cookies {
        vec!["--cookies".into(), file.as_os_str().to_owned()]
    } else if let Some(spec) = env.cookies_from_browser {
        vec!["--cookies-from-browser".into(), spec.into()]
    } else {
        Vec::new()
    }
}

/// Pare a browser's cookie DB down to only `domains`' cookies, writing the filtered copy into
/// `store_dir` for `--cookie-import`. The privacy crux: an import keeps *just* the target site's
/// cookies, never the whole DB (your banking/email cookies never touch bashrs's disk). Done
/// without decrypting anything — the domain column (`host` / `host_key`) is plaintext in both
/// families, so we filter rows on it and copy the (still-encrypted, for Chromium) values through
/// verbatim, leaving yt-dlp to decrypt via the keyring exactly as before. Runs on the bundled
/// python's sqlite3 (like [`COOKIE_EXPIRY`]); returns the number of cookies kept, or `None` if the
/// filter couldn't run. Reads the source read-only + lock-ignoring, so a running browser is fine
/// (it sees the checkpointed DB — the same WAL caveat the import message spells out).
pub(crate) fn filter_cookie_db(
    store: &crate::support::browsers::CookieStore,
    store_dir: &Path,
    domains: &[String],
) -> Option<usize> {
    let (src, dest_name) = &store.files[0];
    let dest = store_dir.join(dest_name);
    let kind = crate::support::browsers::store_kind(store.browser);
    let mut argv: Vec<OsString> = vec![
        "-c".into(), COOKIE_FILTER.into(),
        src.as_os_str().to_owned(), dest.into_os_string(), kind.into(),
    ];
    argv.extend(domains.iter().map(OsString::from));
    let (ok, out, _err) = capture_output(crate::tools::resolve("python3"), argv)?;
    ok.then(|| out.trim().parse().ok()).flatten()
}

/// The cookie-DB filter, embedded python (bundled python has sqlite3). Recreates the source
/// table's exact schema in the destination, copies only the rows whose host matches a target
/// domain (equal or a dot-boundary subdomain), and carries the version metadata yt-dlp needs to
/// read the result — Firefox's `PRAGMA user_version` (cookie-expiry units) and Chromium's `meta`
/// table (encryption version). Prints the kept-row count. Flush-left on purpose: a raw literal
/// keeps python's indentation — a `\`-continued Rust string strips the leading whitespace and
/// hands python an IndentationError.
const COOKIE_FILTER: &str = r#"import sqlite3, sys
src, dst, kind = sys.argv[1], sys.argv[2], sys.argv[3]
domains = [d.lower() for d in sys.argv[4:]]
def keep(host):
    h = (host or "").lstrip(".").lower()
    return any(h == d or h.endswith("." + d) for d in domains)
table, hostcol = ("moz_cookies", "host") if kind == "firefox" else ("cookies", "host_key")
s = sqlite3.connect("file:%s?immutable=1" % src, uri=True)
d = sqlite3.connect(dst)
schema = s.execute("SELECT sql FROM sqlite_master WHERE type='table' AND name=?", (table,)).fetchone()
if not schema:
    print(0); sys.exit(0)
d.execute(schema[0])
cols = [r[1] for r in s.execute("PRAGMA table_info(%s)" % table)]
hi = cols.index(hostcol)
kept = [r for r in s.execute("SELECT * FROM %s" % table) if keep(r[hi])]
d.executemany("INSERT INTO %s VALUES (%s)" % (table, ",".join("?" * len(cols))), kept)
if kind == "firefox":
    d.execute("PRAGMA user_version=%d" % s.execute("PRAGMA user_version").fetchone()[0])
else:
    meta = s.execute("SELECT sql FROM sqlite_master WHERE type='table' AND name='meta'").fetchone()
    if meta:
        d.execute(meta[0])
        d.executemany("INSERT INTO meta VALUES (?,?)", list(s.execute("SELECT key, value FROM meta")))
d.commit()
print(len(kept))
"#;

/// Whether the imported store's cookies have all expired — `Some(true)` if every persistent cookie's
/// expiry is in the past, `Some(false)` if any is still live (or there are only session cookies,
/// whose death isn't a timestamp), `None` if the check couldn't run. Reads the expiry column
/// straight from the store DB via the bundled python's sqlite3 (like [`filter_cookie_db`]) — expiry
/// is plaintext in both families, so no decryption and no yt-dlp.
pub(crate) fn cookies_expired(db: &Path, kind: &str) -> Option<bool> {
    let argv: Vec<OsString> =
        vec!["-c".into(), COOKIE_EXPIRY.into(), db.as_os_str().to_owned(), kind.into()];
    let (ok, out, _err) = capture_output(crate::tools::resolve("python3"), argv)?;
    if !ok {
        return None;
    }
    match out.trim() {
        "expired" => Some(true),
        "live" => Some(false),
        _ => None,
    }
}

/// Prints `expired` (every persistent cookie is in the past) or `live`. Firefox `moz_cookies.expiry`
/// is unix seconds; Chromium `cookies.expires_utc` is microseconds since 1601-01-01, converted to
/// unix. Session cookies (0/null expiry) are skipped — their death isn't a timestamp — so a store of
/// only session cookies reads `live`, never a false `expired`. Flush-left raw literal to keep
/// python's indentation (see [`COOKIE_FILTER`]).
const COOKIE_EXPIRY: &str = r#"import sqlite3, sys, time
db, kind = sys.argv[1], sys.argv[2]
table, col = ("moz_cookies", "expiry") if kind == "firefox" else ("cookies", "expires_utc")
con = sqlite3.connect("file:%s?immutable=1" % db, uri=True)
exps = [r[0] for r in con.execute("SELECT %s FROM %s" % (col, table)) if r[0]]
if kind != "firefox":
    exps = [e / 1000000.0 - 11644473600 for e in exps]
print("expired" if exps and max(exps) < time.time() else "live")
"#;

/// Have the bundled yt-dlp read the imported (already domain-filtered) store back and count what
/// decrypts, so `--cookie-import` can confirm the store is actually usable *before* every future
/// run leans on it. Runs fully offline — no URL, so yt-dlp extracts the cookies to a throwaway
/// Netscape file and quits without fetching — yet it drives the real decryption path, so a
/// keyring-locked Chromium store reports `0` here instead of failing quietly on the first
/// download. The throwaway file (decrypted cookies, briefly) is removed immediately. `None` when
/// the check couldn't run (yt-dlp not bundled, or the read crashed before writing) — the caller
/// then keeps its provisional count rather than claiming a verified one.
pub(crate) fn readable_cookie_count(spec: &str, cookies_root: &Path) -> Option<usize> {
    let probe = cookies_root.join(".probe-cookies.txt");
    let _ = std::fs::remove_file(&probe); // clear any stale probe an interrupted run left behind
    let argv: Vec<OsString> = vec![
        "--cookies-from-browser".into(),
        spec.into(),
        "--cookies".into(),
        probe.clone().into_os_string(),
    ];
    capture_ytdlp(argv)?; // None only if yt-dlp couldn't be launched at all
    let dump = std::fs::read_to_string(&probe).ok();
    let _ = std::fs::remove_file(&probe); // the decrypted dump must not linger
    Some(count_cookie_dump(&dump?))
}

/// Count the cookie rows in a Netscape dump (`#HttpOnly_` lines are cookies too; only genuine
/// comments and blanks are skipped). Split out so it's testable without yt-dlp on disk.
fn count_cookie_dump(dump: &str) -> usize {
    dump.lines()
        .map(|line| line.strip_prefix("#HttpOnly_").unwrap_or(line))
        .filter(|entry| !entry.starts_with('#') && !entry.trim().is_empty())
        .count()
}

/// `--force-ipv4` unless the user opted into IPv6. Added to *every* network invocation (via
/// [`seeded`] and [`common`]): a broken or slow IPv6 route otherwise stalls each yt-dlp request
/// ~5s on the happy-eyeballs fallback (measured — the difference between a ~13s and a ~87s
/// download of one small video).
fn ipv4_flag(env: Env) -> Vec<OsString> {
    if env.allow_ipv6 {
        Vec::new()
    } else {
        vec!["--force-ipv4".into()]
    }
}

/// Starter argv for the metadata-side invocations (probes, scans): even those run the YouTube
/// extractor, which warns — and may miss formats — without a JS runtime, warns again without an
/// ffmpeg, and can't see gated content without cookies. Hand it the bundles and cookies.
fn seeded(env: Env) -> Vec<OsString> {
    let mut argv = ipv4_flag(env);
    argv.extend(bundled_deno().map(|deno| js_runtime_flag(&deno).to_vec()).unwrap_or_default());
    if let Some(dir) = bundled_ffmpeg_dir() {
        argv.push("--ffmpeg-location".into());
        argv.push(dir.into_os_string());
    }
    argv.extend(cookie_args(env));
    argv
}

/// A lone video: probe its subtitle situation, then download with the exact list.
pub(crate) fn download_video(url: &str, into: &Path, env: Env) -> i32 {
    // The subtitle probe and the download are both for a video we don't have yet. If its file is
    // already on disk (id parsed straight from the URL — no yt-dlp call), skip both: subtitle
    // handling rides the download, so there's nothing to probe for. A URL we can't pull an id from
    // falls back to the probe. The opt-in `--thumbnail` pass below still runs, patching a re-run.
    let url_id = id_from_url(url);
    let on_disk = url_id.as_deref().is_some_and(|id| find_by_id(into, id).is_some());
    let mut code = 0;
    // `picks` feed the opt-in `--subtitles` pass. A fresh download always probes (it needs the
    // list to embed inline); an already-downloaded video probes only when `--subtitles` forces a
    // scan; a plain re-run on an existing video probes nothing.
    let mut picks: Vec<Pick> = Vec::new();
    let id = if !on_disk {
        // Announced because the probe runs silently for a few seconds (its output is captured).
        println!("probing subtitles…");
        let (probe_id, probed) = video_picks(url, env);
        picks = probed;
        let keys: Vec<String> = picks.iter().map(|pick| pick.key.clone()).collect();
        code = run(video_argv(url, into, env, &keys));
        if code != 0 {
            // The bracketed id is the ledger's stable key ([`scrub_ledger`]); a URL label still
            // carries the id for YouTube links via its `v=` parameter.
            let label =
                probe_id.as_ref().map(|id| format!("[{id}]")).unwrap_or_else(|| url.to_string());
            match diagnose_failure(video_argv(url, into, env, &keys), &label, GeoRescue::IpEnforced)
            {
                None => code = 0, // the plain retry came through
                Some(line) => write_ledger(into, &[line]),
            }
        }
        if code == 0 {
            if let Some(id) = &probe_id {
                finish_media(into, id, &picks, env);
            }
        }
        probe_id.or(url_id)
    } else if env.subtitles {
        println!("{}: already downloaded — scanning subtitles", url_id.as_deref().unwrap_or(url));
        let (probe_id, probed) = video_picks(url, env);
        picks = probed;
        probe_id.or(url_id)
    } else {
        println!("{}: already downloaded — skipping", url_id.as_deref().unwrap_or(url));
        url_id
    };
    if code == 0 {
        if let Some(id) = &id {
            if env.thumbnail {
                embed_thumbnails(into, std::slice::from_ref(id), env);
            }
            if env.subtitles {
                println!("subtitles: scanning 1 video…");
                embed_subtitles(into, id, url, &picks, env);
            }
        }
    }
    scrub_ledger(into);
    code
}

/// The sites the late `--thumbnail`/`--subtitles` patch passes support — YouTube only, today.
/// Kept as a list so the generic path's notice names exactly what IS covered as it grows.
const PATCHABLE_PLATFORMS: &[&str] = &["youtube"];

/// The generic single-video path for non-YouTube sites (`dl` routes here when the host isn't
/// YouTube). One download, flat into `into` — a generic page gives no playlist/channel structure
/// to build folders from — reusing this module's shared argv base ([`common`]), failure
/// diagnosis + rescue, ledger, and download archive. No subtitle probing: that's a
/// YouTube-caption-matrix affair, so a generic site just gets the media, metadata, thumbnail, and
/// chapters. The URL is the ledger label — the only stable key without a metadata probe (and
/// enough for [`scrub_ledger`] when it embeds the id).
pub(crate) fn download_generic(url: &str, into: &Path, env: Env) -> i32 {
    // The patch passes need a per-video id in the filename and a known thumbnail/caption source —
    // YouTube-machinery, today. Name what IS supported (the list will grow) instead of silently
    // ignoring the flag.
    if env.thumbnail || env.subtitles {
        let flags = match (env.thumbnail, env.subtitles) {
            (true, true) => "--thumbnail/--subtitles",
            (true, false) => "--thumbnail",
            _ => "--subtitles",
        };
        eprintln!(
            "dl: {flags} supported platforms: {} — skipped for this site",
            PATCHABLE_PLATFORMS.join(", ")
        );
    }
    let mut code = run(generic_argv(url, into, env));
    if code != 0 {
        match diagnose_failure(generic_argv(url, into, env), url, GeoRescue::XffSweep) {
            None => code = 0, // the plain retry or a spoofed region came through
            Some(line) => write_ledger(into, &[line]),
        }
    }
    scrub_ledger(into);
    code
}

/// Playlist mode. The flat scan comes first — it still *names* entries nothing can play
/// anymore, which downloads would only surface as opaque errors — the traceability report and
/// the download archive live inside the playlist's own folder, and every playable, not-yet-
/// archived entry is downloaded individually with its own subtitle list.
pub(crate) fn download_playlist(url: &str, id: &str, into: &Path, env: Env) -> i32 {
    let Some(scan) = scan_playlist(url, "%(title)S[%(id)S]", env) else {
        eprintln!(
            "dl: could not scan the playlist — downloading in one pass, EN-only subtitles, \
             no unplayable report"
        );
        let keys: Vec<String> = default_picks().iter().map(|pick| pick.key.clone()).collect();
        return run(playlist_argv(url, into, into, env, &keys, None));
    };
    // The playlist's folder, spelled exactly as yt-dlp's template expansion will spell it.
    let dir = scan.dirname.clone().unwrap_or_else(|| format!("[{id}]"));
    let home = into.join(&dir);
    if let Err(err) = std::fs::create_dir_all(&home) {
        eprintln!("dl: cannot create {}: {err}", home.display());
        return 1;
    }

    let dead: Vec<&ScanEntry> =
        scan.entries.iter().filter(|entry| is_tombstone(&entry.title)).collect();
    if dead.is_empty() {
        println!("all {} playlist entries are playable", scan.entries.len());
    } else {
        let report = home.join(format!("unplayable__{id}.txt"));
        match std::fs::write(&report, unplayable_report(&scan.title, id, &dead)) {
            Ok(()) => println!(
                "{} of {} entries are unplayable — traces written to {}",
                dead.len(),
                scan.entries.len(),
                report.display()
            ),
            Err(err) => eprintln!("dl: could not write {}: {err}", report.display()),
        }
    }

    let archived = archived_ids(&home.join(ARCHIVE_NAME));
    let pending: Vec<&ScanEntry> = scan
        .entries
        .iter()
        .filter(|entry| !is_tombstone(&entry.title) && !archived.contains(&entry.id))
        .collect();
    let skipped = scan.entries.len() - pending.len();
    if skipped > 0 {
        println!("{skipped} entries already archived (or unplayable) — skipped");
    }
    let code = if pending.is_empty() {
        0
    } else {
        download_pending(url, &pending, into, &home, env, |url, into, home, env, langs, items| {
            playlist_argv(url, into, home, env, langs, Some(items))
        })
    };
    // Late thumbnail pass (opt-in) over every playable entry — including archived ones, so a
    // re-run with `--thumbnail` patches previously-downloaded videos.
    if env.thumbnail {
        let ids: Vec<String> = scan
            .entries
            .iter()
            .filter(|entry| !is_tombstone(&entry.title))
            .map(|entry| entry.id.clone())
            .collect();
        embed_thumbnails(&home, &ids, env);
    }
    if env.subtitles {
        let entries: Vec<&ScanEntry> =
            scan.entries.iter().filter(|entry| !is_tombstone(&entry.title)).collect();
        patch_collection_subtitles(url, &entries, &home, env);
    }
    code
}

/// The shared batched tail of playlist and channel-tab downloads: ONE probe invocation covers
/// every pending entry's subtitles, entries are grouped by their computed list, and each group
/// downloads in one yt-dlp invocation — process startup and player work are the expensive
/// parts, so the invocation count is what matters. Probe-failed entries fall back to the EN
/// default rather than being dropped.
fn download_pending(
    url: &str,
    pending: &[&ScanEntry],
    into: &Path,
    home: &Path,
    env: Env,
    argv: impl Fn(&str, &Path, &Path, Env, &[String], &str) -> Vec<OsString>,
) -> i32 {
    println!("probing subtitles of {} entries…", pending.len());
    let indexes: Vec<String> = pending.iter().map(|entry| entry.index.clone()).collect();
    let probed = batch_probe(url, &indexes, env);
    let planned: Vec<Planned> = pending
        .iter()
        .map(|entry| {
            probed
                .iter()
                .find(|plan| plan.index == entry.index)
                .map(|plan| Planned {
                    index: plan.index.clone(),
                    id: plan.id.clone(),
                    picks: plan.picks.clone(),
                })
                .unwrap_or_else(|| Planned {
                    index: entry.index.clone(),
                    id: entry.id.clone(),
                    picks: default_picks(),
                })
        })
        .collect();
    let mut worst = 0;
    for group in group_by_langs(&planned) {
        let keys: Vec<String> =
            group[0].picks.iter().map(|pick| pick.key.clone()).collect();
        let items =
            group.iter().map(|plan| plan.index.as_str()).collect::<Vec<_>>().join(",");
        let subs = if keys.is_empty() { "none".to_string() } else { keys.join(",") };
        println!(
            "=== downloading {} entr{} (subs: {subs}) ===",
            group.len(),
            if group.len() > 1 { "ies" } else { "y" },
        );
        let code = run(argv(url, into, home, env, &keys, &items));
        if code != 0 && worst == 0 {
            worst = code;
        }
        for plan in &group {
            finish_media(home, &plan.id, &plan.picks, env);
        }
    }
    // Post-mortem: whatever is still unarchived failed inside a group, where `--ignore-errors`
    // kept the rest flowing but discarded the why. Each gets a captured re-run to diagnose,
    // geo-blocks get the region rescue, and the stubborn ones go to the ledger — unarchived on
    // purpose, so future runs keep retrying them (and the scrub below clears their ledger lines
    // the moment a retry lands).
    let survivors = archived_ids(&home.join(ARCHIVE_NAME));
    let mut ledger = Vec::new();
    for plan in planned.iter().filter(|plan| !survivors.contains(&plan.id)) {
        let title = pending
            .iter()
            .find(|entry| entry.index == plan.index)
            .map(|entry| entry.title.as_str())
            .unwrap_or(&plan.id);
        // The bracketed id is the ledger's stable key — index and title both drift as the
        // playlist changes, so scrub_ledger matches on the id alone.
        let label = format!("#{} {title} [{}]", plan.index, plan.id);
        println!("--- diagnosing {label} ---");
        let keys: Vec<String> = plan.picks.iter().map(|pick| pick.key.clone()).collect();
        match diagnose_failure(argv(url, into, home, env, &keys, &plan.index), &label, GeoRescue::IpEnforced) {
            // A plain-retry success downloaded the file just now, after the group's finish pass
            // already ran — so post-process it here, or it would keep yt-dlp's default subtitle
            // titles and (in audio mode) miss its metadata tags.
            None => finish_media(home, &plan.id, &plan.picks, env),
            Some(line) => ledger.push(line),
        }
    }
    scrub_ledger(home);
    write_ledger(home, &ledger);
    worst
}

/// Rename the embedded titles of auto-generated subtitle tracks. YouTube's own names read like
/// authentic tracks — "Spanish (Original)" sounds MORE official than the uploader's, and a
/// translated track is often titled plain "English" — so every auto pick's track becomes
/// "<name> (auto-generated)", with the misleading " (Original)" dropped. A local stream-copy
/// remux (no re-encode, no network); any failure leaves the downloaded file as-is.
fn mark_auto_titles(root: &Path, id: &str, picks: &[Pick], env: Env) {
    let autos: Vec<&Pick> = picks.iter().filter(|pick| pick.auto).collect();
    if autos.is_empty() || env.audio {
        return;
    }
    let Some(file) = find_by_id(root, id) else { return };
    let ffprobe = env
        .ffmpeg_dir
        .map(|dir| dir.join("ffprobe").into_os_string())
        .unwrap_or_else(|| "ffprobe".into());
    let Some(listing) = capture_stdout(
        &ffprobe,
        [
            OsString::from("-v"), "error".into(),
            "-select_streams".into(), "s".into(),
            "-show_entries".into(), "stream_tags=title".into(),
            "-of".into(), "csv=p=0".into(),
            file.as_os_str().to_owned(),
        ],
    ) else {
        return;
    };
    let mut retitle: Vec<(usize, String)> = Vec::new(); // (subtitle-stream order, new title)
    for (order, title) in listing.lines().enumerate() {
        if let Some(pick) = autos.iter().find(|pick| pick.name == title.trim()) {
            retitle.push((order, stamped_title(&pick.name)));
        }
    }
    if retitle.is_empty() {
        return;
    }
    let ffmpeg = env
        .ffmpeg_dir
        .map(|dir| dir.join("ffmpeg").into_os_string())
        .unwrap_or_else(|| "ffmpeg".into());
    let stamped = file.with_extension("stamping.mkv");
    let mut argv: Vec<OsString> = ["-v", "error", "-y", "-i"].map(OsString::from).to_vec();
    argv.push(file.as_os_str().to_owned());
    argv.extend(["-map", "0", "-c", "copy"].map(OsString::from));
    for (order, title) in &retitle {
        argv.push(format!("-metadata:s:s:{order}").into());
        argv.push(format!("title={title}").into());
    }
    argv.push(stamped.as_os_str().to_owned());
    let renamed = matches!(
        std::process::Command::new(&ffmpeg).args(&argv).status(),
        Ok(status) if status.success()
    ) && std::fs::rename(&stamped, &file).is_ok();
    if renamed {
        for (_, title) in &retitle {
            println!("stamped subtitle track: {title}");
        }
    } else {
        let _ = std::fs::remove_file(&stamped);
        eprintln!("dl: could not stamp auto-subtitle titles in {}", file.display());
    }
}

/// Fold the kept `.vtt` sidecars into an audio file's metadata: each becomes a tag named
/// [`subtitle_tag_name`]-style (`subtitles_en`, `subtitles_iw_autogenerated`, …) holding the
/// full VTT text, and the sidecar is deleted. Audio containers can't carry subtitle streams,
/// but their tags hold text fine — the words travel with the sound. Tags are written IN PLACE
/// by lofty, in-process (a remux would drag the embedded cover art through container rules —
/// ogg refuses picture streams); missing sidecars (a 429'd track) are simply skipped: picks
/// are must-try.
fn embed_subtitle_tags(root: &Path, id: &str, picks: &[Pick]) {
    if picks.is_empty() {
        return;
    }
    let Some(file) = find_by_id(root, id) else { return };
    let pairs = subtitle_sidecars(&file, picks);
    if pairs.is_empty() {
        return;
    }
    match write_subtitle_tags(&file, &pairs) {
        Ok(()) => {
            let names: Vec<&str> = pairs.iter().map(|(name, _)| name.as_str()).collect();
            for (_, sidecar) in &pairs {
                let _ = std::fs::remove_file(sidecar);
            }
            println!("embedded subtitle tags: {}", names.join(", "));
        }
        Err(why) => eprintln!(
            "dl: could not embed subtitle tags into {} ({why}) — sidecar .vtt files kept",
            file.display()
        ),
    }
}

/// The in-place tag writer (lofty, per-container): a plain key in Vorbis comments (opus/ogg/
/// flac), a TXXX frame on ID3 (mp3/wav), and a `----:bashrs:<name>` freeform atom on MP4 (m4a) —
/// the same spellings the earlier python/mutagen writer used, so old and new files read alike.
/// Existing tags (yt-dlp's embedded metadata) are read first and written back with the additions.
fn write_subtitle_tags(file: &Path, pairs: &[(String, PathBuf)]) -> Result<(), String> {
    use lofty::config::{ParseOptions, WriteOptions};
    use lofty::file::{AudioFile, FileType};
    use lofty::tag::TagExt;

    let mut texts: Vec<(String, String)> = Vec::new();
    for (name, sidecar) in pairs {
        let text = std::fs::read_to_string(sidecar).map_err(|err| err.to_string())?;
        texts.push((name.clone(), text));
    }
    let file_type = lofty::probe::Probe::open(file)
        .map_err(|err| err.to_string())?
        .guess_file_type()
        .map_err(|err| err.to_string())?
        .file_type()
        .ok_or("unrecognized audio container")?;
    let mut reader = std::fs::File::open(file).map_err(|err| err.to_string())?;
    let parse = ParseOptions::new();
    let write = WriteOptions::default();
    let vorbis = |tag: &mut lofty::ogg::VorbisComments, texts: Vec<(String, String)>| {
        for (name, text) in texts {
            tag.push(name, text);
        }
    };
    match file_type {
        FileType::Opus => {
            let mut audio = lofty::ogg::OpusFile::read_from(&mut reader, parse)
                .map_err(|err| err.to_string())?;
            vorbis(audio.vorbis_comments_mut(), texts);
            audio.vorbis_comments().save_to_path(file, write).map_err(|err| err.to_string())
        }
        FileType::Vorbis => {
            let mut audio = lofty::ogg::VorbisFile::read_from(&mut reader, parse)
                .map_err(|err| err.to_string())?;
            vorbis(audio.vorbis_comments_mut(), texts);
            audio.vorbis_comments().save_to_path(file, write).map_err(|err| err.to_string())
        }
        FileType::Flac => {
            let mut audio = lofty::flac::FlacFile::read_from(&mut reader, parse)
                .map_err(|err| err.to_string())?;
            if audio.vorbis_comments().is_none() {
                audio.set_vorbis_comments(lofty::ogg::VorbisComments::default());
            }
            let tag = audio.vorbis_comments_mut().expect("tag ensured above");
            vorbis(tag, texts);
            tag.save_to_path(file, write).map_err(|err| err.to_string())
        }
        FileType::Mpeg | FileType::Wav => {
            // Both carry ID3v2; read via their own parsers so the tag offset is right.
            let mut id3 = if file_type == FileType::Mpeg {
                lofty::mpeg::MpegFile::read_from(&mut reader, parse)
                    .map_err(|err| err.to_string())?
                    .id3v2()
                    .cloned()
            } else {
                lofty::iff::wav::WavFile::read_from(&mut reader, parse)
                    .map_err(|err| err.to_string())?
                    .id3v2()
                    .cloned()
            }
            .unwrap_or_default();
            for (name, text) in texts {
                id3.insert_user_text(name, text);
            }
            id3.save_to_path(file, write).map_err(|err| err.to_string())
        }
        FileType::Mp4 => {
            use lofty::mp4::{Atom, AtomData, AtomIdent};
            let mut audio = lofty::mp4::Mp4File::read_from(&mut reader, parse)
                .map_err(|err| err.to_string())?;
            if audio.ilst().is_none() {
                audio.set_ilst(lofty::mp4::Ilst::new());
            }
            let ilst = audio.ilst_mut().expect("tag ensured above");
            for (name, text) in texts {
                let ident = AtomIdent::Freeform { mean: "bashrs".into(), name: name.into() };
                ilst.insert(Atom::new(ident, AtomData::UTF8(text)));
            }
            ilst.save_to_path(file, write).map_err(|err| err.to_string())
        }
        other => Err(format!("unsupported audio container: {other:?}")),
    }
}

/// The per-file finishing pass, by mode: video files get their auto-subtitle track titles
/// stamped; audio files get the kept `.vtt` sidecars folded into metadata tags.
fn finish_media(root: &Path, id: &str, picks: &[Pick], env: Env) {
    if env.audio {
        embed_subtitle_tags(root, id, picks);
    } else {
        mark_auto_titles(root, id, picks, env);
    }
}

/// The sidecar `.vtt` files that actually arrived for `file`'s picks, paired with their tag
/// names — yt-dlp names each `<output-stem>.<lang-key>.vtt`. A missing sidecar (a refused
/// track) simply isn't listed: picks are must-try.
fn subtitle_sidecars(file: &Path, picks: &[Pick]) -> Vec<(String, PathBuf)> {
    let stem = file.with_extension("");
    picks
        .iter()
        .filter_map(|pick| {
            let sidecar = PathBuf::from(format!("{}.{}.vtt", stem.display(), pick.key));
            sidecar.is_file().then(|| (subtitle_tag_name(pick), sidecar))
        })
        .collect()
}

/// The metadata tag carrying one subtitle track: `subtitles_<key>` with the key lowercased and
/// `_`-joined, the redundant `_orig` marker folded into the `_autogenerated` suffix that every
/// auto track gets.
fn subtitle_tag_name(pick: &Pick) -> String {
    let key = pick.key.to_lowercase().replace('-', "_");
    let key = key.strip_suffix("_orig").unwrap_or(&key);
    if pick.auto {
        format!("subtitles_{key}_autogenerated")
    } else {
        format!("subtitles_{key}")
    }
}

/// An auto track's honest title: YouTube's name with the actively-misleading " (Original)"
/// dropped and the machine origin stated.
fn stamped_title(name: &str) -> String {
    format!("{} (auto-generated)", name.replace(" (Original)", ""))
}

/// The absolute stream index of the embedded cover in an ffprobe
/// `stream=index:stream_disposition=attached_pic -of default=nw=1` listing (`index=N` lines, each
/// followed by its dispositions), or `None` when no stream carries `attached_pic=1`. Pure, so the
/// idempotency gate and the remux cover-carry are unit-tested without ffprobe.
fn attached_pic_stream_index(listing: &str) -> Option<u32> {
    let mut current = None;
    for line in listing.lines() {
        if let Some(index) = line.strip_prefix("index=") {
            current = index.trim().parse().ok();
        } else if line.trim() == "DISPOSITION:attached_pic=1" {
            return current;
        }
    }
    None
}

/// The absolute stream index of `file`'s embedded thumbnail: `Some(Some(idx))` when one exists,
/// `Some(None)` when none does, `None` when ffprobe couldn't run — the caller then leaves the
/// file untouched rather than guess.
fn embedded_thumbnail_index(file: &Path, env: Env) -> Option<Option<u32>> {
    let ffprobe = env
        .ffmpeg_dir
        .map(|dir| dir.join("ffprobe").into_os_string())
        .unwrap_or_else(|| "ffprobe".into());
    let listing = capture_stdout(
        &ffprobe,
        [
            OsString::from("-v"), "error".into(),
            "-show_entries".into(), "stream=index:stream_disposition=attached_pic".into(),
            "-of".into(), "default=nw=1".into(),
            file.as_os_str().to_owned(),
        ],
    )?;
    Some(attached_pic_stream_index(&listing))
}

/// Whether `file` already carries an embedded thumbnail. `None` when ffprobe couldn't run.
fn has_embedded_thumbnail(file: &Path, env: Env) -> Option<bool> {
    Some(embedded_thumbnail_index(file, env)?.is_some())
}

/// Pull the attached cover (stream `index`) out of `file` — a one-packet stream copy into a temp
/// `.jpg` beside it — so a remux can re-`-attach` it. Covers here are always JPEG: both `dl`'s
/// own pass and yt-dlp's legacy inline embeds convert to jpg. `None` (temp cleaned) on failure.
fn extract_thumbnail(file: &Path, index: u32, env: Env) -> Option<PathBuf> {
    let ffmpeg = env
        .ffmpeg_dir
        .map(|dir| dir.join("ffmpeg").into_os_string())
        .unwrap_or_else(|| "ffmpeg".into());
    let tmp = file.with_extension("cover-keep.jpg");
    let mut argv: Vec<OsString> = ["-v", "error", "-y", "-i"].map(OsString::from).to_vec();
    argv.push(file.as_os_str().to_owned());
    argv.push("-map".into());
    argv.push(format!("0:{index}").into());
    argv.extend(["-frames:v", "1", "-c", "copy"].map(OsString::from));
    argv.push(tmp.as_os_str().to_owned());
    let ok = matches!(
        std::process::Command::new(&ffmpeg).args(&argv).status(),
        Ok(status) if status.success()
    ) && tmp.is_file();
    if ok {
        Some(tmp)
    } else {
        let _ = std::fs::remove_file(&tmp);
        None
    }
}

/// Fetch YouTube video `id`'s thumbnail to `dest` (a `.jpg`), best quality first: `maxresdefault`
/// (HD, often absent for low-effort uploads), then `hqdefault` (always present). Uses `curl` — the
/// same fetcher `dl` uses for pages — hitting the two known URLs directly, so no yt-dlp launch and
/// no format-probe cascade. Returns whether a file landed.
fn fetch_youtube_thumbnail(id: &str, dest: &Path) -> bool {
    for quality in ["maxresdefault", "hqdefault"] {
        let url = format!("https://i.ytimg.com/vi/{id}/{quality}.jpg");
        let landed = capture_output(
            "curl",
            [OsString::from("-fsSL"), "-o".into(), dest.as_os_str().to_owned(), url.into()],
        )
        .is_some_and(|(ok, _, _)| ok);
        if landed {
            return true;
        }
    }
    false
}

/// Attach `thumb` into the mkv `file` as cover art (an mkv attachment, exactly as yt-dlp embeds
/// it), keeping every existing stream (`-map 0 -c copy`). Writes a sibling temp file and renames
/// over the original. Returns whether it succeeded.
fn attach_thumbnail(file: &Path, thumb: &Path, env: Env) -> bool {
    let ffmpeg = env
        .ffmpeg_dir
        .map(|dir| dir.join("ffmpeg").into_os_string())
        .unwrap_or_else(|| "ffmpeg".into());
    let out = file.with_extension("thumbing.mkv");
    let mut argv: Vec<OsString> = ["-v", "error", "-y", "-i"].map(OsString::from).to_vec();
    argv.push(file.as_os_str().to_owned());
    argv.extend(["-map", "0", "-c", "copy", "-attach"].map(OsString::from));
    argv.push(thumb.as_os_str().to_owned());
    argv.extend(
        ["-metadata:s:t:0", "mimetype=image/jpeg", "-metadata:s:t:0", "filename=cover.jpg"]
            .map(OsString::from),
    );
    argv.push(out.as_os_str().to_owned());
    let ok = matches!(
        std::process::Command::new(&ffmpeg).args(&argv).status(),
        Ok(status) if status.success()
    ) && std::fs::rename(&out, file).is_ok();
    if !ok {
        let _ = std::fs::remove_file(&out);
    }
    ok
}

/// Fetch and embed `id`'s thumbnail into `file`, cleaning up the temp image. Returns success.
fn embed_one_thumbnail(file: &Path, id: &str, env: Env) -> bool {
    let thumb = file.with_extension("cover.jpg");
    let ok = fetch_youtube_thumbnail(id, &thumb) && attach_thumbnail(file, &thumb, env);
    let _ = std::fs::remove_file(&thumb);
    ok
}

/// The late, opt-in (`--thumbnail`) cover-art pass: for each video `id` under `dir`, scan and
/// report whether it already has an embedded thumbnail — so the user sees up front what's missing
/// rather than waiting on unbounded work — then fetch + embed only the ones lacking one.
/// Deliberately independent of the download archive, so a re-run *patches* previously-downloaded
/// videos; the embedded-thumbnail check keeps it idempotent (a video that already has one is never
/// re-fetched or re-embedded).
fn embed_thumbnails(dir: &Path, ids: &[String], env: Env) {
    if ids.is_empty() {
        return;
    }
    println!("thumbnails: scanning {} video(s)…", ids.len());
    let mut missing: Vec<(String, PathBuf)> = Vec::new();
    for id in ids {
        match find_by_id(dir, id) {
            None => println!("  [{id}]: {}", doc_style::problematic("no file found — skipping")),
            // A cover embeds as an mkv attachment ([`attach_thumbnail`] muxes into the mkv
            // container) — renaming that over an audio/webm file would swap its container out
            // from under it, corrupting it. Skip anything that isn't an mkv, and say so.
            Some(file) if file.extension().and_then(|ext| ext.to_str()) != Some("mkv") => {
                println!("  [{id}]: {}", doc_style::problematic("not an mkv — cover art embeds into video (mkv) only; skipping"));
            }
            Some(file) => match has_embedded_thumbnail(&file, env) {
                Some(true) => println!("  [{id}]: {}", doc_style::approved("already has a thumbnail")),
                Some(false) => {
                    println!("  [{id}]: {}", doc_style::problematic("missing a thumbnail"));
                    missing.push((id.clone(), file));
                }
                None => println!("  [{id}]: {}", doc_style::problematic("could not read (ffprobe) — skipping")),
            },
        }
    }
    if missing.is_empty() {
        println!("thumbnails: all present — nothing to embed");
        return;
    }
    println!("thumbnails: fetching + embedding {} …", missing.len());
    for (id, file) in &missing {
        if embed_one_thumbnail(file, id, env) {
            println!("  [{id}]: {}", doc_style::approved("embedded"));
        } else {
            eprintln!("  [{id}]: {}", doc_style::problematic("could not embed a thumbnail"));
        }
    }
}

/// The titles of the subtitle streams in an ffprobe `-select_streams s -show_entries
/// stream=index:stream_tags=title -of default=nw=1` listing — one `TAG:title=…` line per titled
/// stream. Split out pure so the idempotency match is unit-tested without ffprobe.
fn subtitle_titles(listing: &str) -> Vec<String> {
    listing.lines().filter_map(|line| line.strip_prefix("TAG:title=").map(str::to_string)).collect()
}

/// `(subtitle-stream count, their titles)` for `file`, in one ffprobe pass. `None` if ffprobe
/// couldn't run — the caller then leaves the file untouched rather than guess. The count places
/// new tracks at the right output index when muxing; the titles are the idempotency check.
fn subtitle_streams(file: &Path, env: Env) -> Option<(usize, Vec<String>)> {
    let ffprobe = env
        .ffmpeg_dir
        .map(|dir| dir.join("ffprobe").into_os_string())
        .unwrap_or_else(|| "ffprobe".into());
    let listing = capture_stdout(
        &ffprobe,
        [
            OsString::from("-v"), "error".into(),
            "-select_streams".into(), "s".into(),
            "-show_entries".into(), "stream=index:stream_tags=title".into(),
            "-of".into(), "default=nw=1".into(),
            file.as_os_str().to_owned(),
        ],
    )?;
    let count = listing.lines().filter(|line| line.starts_with("index=")).count();
    Some((count, subtitle_titles(&listing)))
}

/// The title an embedded track carries for `pick` — the auto-generated stamp for an auto track,
/// the plain name otherwise. This is what [`embed_subtitles`] matches on and writes.
fn subtitle_title(pick: &Pick) -> String {
    if pick.auto {
        stamped_title(&pick.name)
    } else {
        pick.name.clone()
    }
}

/// The fixed, id-based stem the subtitle patch fetches its sidecars to (`.dlsub-<id>.<key>.vtt`) —
/// deliberately NOT the video's own filename, so a title/date drift between the on-disk file and
/// yt-dlp's current template can't hide the freshly-fetched tracks.
fn subtitle_sidecar(into: &Path, id: &str, key: &str) -> PathBuf {
    into.join(format!(".dlsub-{id}.{key}.vtt"))
}

/// Fetch `keys`' subtitle sidecars for `url` (bundled yt-dlp, no media), written to the fixed
/// [`subtitle_sidecar`] paths. Returns whether yt-dlp ran.
fn fetch_subtitles(url: &str, id: &str, keys: &[String], into: &Path, env: Env) -> bool {
    let mut argv = seeded(env);
    argv.extend([
        OsString::from("--skip-download"),
        "--write-subs".into(),
        "--write-auto-subs".into(),
        "--sub-langs".into(),
        keys.join(",").into(),
        "--no-playlist".into(),
        "--output".into(),
        into.join(format!(".dlsub-{id}.%(ext)s")).into_os_string(),
        url.into(),
    ]);
    let (program, args) = ytdlp_invocation(argv);
    capture_stdout(program, args).is_some()
}

/// Mux the arrived subtitle sidecars into the mkv `file` in one ffmpeg pass, keeping every existing
/// stream and tagging each new track's language + title. `existing` is the file's current subtitle
/// count, so the new tracks land at the right output indices. An attached cover can't just ride
/// `-map 0` — a remux demotes it to a plain video track (the `attached_pic` disposition doesn't
/// survive an mkv round-trip) — so it's extracted first, excluded from the map, and re-`-attach`ed
/// in the same pass. Renames over the original. Returns success.
fn mux_subtitles(file: &Path, arrived: &[(&Pick, PathBuf)], existing: usize, env: Env) -> bool {
    let ffmpeg = env
        .ffmpeg_dir
        .map(|dir| dir.join("ffmpeg").into_os_string())
        .unwrap_or_else(|| "ffmpeg".into());
    let cover_index = embedded_thumbnail_index(file, env).flatten();
    // Extraction failing (odd, but possible) degrades to the old demote-the-cover behaviour —
    // the exclusion below is applied only when the re-attach is actually in hand.
    let cover = cover_index.and_then(|index| extract_thumbnail(file, index, env));
    let out = file.with_extension("subbing.mkv");
    let mut argv: Vec<OsString> = ["-v", "error", "-y", "-i"].map(OsString::from).to_vec();
    argv.push(file.as_os_str().to_owned());
    for (_, sidecar) in arrived {
        argv.push("-i".into());
        argv.push(sidecar.as_os_str().to_owned());
    }
    if let Some(tmp) = &cover {
        argv.push("-attach".into());
        argv.push(tmp.as_os_str().to_owned());
    }
    argv.extend(["-map", "0"].map(OsString::from));
    if let (Some(index), Some(_)) = (cover_index, &cover) {
        argv.push("-map".into());
        argv.push(format!("-0:{index}").into());
    }
    for input in 1..=arrived.len() {
        argv.push("-map".into());
        argv.push(input.to_string().into());
    }
    argv.extend(["-c", "copy", "-c:s", "srt"].map(OsString::from));
    if cover.is_some() {
        argv.extend(
            ["-metadata:s:t:0", "mimetype=image/jpeg", "-metadata:s:t:0", "filename=cover.jpg"]
                .map(OsString::from),
        );
    }
    for (offset, (pick, _)) in arrived.iter().enumerate() {
        let idx = existing + offset;
        let lang = pick.key.split('-').next().unwrap_or(&pick.key);
        argv.push(format!("-metadata:s:s:{idx}").into());
        argv.push(format!("language={lang}").into());
        argv.push(format!("-metadata:s:s:{idx}").into());
        argv.push(format!("title={}", subtitle_title(pick)).into());
    }
    argv.push(out.as_os_str().to_owned());
    let ok = matches!(
        std::process::Command::new(&ffmpeg).args(&argv).status(),
        Ok(status) if status.success()
    ) && std::fs::rename(&out, file).is_ok();
    if let Some(tmp) = &cover {
        let _ = std::fs::remove_file(tmp); // the extracted cover was only for the re-attach
    }
    if !ok {
        let _ = std::fs::remove_file(&out);
    }
    ok
}

/// The late, opt-in (`--subtitles`) subtitle patch pass for one video: report which of its
/// `expected` tracks are already embedded, then fetch + mux only the missing ones. Idempotent (a
/// track whose title is already present is never re-fetched) and archive-independent (so a re-run
/// patches an already-downloaded video). Video-only: an mkv holds subtitle streams; audio files
/// carry their subtitles as metadata tags from the original download, so they're skipped here.
fn embed_subtitles(into: &Path, id: &str, url: &str, expected: &[Pick], env: Env) {
    let Some(file) = find_by_id(into, id) else {
        println!("  [{id}]: {}", doc_style::problematic("no file found — skipping"));
        return;
    };
    if file.extension().and_then(|ext| ext.to_str()) != Some("mkv") {
        println!("  [{id}]: {}", doc_style::approved("audio — subtitles kept as tags; nothing to patch"));
        return;
    }
    let Some((count, embedded)) = subtitle_streams(&file, env) else {
        println!("  [{id}]: {}", doc_style::problematic("could not read (ffprobe) — skipping"));
        return;
    };
    let missing: Vec<&Pick> = expected
        .iter()
        .filter(|pick| {
            let title = subtitle_title(pick);
            !title.is_empty() && !embedded.contains(&title)
        })
        .collect();
    if missing.is_empty() {
        // "all 0 expected already embedded" would read absurd — a subtitle-less video gets its
        // own honest wording.
        let message = if expected.is_empty() {
            "no subtitles exist for this video — nothing to embed".to_string()
        } else {
            format!("all {} expected subtitle(s) already embedded", expected.len())
        };
        println!("  [{id}]: {}", doc_style::approved(&message));
        return;
    }
    let want: Vec<&str> = missing.iter().map(|pick| pick.key.as_str()).collect();
    println!("  [{id}]: {}", doc_style::problematic(&format!("missing subtitle(s): {}", want.join(", "))));
    let keys: Vec<String> = missing.iter().map(|pick| pick.key.clone()).collect();
    if !fetch_subtitles(url, id, &keys, into, env) {
        eprintln!("  [{id}]: {}", doc_style::problematic("could not fetch subtitles"));
        return;
    }
    let arrived: Vec<(&Pick, PathBuf)> = missing
        .iter()
        .filter_map(|pick| {
            let sidecar = subtitle_sidecar(into, id, &pick.key);
            sidecar.is_file().then_some((*pick, sidecar))
        })
        .collect();
    if arrived.is_empty() {
        eprintln!("  [{id}]: {}", doc_style::problematic("no subtitles arrived (rate-limited?)"));
        return;
    }
    let ok = mux_subtitles(&file, &arrived, count, env);
    for (_, sidecar) in &arrived {
        let _ = std::fs::remove_file(sidecar);
    }
    if ok {
        let done: Vec<&str> = arrived.iter().map(|(pick, _)| pick.key.as_str()).collect();
        println!("  [{id}]: {}", doc_style::approved(&format!("embedded {}", done.join(", "))));
    } else {
        eprintln!("  [{id}]: {}", doc_style::problematic("could not embed subtitles"));
    }
}

/// The collection `--subtitles` pass: one batch probe for every entry's expected tracks, then
/// [`embed_subtitles`] per entry — ignoring the archive, so already-downloaded entries get patched
/// too. `url` is the playlist/tab the entries belong to; `home` holds their files.
fn patch_collection_subtitles(url: &str, entries: &[&ScanEntry], home: &Path, env: Env) {
    if entries.is_empty() {
        return;
    }
    println!("subtitles: scanning {} video(s)…", entries.len());
    let indexes: Vec<String> = entries.iter().map(|entry| entry.index.clone()).collect();
    for plan in batch_probe(url, &indexes, env) {
        let watch = format!("https://www.youtube.com/watch?v={}", plan.id);
        embed_subtitles(home, &plan.id, &watch, &plan.picks, env);
    }
}

/// The downloaded file whose name carries `__<id>.` — the id rides in every output template
/// precisely so files stay findable. A shallow recursive walk under `root`.
fn find_by_id(root: &Path, id: &str) -> Option<PathBuf> {
    let marker = format!("__{id}.");
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|name| {
                let name = name.to_string_lossy();
                name.contains(&marker)
                    && [".mkv", ".opus", ".m4a", ".mp3", ".ogg", ".flac", ".wav", ".webm"]
                        .iter()
                        .any(|ext| name.ends_with(ext))
            }) {
                return Some(path);
            }
        }
    }
    None
}

/// Channel mode: every tab in turn. The archive lives inside the channel's own folder (shared
/// by its tabs); the video-bearing tabs download entry by entry like a playlist, and the
/// playlists tab recurses into [`download_playlist`] per playlist — each with its own folder,
/// archive, and unplayable report, nested under `<channel>/playlists/`. A failing tab (most
/// channels lack a few) doesn't stop the rest; the run counts as success if any came through.
pub(crate) fn download_channel(root: &str, into: &Path, env: Env) -> i32 {
    let mut home: Option<PathBuf> = None; // settled by the first readable tab's scan
    let mut succeeded = 0;
    for tab in CHANNEL_TABS {
        let tab_url = format!("{root}/{tab}");
        println!("=== {tab_url} ===");
        let scan = match scan_tab(&tab_url, "%(uploader)S[%(channel_id)S]", env) {
            TabScan::Found(scan) => scan,
            TabScan::Missing => {
                println!("the channel has no `{tab}` tab");
                continue;
            }
            TabScan::Failed => {
                eprintln!("dl: could not read the `{tab}` tab — moving on");
                continue;
            }
        };
        let home = home
            .get_or_insert_with(|| {
                // The channel's own folder holds the archive all tabs share; an unprobeable
                // name degrades to a shared root archive.
                let dir = match &scan.dirname {
                    Some(dir) => into.join(dir),
                    None => into.to_path_buf(),
                };
                let _ = std::fs::create_dir_all(&dir);
                dir
            })
            .clone();
        if *tab == "playlists" {
            // Entries here are playlists, not videos — recurse, nesting under the channel.
            let nest = home.join("playlists");
            for entry in &scan.entries {
                println!("--- playlist: {} ---", entry.title);
                let url = format!("https://www.youtube.com/playlist?list={}", entry.id);
                if download_playlist(&url, &entry.id, &nest, env) == 0 {
                    succeeded += 1;
                }
            }
            continue;
        }
        let archived = archived_ids(&home.join(ARCHIVE_NAME));
        let pending: Vec<&ScanEntry> =
            scan.entries.iter().filter(|entry| !archived.contains(&entry.id)).collect();
        let skipped = scan.entries.len() - pending.len();
        if skipped > 0 {
            println!("{skipped} entries already archived — skipped");
        }
        let tab_code = if pending.is_empty() {
            0
        } else {
            download_pending(&tab_url, &pending, into, &home, env, |url, into, home, env, langs, items| {
                channel_tab_argv(url, tab, into, home, env, langs, items)
            })
        };
        // Late thumbnail pass (opt-in) over the whole tab — archived entries included, so a re-run
        // with `--thumbnail` patches previously-downloaded videos.
        if env.thumbnail {
            let ids: Vec<String> = scan.entries.iter().map(|entry| entry.id.clone()).collect();
            embed_thumbnails(&home, &ids, env);
        }
        if env.subtitles {
            let entries: Vec<&ScanEntry> = scan.entries.iter().collect();
            patch_collection_subtitles(&tab_url, &entries, &home, env);
        }
        if tab_code == 0 {
            succeeded += 1;
        }
    }
    i32::from(succeeded == 0)
}

/// The `-t/--taglist` listing: the notable yt-dlp flags first, styled for a quick scan, then
/// yt-dlp's own full option list (the repetition is fine — this is the index, that's the book).
pub(crate) fn taglist() -> i32 {
    println!("{}", doc_style::_header("Notable yt-dlp flags — pass them after `--`:"));
    let width = NOTABLE_FLAGS.iter().map(|(flag, _)| flag.len()).max().unwrap_or(0);
    for (flag, blurb) in NOTABLE_FLAGS {
        let pad = " ".repeat(width - flag.len());
        println!("  {}{pad}  {blurb}", theme_code::argname(flag));
    }
    println!("\n{}", doc_style::_header("Everything yt-dlp accepts:"));
    let (program, args) = ytdlp_invocation(vec![OsString::from("--help")]);
    run_reporting_code(program, args)
}

/// The flags worth knowing about, shown by [`taglist`]: `(flag as you'd type it, what it does)`.
pub(crate) const NOTABLE_FLAGS: &[(&str, &str)] = &[
    ("--sponsorblock-remove sponsor", "cut sponsored segments out of the video"),
    ("--write-description", "save the description as a sidecar file"),
    ("--write-info-json", "save every scrap of metadata as JSON"),
    ("--playlist-items 3,5-7", "download only a slice of a playlist"),
    ("--limit-rate 2M", "cap the download speed"),
    ("--merge-output-format webm/mkv", "prefer webm (forfeits embedded cover art)"),
    ("--sub-langs all", "every real subtitle language, not just EN"),
    ("--proxy URL", "route through a proxy"),
];

// --- subtitle resolution ---------------------------------------------------------

/// One subtitle track the plan requests: its yt-dlp language key, YouTube's display name for
/// it (which becomes the embedded track title — what the post-pass matches on), and whether
/// it's auto-generated rather than the uploader's.
#[derive(Clone, Debug, PartialEq)]
struct Pick {
    key: String,
    name: String,
    auto: bool,
}

/// The subtitle tracks to embed for one video, given what exists. The policy: every language
/// the uploader published; the video's native language as auto-captions when the uploader
/// published none in it (the raw `-orig` track, else the plain one); and EN — real, else
/// auto-translated (the `en-<lang>` pair when published, plain `en` otherwise) — unless a real
/// or native EN already covers it. Nothing is ever doubled, and every auto pick is flagged so
/// [`mark_auto_titles`] can stamp its embedded title.
///
/// Every pick is a MUST-TRY, never a must-have — EN above all: YouTube lists auto-translations
/// for every video but its translation endpoint is aggressively rate-limited (429s, sometimes
/// permanently on shared/VPN IPs), so a listed track may never be served. `--ignore-errors`
/// rides every download exactly so a refused subtitle downgrades to a warning and the video
/// completes without it — don't "fix" a sub-only failure into a failed exit.
fn sub_picks_for(
    reals: &[(String, String)],
    autos: &[(String, String)],
    orig: Option<&str>,
) -> Vec<Pick> {
    let base = |lang: &str| lang.split('-').next().unwrap_or(lang).to_lowercase();
    let auto_pick = |key: &str| {
        autos.iter().find(|(auto_key, _)| auto_key == key).map(|(auto_key, name)| Pick {
            key: auto_key.clone(),
            name: if name.is_empty() { auto_key.clone() } else { name.clone() },
            auto: true,
        })
    };
    let mut picks: Vec<Pick> = reals
        .iter()
        .filter(|(key, _)| key != "live_chat")
        .map(|(key, name)| Pick { key: key.clone(), name: name.clone(), auto: false })
        .collect();
    let native = orig.filter(|orig| !orig.is_empty()).map(&base);
    if let Some(native) = &native {
        if !picks.iter().any(|pick| base(&pick.key) == *native) {
            if let Some(pick) = auto_pick(&format!("{native}-orig")).or_else(|| auto_pick(native))
            {
                picks.push(pick);
            }
        }
    }
    if !picks.iter().any(|pick| base(&pick.key) == "en") {
        let translated = native.as_ref().and_then(|native| auto_pick(&format!("en-{native}")));
        if let Some(pick) = translated.or_else(|| auto_pick("en")) {
            picks.push(pick);
        }
    }
    picks
}

/// The pre-probe fallback (and the no-probe path): the EN-family behavior — not flagged auto,
/// since without a probe nobody knows, and a wrong "(auto-generated)" stamp is worse than none.
fn default_picks() -> Vec<Pick> {
    ["en", "en-US", "en-GB"]
        .map(|key| Pick { key: key.to_string(), name: String::new(), auto: false })
        .to_vec()
}

/// One video's subtitle plan plus its id (the post-pass finds the file by it). An unprobeable
/// video falls back to the EN default — resilience over completeness.
fn video_picks(url: &str, env: Env) -> (Option<String>, Vec<Pick>) {
    let mut argv = seeded(env);
    argv.extend(
        [
            "--print", "%(id)s",
            "--print", "%(language)s",
            "--print", "%(subtitles)j",
            "--print", "%(automatic_captions)j",
        ]
        .map(OsString::from),
    );
    argv.push(url.into());
    let (program, args) = ytdlp_invocation(argv);
    let Some(out) = capture_stdout(program, args) else {
        return (None, default_picks());
    };
    let mut lines = out.lines();
    let id = lines.next().map(str::trim).filter(|id| !id.is_empty() && *id != "NA");
    let (Some(language), Some(subs), Some(autos)) = (lines.next(), lines.next(), lines.next())
    else {
        return (id.map(str::to_string), default_picks());
    };
    let orig = Some(language.trim()).filter(|lang| !lang.is_empty() && *lang != "NA");
    let picks = sub_picks_for(&json_lang_names(subs), &json_lang_names(autos), orig);
    (id.map(str::to_string), picks)
}

/// Per-entry print format of the batch subtitle probe.
const PROBE_FORMAT: &str =
    "%(playlist_index)s\t%(id)s\t%(language)s\t%(subtitles)j\t%(automatic_captions)j";

/// Probes larger than this pace themselves with `--sleep-requests` (small ones finish before
/// any rate-limiter would care).
const PROBE_PACING_THRESHOLD: usize = 20;

/// One pending entry with its computed subtitle plan.
struct Planned {
    index: String,
    id: String,
    picks: Vec<Pick>,
}

/// Probe the subtitle situation of many entries in ONE yt-dlp invocation (process startup and
/// player work are the expensive parts — per-entry probes multiply them). Entries that fail to
/// extract are simply absent; callers fall back to [`default_picks`] for those.
fn batch_probe(url: &str, indexes: &[String], env: Env) -> Vec<Planned> {
    let mut argv = seeded(env);
    argv.push("--ignore-errors".into());
    if indexes.len() > PROBE_PACING_THRESHOLD {
        // A big probe is a burst of metadata requests with no downloads between them to slow
        // the rate; pace it below YouTube's radar (~24/min measured) rather than risk the IP
        // getting flagged before the first byte of video downloads.
        argv.extend(["--sleep-requests", "1"].map(OsString::from));
    }
    argv.extend([
        OsString::from("--playlist-items"), indexes.join(",").into(),
        "--print".into(), PROBE_FORMAT.into(),
        url.into(),
    ]);
    let (program, args) = ytdlp_invocation(argv);
    capture_stdout(program, args).map(|out| parse_batch(&out)).unwrap_or_default()
}

/// Parse [`batch_probe`]'s lines into per-entry plans.
fn parse_batch(out: &str) -> Vec<Planned> {
    out.lines()
        .filter_map(|line| {
            let mut fields = line.splitn(5, '\t');
            let (index, id, language, subs, autos) =
                (fields.next()?, fields.next()?, fields.next()?, fields.next()?, fields.next()?);
            let orig = (!language.is_empty() && language != "NA").then_some(language);
            Some(Planned {
                index: index.to_string(),
                id: id.to_string(),
                picks: sub_picks_for(&json_lang_names(subs), &json_lang_names(autos), orig),
            })
        })
        .collect()
}

/// Group planned entries by their subtitle KEY list, so each distinct list becomes ONE download
/// invocation (`--playlist-items` takes a comma list) instead of one per entry. Auto flags may
/// differ within a group (the same `en` can be real for one entry, translated for another) —
/// that's per-entry data the post-pass reads from each [`Planned`].
fn group_by_langs(planned: &[Planned]) -> Vec<Vec<&Planned>> {
    let keys = |plan: &Planned| plan.picks.iter().map(|pick| pick.key.clone()).collect::<Vec<_>>();
    let mut groups: Vec<(Vec<String>, Vec<&Planned>)> = Vec::new();
    for plan in planned {
        let plan_keys = keys(plan);
        match groups.iter_mut().find(|(group_keys, _)| *group_keys == plan_keys) {
            Some((_, members)) => members.push(plan),
            None => groups.push((plan_keys, vec![plan])),
        }
    }
    groups.into_iter().map(|(_, members)| members).collect()
}

/// The top-level keys of a JSON object paired with the first `"name"` string inside each
/// key's value — dependency-free, same walking technique as [`json_top_keys`]. Fits yt-dlp's
/// `%(subtitles)j` shape: `{"en": [{"url": …, "name": "English"}, …], …}`; a key whose value
/// carries no name gets `""`.
fn json_lang_names(json: &str) -> Vec<(String, String)> {
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut depth = 0u32;
    let mut in_string: Option<String> = None;
    let mut escaped = false;
    let mut pending: Option<(u32, String)> = None; // a closed string, with its depth
    let mut await_name_value = false; // the last string was a `"name"` key inside a value
    for ch in json.chars() {
        if let Some(buffer) = in_string.as_mut() {
            if escaped {
                buffer.push(ch);
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                let text = in_string.take().unwrap();
                if await_name_value {
                    await_name_value = false;
                    if let Some((_, name)) = entries.last_mut() {
                        if name.is_empty() {
                            *name = text;
                        }
                    }
                } else {
                    pending = Some((depth, text));
                }
            } else {
                buffer.push(ch);
            }
            continue;
        }
        match ch {
            '"' => in_string = Some(String::new()),
            ':' => match pending.take() {
                Some((1, key)) => entries.push((key, String::new())),
                Some((_, key)) if key == "name" => await_name_value = true,
                _ => {}
            },
            ' ' | '\t' | '\n' | '\r' => {}
            '{' | '[' => {
                depth += 1;
                pending = None;
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                pending = None;
            }
            _ => {
                pending = None;
                await_name_value = false;
            }
        }
    }
    entries
}

// --- argv assembly ---------------------------------------------------------------

/// The file name every YouTube mode shares: sortable upload date, title, and the video id that
/// keeps any file traceable back to its source (ideas kept from the old dl_youtube.py).
const YT_NAME: &str = "%(upload_date)s__%(title)s__%(id)s.%(ext)s";

/// Output template for a generic (non-YouTube) single download: title + id, flat under the
/// destination. Simpler than [`YT_NAME`] — a random site rarely carries a reliable upload date,
/// and there's no collection to sort into. The `[id]` also gives [`scrub_ledger`] a key to match.
const GENERIC_NAME: &str = "%(title)s [%(id)s].%(ext)s";

/// The download-archive's file name, dropped inside whatever folder owns the collection. Named
/// for the `dl` command, not YouTube — every platform's downloads log here now.
const ARCHIVE_NAME: &str = ".dl_video_archive.txt";

/// The flags every yt-dlp run shares: keep going past broken entries, parallel fragments, the
/// video's requested subtitles fetched-and-embedded (sidecars cleaned up), its title/uploader/
/// date/description and thumbnail embedded as tags and cover art (thumbnail converted to jpg —
/// the one format the mkv cover-art convention reliably recognizes), mkv output, and a
/// download-archive (at `archive_dir`) so interrupted or repeated runs resume instead of
/// redoing. yt-dlp marks the archive only after post-processing finishes — verified by racing
/// the archive file against the output directory.
///
/// mkv beats webm as the merge container even though both box the same codecs at the same
/// quality: mkv accepts any codec yt-dlp picks (h264 fallbacks included) and embeds the
/// thumbnail, where webm refuses attachments and leaves the art as loose image files.
fn common(archive_dir: &Path, env: Env, langs: &[String]) -> Vec<OsString> {
    // `--ignore-errors` also keeps subtitle picks must-try: a 429'd track warns and is skipped
    // without failing the video (see `sub_picks_for`).
    let mut argv: Vec<OsString> = ["--ignore-errors", "--concurrent-fragments", "4"]
        .into_iter()
        .map(OsString::from)
        .collect();
    argv.extend(ipv4_flag(env));
    if !langs.is_empty() {
        argv.extend(["--write-subs", "--write-auto-subs", "--sub-langs"].map(OsString::from));
        argv.push(langs.join(",").into());
        if !env.audio {
            // Audio mode skips these: subtitles can't embed into an extracted audio track, so
            // the `.vtt` sidecars are deliberately kept for [`embed_subtitle_tags`] to fold
            // into the file's metadata afterwards.
            argv.extend(["--embed-subs", "--compat-options", "no-keep-subs"].map(OsString::from));
        }
        // YouTube rate-limits its subtitle endpoint (429s, particularly on auto-translations);
        // a short pause per subtitle fetch stays under its radar and only taxes subbed videos.
        argv.extend(["--sleep-subtitles", "2"].map(OsString::from));
    }
    argv.extend(
        [
            "--embed-metadata", "--embed-chapters",
            "--merge-output-format", "mkv",
            "--download-archive",
        ]
        .map(OsString::from),
    );
    argv.push(archive_dir.join(ARCHIVE_NAME).into_os_string());
    if let Some(dir) = env.ffmpeg_dir {
        // yt-dlp only finds PATH ffmpegs on its own — point it at the bundled build.
        argv.push("--ffmpeg-location".into());
        argv.push(dir.as_os_str().to_owned());
    }
    if let Some(deno) = env.js_runtime {
        // Same story for the JS runtime: yt-dlp only searches PATH for deno.
        argv.extend(js_runtime_flag(deno));
    }
    argv.extend(cookie_args(env));
    if env.audio {
        argv.push("-x".into());
    }
    if let Some(height) = env.res {
        argv.push("-S".into());
        argv.push(format!("res:{height}").into());
    }
    argv
}

/// How to invoke yt-dlp: the bundled zipapp is run through the bundled python *explicitly* —
/// its `env python3` shebang would otherwise pick whatever python the caller's PATH offers,
/// and the curl_cffi impersonation support lives in ours. A system yt-dlp runs as itself.
fn ytdlp_invocation(args: Vec<OsString>) -> (OsString, Vec<OsString>) {
    let ytdlp = crate::tools::resolve("yt-dlp");
    if ytdlp == "yt-dlp" {
        return (ytdlp, args);
    }
    let mut full = vec![ytdlp];
    full.extend(args);
    (crate::tools::resolve("python3"), full)
}

fn run(argv: Vec<OsString>) -> i32 {
    let (program, args) = ytdlp_invocation(argv);
    let code = run_reporting_code(program, args);
    if code != 0 {
        // The interpreter's name in the line above obscures the real actor; and the most common
        // hard failure deserves its diagnosis spelled out.
        eprintln!(
            "dl: yt-dlp failed (exit {code}). A `403 Forbidden` on media usually means the site \
             has blocklisted this network's IP (VPN / datacenter exits often are) — switching \
             the node/network tends to fix it, and --cookies is the other lever."
        );
    }
    code
}

/// A lone video, named [`YT_NAME`] directly under the destination (which also holds the
/// archive). `--no-playlist` pins the meaning even when the link happens to carry a `list=`
/// (the `--single` case).
fn video_argv(url: &str, into: &Path, env: Env, langs: &[String]) -> Vec<OsString> {
    let mut argv = common(into, env, langs);
    argv.extend(["--no-playlist", "--output"].map(OsString::from));
    argv.push(into.join(YT_NAME).into_os_string());
    argv.extend(env.extra.iter().map(OsString::from));
    argv.push(url.into());
    argv
}

/// A generic single download: [`common`] with no subtitle list (off YouTube there's no caption
/// matrix to resolve), flat under `into` via [`GENERIC_NAME`]. `--no-playlist` keeps a page that
/// happens to expose a playlist to the one video asked for — override with `-- --yes-playlist`,
/// since `extra` is appended last and wins.
fn generic_argv(url: &str, into: &Path, env: Env) -> Vec<OsString> {
    let mut argv = common(into, env, &[]);
    argv.extend(["--no-playlist", "--output"].map(OsString::from));
    argv.push(into.join(GENERIC_NAME).into_os_string());
    argv.extend(env.extra.iter().map(OsString::from));
    argv.push(url.into());
    argv
}

/// A playlist entry (or, `item`-less, a whole playlist in one pass — the scanless fallback):
/// its own `title[id]` folder under `into`, entries ordered by playlist position, the archive
/// at `archive_dir`.
fn playlist_argv(
    url: &str,
    into: &Path,
    archive_dir: &Path,
    env: Env,
    langs: &[String],
    item: Option<&str>,
) -> Vec<OsString> {
    let mut argv = common(archive_dir, env, langs);
    if let Some(index) = item {
        argv.extend([OsString::from("--playlist-items"), index.into()]);
    }
    argv.push("--output".into());
    argv.push(
        into.join(format!("%(playlist_title)s[%(playlist_id)s]/%(playlist_index)03d__{YT_NAME}"))
            .into_os_string(),
    );
    argv.extend(env.extra.iter().map(OsString::from));
    argv.push(url.into());
    argv
}

/// The channel tabs, each downloaded into its own folder under `uploader[channel_id]/`; the
/// playlists tab is handled by recursion instead (see [`download_channel`]).
const CHANNEL_TABS: &[&str] = &["videos", "shorts", "streams", "playlists"];

fn channel_tab_argv(
    tab_url: &str,
    tab: &str,
    into: &Path,
    archive_dir: &Path,
    env: Env,
    langs: &[String],
    item: &str,
) -> Vec<OsString> {
    let mut argv = common(archive_dir, env, langs);
    argv.extend([OsString::from("--playlist-items"), item.into()]);
    argv.push("--output".into());
    argv.push(
        into.join(format!("%(uploader)s[%(channel_id)s]/{tab}/{YT_NAME}")).into_os_string(),
    );
    argv.extend(env.extra.iter().map(OsString::from));
    argv.push(tab_url.into());
    argv
}

// --- collection scanning ----------------------------------------------------------

/// One playlist entry as the flat scan sees it.
struct ScanEntry {
    index: String,
    id: String,
    title: String,
}

/// A scanned playlist: its title, its folder name (as yt-dlp will spell it), and every entry,
/// dead or alive.
struct PlaylistScan {
    title: String,
    dirname: Option<String>,
    entries: Vec<ScanEntry>,
}

/// Per-entry print format of the flat scan (playlist title rides along on every line).
const SCAN_FORMAT: &str = "%(playlist_index)s\t%(id)s\t%(title)s\t%(playlist_title)s";

/// A channel tab's scan outcome: tabs a channel simply doesn't have are normal, not errors.
enum TabScan {
    Found(PlaylistScan),
    Missing,
    Failed,
}

/// Scan a channel tab with stderr captured, so "this channel has no such tab" — yt-dlp's error,
/// but an everyday reality — turns into a calm message instead of an ERROR dump; anything else
/// on stderr is passed through as the real failure it is.
fn scan_tab(url: &str, dir_template: &str, env: Env) -> TabScan {
    let mut args = seeded(env);
    args.extend([
        OsString::from("--flat-playlist"),
        "--print".into(), SCAN_FORMAT.into(),
        "--print".into(), format!("playlist:{dir_template}").into(),
        url.into(),
    ]);
    let (program, args) = ytdlp_invocation(args);
    let Some((ok, stdout, stderr)) = capture_output(program, args) else {
        return TabScan::Failed;
    };
    if ok {
        if let Some(scan) = parse_scan(&stdout) {
            return TabScan::Found(scan);
        }
        return TabScan::Missing; // reachable but empty: nothing to download either way
    }
    if tab_absence(&stderr) {
        return TabScan::Missing;
    }
    eprint!("{stderr}");
    TabScan::Failed
}

/// Whether yt-dlp's stderr says the tab doesn't exist (its stable phrasing:
/// `ERROR: [youtube:tab] @handle: This channel does not have a streams tab`).
fn tab_absence(stderr: &str) -> bool {
    stderr.contains("does not have a")
}

/// One flat invocation lists the entries AND prints the collection's folder name (playlist
/// scope, sanitized by the `S` conversion in `dir_template` — a channel tab's flat entries
/// often lack `uploader`/`channel_id`, the `NA[NA]` trap, while the tab's own fields don't).
fn scan_playlist(url: &str, dir_template: &str, env: Env) -> Option<PlaylistScan> {
    let mut argv = seeded(env);
    argv.extend([
        OsString::from("--flat-playlist"),
        "--print".into(), SCAN_FORMAT.into(),
        "--print".into(), format!("playlist:{dir_template}").into(),
        url.into(),
    ]);
    let (program, args) = ytdlp_invocation(argv);
    let out = capture_stdout(program, args)?;
    parse_scan(&out)
}

/// Parse the flat scan's tab-separated lines (malformed ones are skipped; `None` only when
/// nothing parsed at all).
fn parse_scan(out: &str) -> Option<PlaylistScan> {
    let mut title = String::new();
    let mut dirname = None;
    let mut entries = Vec::new();
    for line in out.lines() {
        if !line.contains('\t') {
            // The playlist-scope print: a lone tabless line (it arrives after the entries;
            // the last one wins). An `NA` in it means the fields weren't there — no folder.
            let line = line.trim();
            if !line.is_empty() && !line.contains("NA") {
                dirname = Some(line.to_string());
            }
            continue;
        }
        let mut fields = line.split('\t');
        let (Some(index), Some(id), Some(entry_title)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if let Some(playlist_title) = fields.next() {
            title = playlist_title.to_string();
        }
        entries.push(ScanEntry {
            index: index.to_string(),
            id: id.to_string(),
            title: entry_title.to_string(),
        });
    }
    (!entries.is_empty()).then_some(PlaylistScan { title, dirname, entries })
}

/// The video ids a download-archive has recorded (its lines are `<extractor> <id>` — the
/// extractor prefix is yt-dlp's own on-disk format, kept so it can read the file back).
fn archived_ids(path: &Path) -> HashSet<String> {
    std::fs::read_to_string(path)
        .map(|content| {
            content
                .lines()
                .filter_map(|line| line.split_whitespace().nth(1).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Regions tried in order when a download proves geo-blocked — `--xff` header spoofing
/// defeats softly-enforced blocks.
const XFF_REGIONS: &[&str] =
    &["US", "GB", "DE", "FR", "NL", "SE", "CA", "AU", "JP", "KR", "BR", "IN"];

/// How [`diagnose_failure`] treats a geo-block, chosen by the caller per site. Header spoofing
/// only shifts a *softly*-enforced block (a `--xff` region the site trusts); YouTube enforces geo
/// by the real connection IP and ignores the header (verified against yt-dlp's source), so a dozen
/// spoofed retries would only waste time — it reports the block instead.
enum GeoRescue {
    /// Walk [`XFF_REGIONS`], retrying each — for sites that may honor `X-Forwarded-For`.
    XffSweep,
    /// Report the block without retrying — for sites that enforce geo by IP (YouTube).
    IpEnforced,
}

/// The failure ledger, written beside the collection's archive: what stayed undownloaded and
/// why. Entries here are also deliberately NOT archived, so every future run retries them —
/// members-only videos get released publicly later, and blocks lift. Named for `dl`, not
/// YouTube — the generic path writes here too.
const FAILED_LEDGER: &str = ".dl_video_failed_download.txt";

// TODO: scrape historic snapshots (e.g. the WaybackMachine's copies of channel tabs) to find
// videos that were de-listed or disappeared entirely — candidates for the unplayable report and
// the failure ledger, which today only see what YouTube still admits exists.

/// What a failed download turned out to be, judged by yt-dlp's stderr.
#[derive(Debug, PartialEq)]
enum Failure {
    Geo,
    BotWall,
    Members,
    AgeRestricted,
    Sensitive,
    LoginRequired,
    Drm,
    Other,
}

fn classify_failure(stderr: &str) -> Failure {
    // Match case-insensitively against the phrasings yt-dlp's extractors actually emit (verified
    // against their source): the geo notices; the anti-bot/CAPTCHA and JS-challenge walls; the
    // members badge reasons; the YouTube age gate; the login/private/sensitive gates; and
    // report_drm's DRM notice. Order matters where phrasings could overlap — the bot-wall is
    // checked before the login/age gates (YouTube's "…confirm you're not a bot" is a bot-wall,
    // not a login). A missed geo phrasing costs the geo rescue, so that set errs wide; a bare
    // 403 is left as `Other` on purpose (it's ambiguous — rate-limit vs. bot vs. transient —
    // and we won't mislabel it).
    let s = stderr.to_lowercase();
    let has = |needle: &str| s.contains(needle);
    if has("in your country") || has("from your location") || has("in your region") || has("geo restrict") || has("geo_restrict") {
        Failure::Geo
    } else if has("not a bot") || has("captcha") || has("unusual traffic") || has("solve js challenge") || has("challenge data") || has("verify you are human") || has("verify you're human") {
        Failure::BotWall
    } else if has("members-only") || has("members only") || has("channel's members") || has("join this channel") {
        Failure::Members
    } else if has("confirm your age") || has("age-restricted") || has("age-verification") || has("age_check_required") {
        Failure::AgeRestricted
    } else if has("for some audiences") || has("not be comfortable") {
        Failure::Sensitive
    } else if has("requiring login") || has("log in for access") || has("log into an account") || has("permission to view") || has("account is private") || has("private video") {
        Failure::LoginRequired
    } else if has("drm protected") || has("drm-protected") || has("protected by drm") {
        Failure::Drm
    } else {
        Failure::Other
    }
}

fn capture_ytdlp(argv: Vec<OsString>) -> Option<(bool, String, String)> {
    let (program, args) = ytdlp_invocation(argv);
    capture_output(program, args)
}

/// Re-run a failed download with output captured to learn WHY (group runs stream live and keep no
/// stderr) and return the ledger line for whatever stays dead (`None` when a retry succeeded).
/// A geo-block is handled per `rescue`: [`GeoRescue::XffSweep`] walks [`XFF_REGIONS`] for sites
/// that may honor a spoofed `X-Forwarded-For`; [`GeoRescue::IpEnforced`] (YouTube) reports it
/// without retrying, since YouTube reads the real connection IP and ignores the header. A terminal
/// failure is also announced on stderr as it's decided, so the reason shows in the run's output —
/// not only later in the ledger.
fn diagnose_failure(base: Vec<OsString>, label: &str, rescue: GeoRescue) -> Option<String> {
    let Some((ok, _, stderr)) = capture_ytdlp(base.clone()) else {
        return Some(dead(format!("{label} — failed (could not even re-run yt-dlp)")));
    };
    if ok {
        println!("{label}: succeeded on retry");
        return None;
    }
    // Whether the failed attempt already carried cookies — the login/age gates give honest advice
    // from this (don't say "add cookies" when they were already there).
    let had_cookies = base.iter().any(|a| a == "--cookies" || a == "--cookies-from-browser");
    match classify_failure(&stderr) {
        Failure::Geo => match rescue {
            // Sites that may honor a spoofed X-Forwarded-For: try each region, take the first win.
            GeoRescue::XffSweep => {
                for region in XFF_REGIONS {
                    println!("{label}: geo-blocked — trying region {region}…");
                    let mut spoofed = base.clone();
                    spoofed.push("--xff".into());
                    spoofed.push((*region).into());
                    if matches!(capture_ytdlp(spoofed), Some((true, _, _))) {
                        println!("{label}: region {region} worked");
                        return None;
                    }
                }
                Some(dead(format!("{label} — geo-blocked (tried {})", XFF_REGIONS.join(","))))
            }
            // YouTube reads the real connection IP and ignores X-Forwarded-For, so spoofing can't
            // move the block — don't burn a dozen retries; report it and the only real fix.
            GeoRescue::IpEnforced => Some(dead(format!(
                "{label} — geo-blocked (enforced by IP; retry from a VPN or --proxy with an IP in an allowed region)"
            ))),
        },
        Failure::BotWall => Some(dead(bot_wall_line(label))),
        Failure::Members => {
            Some(dead(format!("{label} — members-only (channels often release these publicly later)")))
        }
        Failure::AgeRestricted => Some(dead(age_restricted_line(label, had_cookies))),
        Failure::Sensitive => Some(dead(sensitive_content_line(label, had_cookies))),
        Failure::LoginRequired => Some(dead(login_required_line(label, had_cookies))),
        Failure::Drm => Some(dead(drm_line(label, had_cookies))),
        Failure::Other => {
            let detail = stderr
                .lines()
                .find(|line| line.contains("ERROR"))
                .unwrap_or("unknown error")
                .trim();
            Some(dead(format!("{label} — failed: {detail}")))
        }
    }
}

/// Announce a terminal download failure on stderr and hand the same text back for the ledger,
/// so the reason shows in the live run and is recorded in one move.
fn dead(line: String) -> String {
    eprintln!("dl: {line}");
    line
}

/// The ledger line for an age-restricted block, tailored to whether cookies were already tried.
/// Without cookies it's a plain nudge to add them; with cookies the gate is the harder kind — it
/// needs a signed-in *18+* account, and YouTube age-verifies the browser *session*, so the fix is
/// to verify in that browser and re-import, not to repeat "add cookies" when the user already did.
/// The cookies-present line also names the one lever past that (per the cookie research, entitled
/// cookies are necessary but not always sufficient without a PO token — not integrated yet).
fn age_restricted_line(label: &str, had_cookies: bool) -> String {
    if had_cookies {
        format!("{label} — age-restricted despite cookies (use a signed-in 18+ account; play it in that browser once to verify age, then re-import; past that, the last lever is a PO-token provider — not integrated yet)")
    } else {
        format!("{label} — age-restricted (needs cookies from a signed-in 18+ account: dl --cookie-import youtube, then retry)")
    }
}

/// The ledger line for content behind a login / private / sensitive gate, tailored to whether
/// cookies were already tried. Unlike a bot-wall this IS solvable — cookies from an account with
/// access are the fix — so without them it points at the import; with them it means the account
/// lacks access or the cookies went stale, not that a plain retry would help.
fn login_required_line(label: &str, had_cookies: bool) -> String {
    if had_cookies {
        format!("{label} — still blocked with cookies (the account may lack access, or the cookies went stale — re-import from a browser where it plays)")
    } else {
        format!("{label} — needs an account with access: import cookies (dl --cookie-import <site>), then retry")
    }
}

/// The ledger line for a post flagged "sensitive" / not-for-all-audiences (TikTok's "may not be
/// comfortable for some audiences"). Solvable like a login gate — an account allowed to view it —
/// but the account itself must be permitted (18+ / mature content enabled), so with cookies already
/// present the fix is that account condition, not another plain retry.
fn sensitive_content_line(label: &str, had_cookies: bool) -> String {
    if had_cookies {
        format!("{label} — flagged sensitive and still blocked with cookies (the account must be allowed to view sensitive/mature content — re-import from a browser where it plays)")
    } else {
        format!("{label} — flagged sensitive (not for all audiences): needs a signed-in account allowed to view it (dl --cookie-import <site>), then retry")
    }
}

/// The ledger line for an anti-bot / CAPTCHA / JS-challenge wall. Unlike the login/age gates this
/// has no reliable fix — yt-dlp can't solve a human-verification challenge — so the line is honest
/// that the post may be undownloadable and never tells the user to just try again. It does name
/// every real lever, including the one bashrs doesn't wire up yet (a PO-token provider — per the
/// cookie research, the fourth mitigation besides cookies, IP, and yt-dlp freshness).
fn bot_wall_line(label: &str) -> String {
    format!("{label} — blocked by an anti-bot/CAPTCHA challenge yt-dlp can't solve; may be undownloadable (fresh cookies from a browser where it plays, a matching IP, and current yt-dlp sometimes help; the last lever is a PO-token provider — not integrated yet)")
}

/// The ledger line for DRM-protected content, tailored to whether cookies were already tried.
/// yt-dlp never circumvents DRM — but per the cookie research, YouTube's `tv` client serves
/// DRM'd formats to cookie-less requests while ANY cookies (even a logged-out browser session)
/// surface non-DRM formats. So without cookies, "import and retry" is a genuine fix, not a
/// platitude; with them, it's the real thing and honesty beats a retry loop.
fn drm_line(label: &str, had_cookies: bool) -> String {
    if had_cookies {
        format!("{label} — DRM-protected even with cookies (yt-dlp doesn't circumvent DRM — undownloadable)")
    } else {
        format!("{label} — DRM-protected formats only; cookies sometimes unlock non-DRM variants (on YouTube even a logged-out session works): dl --cookie-import <site>, then retry")
    }
}

/// Whether a ledger line refers to video `id` — via the bracketed `[id]` every writer now
/// emits, the `=id` shape of a URL label (`watch?v=id`), or a legacy bare-id label opening the
/// line. Deliberately delimited forms rather than raw substring: a short id from a non-YouTube
/// extractor must not match mid-word inside some other entry's title.
fn ledger_line_refers(line: &str, id: &str) -> bool {
    line.contains(&format!("[{id}]"))
        || line.contains(&format!("={id}"))
        || line.starts_with(&format!("{id} "))
}

/// Clear ledger entries whose videos have since downloaded — the archive is the proof of
/// success. Runs after every download pass, so a lifted geo-block or a members-only video the
/// channel later released drops off the list the moment its retry lands (entries are left
/// unarchived precisely so reruns keep retrying them). Timestamp headers left with no entries
/// go too, and a fully-cleared ledger file is removed. One line per entry keeps this a
/// line-filter; entries whose label carries no id (non-YouTube URL labels) stay until pruned
/// by hand.
fn scrub_ledger(dir: &Path) {
    let path = dir.join(FAILED_LEDGER);
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    let archived = archived_ids(&dir.join(ARCHIVE_NAME));
    if archived.is_empty() {
        return;
    }
    let mut kept: Vec<&str> = Vec::new();
    let mut cleared = 0usize;
    for line in text.lines() {
        if line.starts_with("── ") {
            // A header whose whole block was cleared is still on top of the stack — replace it.
            if kept.last().is_some_and(|last| last.starts_with("── ")) {
                kept.pop();
            }
            kept.push(line);
        } else if archived.iter().any(|id| ledger_line_refers(line, id)) {
            cleared += 1;
        } else if !line.trim().is_empty() {
            kept.push(line);
        }
    }
    if kept.last().is_some_and(|last| last.starts_with("── ")) {
        kept.pop();
    }
    if cleared == 0 {
        return;
    }
    let outcome = if kept.is_empty() {
        std::fs::remove_file(&path)
    } else {
        std::fs::write(&path, kept.join("\n") + "\n")
    };
    match outcome {
        Ok(()) => println!(
            "{cleared} previously-failed download(s) have since succeeded — cleared from {}{}",
            path.display(),
            if kept.is_empty() { " (nothing left; file removed)" } else { "" },
        ),
        Err(err) => eprintln!("dl: could not rewrite {}: {err}", path.display()),
    }
}

/// Append this run's failures to the ledger and tell the user where it lives.
fn write_ledger(dir: &Path, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    let path = dir.join(FAILED_LEDGER);
    let mut block = format!("── {} ──\n", preferences::datehour_stamp());
    for line in lines {
        block += line;
        block.push('\n');
    }
    use std::io::Write;
    let written = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(block.as_bytes()));
    match written {
        Ok(()) => println!("{} download(s) failed — details in {}", lines.len(), path.display()),
        Err(err) => eprintln!("dl: could not write {}: {err}", path.display()),
    }
}

/// The titles YouTube substitutes once an entry can't be played by anyone anymore.
const TOMBSTONES: &[&str] = &["[Private video]", "[Deleted video]", "[Unavailable video]"];

fn is_tombstone(title: &str) -> bool {
    TOMBSTONES.contains(&title)
}

/// The unplayable-entries report: everything a future search needs to trace what each entry
/// was — its id (the strongest key), its position, and lookup links for the WaybackMachine
/// and filmot (which indexes titles of deleted videos).
fn unplayable_report(playlist_title: &str, playlist_id: &str, dead: &[&ScanEntry]) -> String {
    let mut report = format!(
        "Unplayable entries of playlist: {playlist_title} [{playlist_id}]\n\
         recorded {} — video ids stay valid keys for archive services even after deletion\n\n",
        preferences::datehour_stamp()
    );
    for entry in dead {
        report += &format!(
            "#{index}  {title}\n  \
             was:      https://www.youtube.com/watch?v={id}\n  \
             wayback:  https://web.archive.org/web/*/https://www.youtube.com/watch?v={id}\n  \
             filmot:   https://filmot.com/video/{id}\n\n",
            index = entry.index,
            title = entry.title,
            id = entry.id,
        );
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_classify_by_what_they_point_at() {
        let video = |url: &str| assert_eq!(classify(url, false), Link::Video, "{url}");
        video("https://www.youtube.com/watch?v=MFT4OgFxfes");
        video("http://www.youtube.com/watch?v=MFT4OgFxfes");
        video("https://youtu.be/pv21e6iEZUw?si=sl5UGl0DI00f-0_h");
        video("https://youtu.be/IYnsfV5N2n8?si=6xSF90BnXIJcdy04&t=39"); // timestamped share
        video("https://vimeo.com/12345"); // not ours to refuse: yt-dlp knows other sites

        let playlist = classify(
            "https://www.youtube.com/watch?v=7jrKjkrX3Gw&list=PLvR1Vs9Qj4fk-VnGR2xUtNLvLcwBwGB6V",
            false,
        );
        assert_eq!(playlist, Link::Playlist { id: "PLvR1Vs9Qj4fk-VnGR2xUtNLvLcwBwGB6V".into() });
        assert_eq!(
            classify("https://www.youtube.com/playlist?list=PLxyz&feature=share", false),
            Link::Playlist { id: "PLxyz".into() }
        );
    }

    /// The EN-family keys, as tests pass them to the argv builders.
    fn en_keys() -> Vec<String> {
        ["en", "en-US", "en-GB"].map(str::to_string).to_vec()
    }

    #[test]
    fn the_single_flag_pins_a_video_carrying_a_playlist_to_just_the_video() {
        let url = "https://www.youtube.com/watch?v=7jrKjkrX3Gw&list=PLxyz";
        assert_eq!(classify(url, true), Link::Video);
        let argv = video_argv(url, Path::new("."), Env::default(), &en_keys());
        assert!(argv.contains(&OsString::from("--no-playlist")), "{argv:?}");
    }

    #[test]
    fn channel_urls_normalize_to_their_root_dropping_any_tab() {
        for url in [
            "https://www.youtube.com/@MontemayorChannel",
            "https://www.youtube.com/@MontemayorChannel/videos",
            "https://www.youtube.com/@MontemayorChannel/streams?view=0",
        ] {
            assert_eq!(
                classify(url, false),
                Link::Channel { root: "https://www.youtube.com/@MontemayorChannel".into() },
                "{url}"
            );
        }
        assert_eq!(
            classify("https://www.youtube.com/channel/UCabc123/playlists", false),
            Link::Channel { root: "https://www.youtube.com/channel/UCabc123".into() }
        );
        assert_eq!(
            classify("https://www.youtube.com/user/OldName", false),
            Link::Channel { root: "https://www.youtube.com/user/OldName".into() }
        );
    }

    /// `(key, name)` pairs the way the probe parser hands them over.
    fn named(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items.iter().map(|(key, name)| (key.to_string(), name.to_string())).collect()
    }

    fn keys_of(picks: &[Pick]) -> Vec<&str> {
        picks.iter().map(|pick| pick.key.as_str()).collect()
    }

    #[test]
    fn subtitle_policy_covers_uploader_native_and_english_without_doubles() {
        // The PlayStation case: uploader published en+ko — both taken, no auto anything.
        let real = sub_picks_for(
            &named(&[("en", "English"), ("ko", "Korean")]),
            &named(&[("ko", "Korean"), ("ko-orig", "Korean (Original)"), ("en", "English")]),
            Some("ko"),
        );
        assert_eq!(keys_of(&real), ["en", "ko"]);
        assert!(real.iter().all(|pick| !pick.auto), "uploader subs must never be stamped");

        // No uploader subs on a Korean video: native auto + translated EN, both flagged auto,
        // preferring the pair/orig keys when published.
        let auto = sub_picks_for(
            &[],
            &named(&[
                ("ko", "Korean"),
                ("ko-orig", "Korean (Original)"),
                ("en", "English"),
                ("en-ko", "English from Korean"),
            ]),
            Some("ko"),
        );
        assert_eq!(keys_of(&auto), ["ko-orig", "en-ko"]);
        assert!(auto.iter().all(|pick| pick.auto));
        assert_eq!(auto[0].name, "Korean (Original)", "the name is what the post-pass matches");

        // The Hebrew case, legacy `iw` code and all — no pair key published, plain `en` plan B.
        let hebrew = sub_picks_for(
            &[],
            &named(&[("iw", "Hebrew"), ("iw-orig", "Hebrew (Original)"), ("en", "English")]),
            Some("iw"),
        );
        assert_eq!(keys_of(&hebrew), ["iw-orig", "en"]);
        assert!(hebrew.iter().all(|pick| pick.auto), "plain `en` here is still a translation");

        // English-native video with no uploader subs: EN once, not twice.
        assert_eq!(
            keys_of(&sub_picks_for(
                &[],
                &named(&[("en", "English"), ("en-orig", "English (Original)")]),
                Some("en")
            )),
            ["en-orig"]
        );
        // Real Korean only: EN still arrives as the auto translation, flagged as such.
        let mixed = sub_picks_for(
            &named(&[("ko", "Korean")]),
            &named(&[("en", "English"), ("ko-orig", "Korean (Original)")]),
            Some("ko"),
        );
        assert_eq!(keys_of(&mixed), ["ko", "en"]);
        assert_eq!(mixed.iter().map(|pick| pick.auto).collect::<Vec<_>>(), [false, true]);
        // live_chat is not a subtitle; unknown native language degrades gracefully.
        assert_eq!(
            keys_of(&sub_picks_for(&named(&[("live_chat", ""), ("en", "English")]), &[], None)),
            ["en"]
        );
        assert!(sub_picks_for(&[], &[], None).is_empty(), "nothing available, nothing requested");
    }

    #[test]
    fn auto_tracks_get_stamped_titles() {
        assert_eq!(stamped_title("Hebrew (Original)"), "Hebrew (auto-generated)");
        assert_eq!(stamped_title("English"), "English (auto-generated)");
        assert_eq!(stamped_title("English from Korean"), "English from Korean (auto-generated)");
    }

    #[test]
    fn json_lang_names_pair_each_key_with_its_display_name() {
        let json = r#"{"en": [{"url": "u", "name": "English"}], "iw-orig": [{"ext": "vtt", "name": "Hebrew (Original)"}], "bare": []}"#;
        assert_eq!(
            json_lang_names(json),
            [
                ("en".to_string(), "English".to_string()),
                ("iw-orig".to_string(), "Hebrew (Original)".to_string()),
                ("bare".to_string(), String::new()),
            ]
        );
        assert!(json_lang_names("NA").is_empty());
    }

    #[test]
    fn every_yt_run_embeds_metadata_resumes_and_targets_mkv() {
        let argv = common(
            Path::new("/dl"),
            Env {
                ffmpeg_dir: Some(Path::new("/ff/bin")),
                cookies: Some(Path::new("/c.txt")),
                js_runtime: Some(Path::new("/dn/deno")),
                ..Default::default()
            },
            &en_keys(),
        );
        let text: Vec<String> = argv.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
        for expected in [
            "--write-subs", "--write-auto-subs", "--embed-subs",
            "--sub-langs", "en,en-US,en-GB",
            "--sleep-subtitles", "2",
            "--embed-metadata", "--embed-chapters",
            "--merge-output-format", "mkv",
            "--ignore-errors",
            "--download-archive", "/dl/.dl_video_archive.txt",
            "--ffmpeg-location", "/ff/bin",
            "--cookies", "/c.txt",
            "--js-runtimes", "deno:/dn/deno",
        ] {
            assert!(text.iter().any(|arg| arg == expected), "missing {expected}: {text:?}");
        }
        // With no bundled ffmpeg and no cookies, neither flag appears — nor the knobs. And a
        // video with no subtitles anywhere requests none.
        let bare = common(Path::new("/dl"), Env::default(), &[]);
        for absent in [
            "--ffmpeg-location", "--cookies", "--cookies-from-browser", "--js-runtimes", "-x", "-S",
            "--write-subs", "--sub-langs", "--sleep-subtitles",
        ] {
            assert!(!bare.iter().any(|arg| arg == absent), "{absent} leaked in");
        }
        // Thumbnails are a late opt-in pass now, never inline in the download argv.
        assert!(!text.iter().any(|a| a == "--embed-thumbnail"), "thumbnail must not be inline");
    }

    #[test]
    fn the_attached_pic_index_is_read_from_ffprobes_stream_listing() {
        // `index=N` lines each followed by that stream's dispositions (default=nw=1 layout).
        let with_cover = "index=0\nDISPOSITION:attached_pic=0\nindex=1\nDISPOSITION:attached_pic=0\nindex=2\nDISPOSITION:attached_pic=1";
        assert_eq!(attached_pic_stream_index(with_cover), Some(2), "the cover's absolute index");
        let without = "index=0\nDISPOSITION:attached_pic=0\nindex=1\nDISPOSITION:attached_pic=0";
        assert_eq!(attached_pic_stream_index(without), None, "video+audio, no cover");
        assert_eq!(attached_pic_stream_index(""), None, "no streams");
    }

    #[test]
    fn id_from_url_pulls_the_11_char_video_id_or_gives_up() {
        assert_eq!(id_from_url("https://www.youtube.com/watch?v=MFT4OgFxfes").as_deref(), Some("MFT4OgFxfes"));
        assert_eq!(id_from_url("https://youtu.be/pv21e6iEZUw?si=x").as_deref(), Some("pv21e6iEZUw"));
        assert_eq!(
            id_from_url("https://www.youtube.com/watch?v=7jrKjkrX3Gw&list=PLxyz").as_deref(),
            Some("7jrKjkrX3Gw")
        );
        assert_eq!(id_from_url("https://www.youtube.com/shorts/abcDEF12345").as_deref(), Some("abcDEF12345"));
        assert_eq!(id_from_url("https://www.youtube.com/live/abcDEF12345?feature=share").as_deref(), Some("abcDEF12345"));
        assert_eq!(id_from_url("https://example.com/video/123"), None); // no recognizable id
    }

    #[test]
    fn find_by_id_walks_nested_dirs_and_matches_media_extensions_only() {
        let dir = scratch_dir("findbyid");
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("a/b/20200101__Title__abcdefghijk.mkv"), b"x").unwrap();
        std::fs::write(dir.join("notes__abcdefghijk.txt"), b"x").unwrap(); // not a media ext
        std::fs::write(dir.join("song__qrstuvwxyz1.opus"), b"x").unwrap();
        let found = find_by_id(&dir, "abcdefghijk").expect("nested mkv found");
        assert!(found.ends_with("a/b/20200101__Title__abcdefghijk.mkv"), "{found:?}");
        assert!(find_by_id(&dir, "qrstuvwxyz1").is_some(), "audio extensions match too");
        assert!(find_by_id(&dir, "zzzzzzzzzzz").is_none(), "an unknown id finds nothing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn subtitle_titles_reads_ffprobes_titled_streams_for_the_idempotency_match() {
        let listing = "index=2\nTAG:title=English\nindex=3\nTAG:title=Hebrew (auto-generated)\nindex=4";
        assert_eq!(subtitle_titles(listing), ["English", "Hebrew (auto-generated)"]);
        assert!(subtitle_titles("index=2\nindex=3").is_empty(), "streams with no title tag → none");
        // The title a pick is matched/written by: plain name for a real track, stamped for auto.
        let real = Pick { key: "en".into(), name: "English".into(), auto: false };
        let auto = Pick { key: "iw-orig".into(), name: "Hebrew (Original)".into(), auto: true };
        assert_eq!(subtitle_title(&real), "English");
        assert_eq!(subtitle_title(&auto), "Hebrew (auto-generated)");
    }

    // --- python-backed checks (fixture sqlite DBs; skip-with-notice when python3 is absent) ----

    /// Skip-with-notice: the resolved python3 when it can do sqlite work, else `None` after
    /// printing a visible SKIP line — the test then passes vacuously instead of failing on a
    /// machine with no bundled or system python.
    fn python3_or_skip(test: &str) -> Option<std::ffi::OsString> {
        let python = crate::tools::resolve("python3");
        let works = std::process::Command::new(&python)
            .args(["-c", "import sqlite3"])
            .output()
            .is_ok_and(|out| out.status.success());
        if !works {
            eprintln!("SKIPPED {test}: no usable python3 (with sqlite3) available");
        }
        works.then_some(python)
    }

    /// Run a python snippet with `args`, returning its stdout (asserting it succeeded).
    fn py(python: &std::ffi::OsStr, code: &str, args: &[&std::ffi::OsStr]) -> String {
        let out = std::process::Command::new(python)
            .arg("-c")
            .arg(code)
            .args(args)
            .output()
            .expect("run python3");
        assert!(out.status.success(), "python failed: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// A fresh scratch directory under the system temp dir.
    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bashrs_yt_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_cookie_filter_keeps_only_target_domain_rows_and_carries_read_metadata() {
        // The feature's privacy contract: an import copies the target site's rows — dot-hosts,
        // the bare domain, true subdomains — and NOTHING else; lookalike hosts stay behind. The
        // version metadata each family needs to read the copy travels with it.
        let Some(python) = python3_or_skip("cookie filter") else { return };
        let dir = scratch_dir("cookiefilter");
        let domains = vec!["tiktok.com".to_string()];

        // Firefox family: `moz_cookies.host`, expiry units recorded in `PRAGMA user_version`.
        let src = dir.join("cookies.sqlite");
        py(
            &python,
            r#"import sqlite3, sys, time
con = sqlite3.connect(sys.argv[1])
con.execute("CREATE TABLE moz_cookies (id INTEGER PRIMARY KEY, host TEXT, name TEXT, value TEXT, expiry INTEGER)")
exp = int(time.time()) + 86400
rows = [(".tiktok.com", "keep_dot"), ("tiktok.com", "keep_bare"), ("sub.tiktok.com", "keep_sub"),
        ("evil.com", "other_site"), ("xtiktok.com", "suffix_lookalike"), ("tiktok.com.evil.net", "prefix_lookalike")]
con.executemany("INSERT INTO moz_cookies (host, name, value, expiry) VALUES (?, ?, 'v', %d)" % exp, rows)
con.execute("PRAGMA user_version = 10")
con.commit()"#,
            &[src.as_os_str()],
        );
        let store_dir = dir.join("ff_store");
        std::fs::create_dir_all(&store_dir).unwrap();
        let store = crate::support::browsers::CookieStore {
            label: "test firefox".into(),
            browser: "firefox",
            files: vec![(src, "cookies.sqlite")],
        };
        assert_eq!(filter_cookie_db(&store, &store_dir, &domains), Some(3), "dot + bare + subdomain, nothing else");
        let read = py(
            &python,
            r#"import sqlite3, sys
con = sqlite3.connect(sys.argv[1])
print(con.execute("PRAGMA user_version").fetchone()[0])
for (n,) in con.execute("SELECT name FROM moz_cookies ORDER BY name"):
    print(n)"#,
            &[store_dir.join("cookies.sqlite").as_os_str()],
        );
        assert_eq!(read, "10\nkeep_bare\nkeep_dot\nkeep_sub\n", "rows filtered, user_version carried");

        // Chromium family: `cookies.host_key`, encryption version in the `meta` table.
        let src = dir.join("Cookies");
        py(
            &python,
            r#"import sqlite3, sys
con = sqlite3.connect(sys.argv[1])
con.execute("CREATE TABLE cookies (creation_utc INTEGER, host_key TEXT, name TEXT, encrypted_value BLOB)")
con.execute("CREATE TABLE meta (key LONGVARCHAR NOT NULL UNIQUE PRIMARY KEY, value LONGVARCHAR)")
con.execute("INSERT INTO meta VALUES ('version', '24')")
rows = [(".tiktok.com", "keep_dot"), ("www.tiktok.com", "keep_www"), ("accounts.evil.com", "other_site")]
con.executemany("INSERT INTO cookies (creation_utc, host_key, name, encrypted_value) VALUES (0, ?, ?, x'76')", rows)
con.commit()"#,
            &[src.as_os_str()],
        );
        let store_dir = dir.join("cr_store");
        std::fs::create_dir_all(&store_dir).unwrap();
        let store = crate::support::browsers::CookieStore {
            label: "test chrome".into(),
            browser: "chrome",
            files: vec![(src, "Cookies")],
        };
        assert_eq!(filter_cookie_db(&store, &store_dir, &domains), Some(2));
        let read = py(
            &python,
            r#"import sqlite3, sys
con = sqlite3.connect(sys.argv[1])
print(con.execute("SELECT value FROM meta WHERE key='version'").fetchone()[0])
for (n,) in con.execute("SELECT name FROM cookies ORDER BY name"):
    print(n)"#,
            &[store_dir.join("Cookies").as_os_str()],
        );
        assert_eq!(read, "24\nkeep_dot\nkeep_www\n", "rows filtered, meta carried");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_expiry_probe_reads_both_epochs_and_never_cries_wolf_on_session_cookies() {
        let Some(python) = python3_or_skip("cookie expiry") else { return };
        let dir = scratch_dir("cookieexpiry");
        // One builder for both families: rows are `past`/`future`/`session`, converted to the
        // family's epoch (firefox: unix seconds; chromium: microseconds since 1601).
        let builder = r#"import sqlite3, sys, time
db, kind, rows = sys.argv[1], sys.argv[2], sys.argv[3].split(",")
now = int(time.time())
val = {"past": now - 86400, "future": now + 86400, "session": 0}
table, col = ("moz_cookies", "expiry") if kind == "firefox" else ("cookies", "expires_utc")
con = sqlite3.connect(db)
con.execute("CREATE TABLE %s (%s INTEGER)" % (table, col))
for r in rows:
    e = val[r]
    if kind != "firefox" and e:
        e = int((e + 11644473600) * 1000000)
    con.execute("INSERT INTO %s VALUES (?)" % table, (e,))
con.commit()"#;
        let case = |name: &str, kind: &str, rows: &str| {
            let db = dir.join(name);
            py(&python, builder, &[db.as_os_str(), kind.as_ref(), rows.as_ref()]);
            cookies_expired(&db, kind)
        };
        assert_eq!(case("ff_dead", "firefox", "past,past"), Some(true), "all past → expired");
        assert_eq!(case("ff_live", "firefox", "past,future"), Some(false), "one live → live");
        assert_eq!(case("ff_sess", "firefox", "session,session"), Some(false), "session-only → never a false alarm");
        assert_eq!(case("cr_dead", "chromium", "past"), Some(true), "webkit epoch converts");
        assert_eq!(case("cr_live", "chromium", "past,future"), Some(false));
        assert_eq!(case("cr_sess", "chromium", "session"), Some(false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_thumbnail_pass_leaves_non_mkv_files_untouched() {
        // Regression: the cover attaches via an mkv remux — renaming that over an audio file
        // would swap its container. The pass must skip non-mkv files without touching them
        // (guard fires before any ffprobe/fetch, so this runs fully offline).
        let dir = scratch_dir("thumbguard");
        let file = dir.join("song__abcdefghijk.opus");
        std::fs::write(&file, b"OPUSDATA").unwrap();
        embed_thumbnails(&dir, &["abcdefghijk".to_string()], Env::default());
        assert_eq!(std::fs::read(&file).unwrap(), b"OPUSDATA", "audio bytes must be untouched");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name() != "song__abcdefghijk.opus")
            .collect();
        assert!(leftovers.is_empty(), "no temp or mkv artifacts may appear: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- ffmpeg-backed round-trips (offline; skip-with-notice when no ffmpeg is available) -----

    /// Skip-with-notice: `true` when ffmpeg+ffprobe are runnable (bundled or PATH) — the same
    /// resolution the code under test uses via `Env::ffmpeg_dir`.
    fn ffmpeg_or_skip(test: &str) -> bool {
        let dir = bundled_ffmpeg_dir();
        let works = ["ffmpeg", "ffprobe"].iter().all(|name| {
            let bin = dir
                .as_ref()
                .map(|dir| dir.join(name).into_os_string())
                .unwrap_or_else(|| (*name).into());
            std::process::Command::new(bin)
                .arg("-version")
                .output()
                .is_ok_and(|out| out.status.success())
        });
        if !works {
            eprintln!("SKIPPED {test}: no usable ffmpeg/ffprobe available");
        }
        works
    }

    /// The resolved name of an ffmpeg-family binary, mirroring the code under test.
    fn ff_bin(name: &str) -> std::ffi::OsString {
        bundled_ffmpeg_dir()
            .map(|dir| dir.join(name).into_os_string())
            .unwrap_or_else(|| name.into())
    }

    /// A tiny real mkv (blue frame + a beep) at `path` — the fixture every round-trip starts from.
    fn build_test_mkv(path: &Path) {
        let ok = std::process::Command::new(ff_bin("ffmpeg"))
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=c=blue:s=64x64:d=1"])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=1"])
            .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac"])
            .arg(path)
            .status()
            .is_ok_and(|status| status.success());
        assert!(ok, "could not build the test mkv");
    }

    #[test]
    fn a_thumbnail_attaches_probes_and_extracts_back_out() {
        if !ffmpeg_or_skip("thumbnail round-trip") {
            return;
        }
        let dir = scratch_dir("thumbtrip");
        let ffmpeg_dir = bundled_ffmpeg_dir();
        let env = Env { ffmpeg_dir: ffmpeg_dir.as_deref(), ..Default::default() };
        let file = dir.join("v__abcdefghijk.mkv");
        build_test_mkv(&file);
        let cover = dir.join("cover.jpg");
        let ok = std::process::Command::new(ff_bin("ffmpeg"))
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=c=red:s=32x32:d=1", "-frames:v", "1"])
            .arg(&cover)
            .status()
            .is_ok_and(|status| status.success());
        assert!(ok, "could not build the cover");

        assert_eq!(has_embedded_thumbnail(&file, env), Some(false), "starts bare");
        assert!(attach_thumbnail(&file, &cover, env), "attach succeeds");
        assert_eq!(has_embedded_thumbnail(&file, env), Some(true), "probe sees the cover");
        let index = embedded_thumbnail_index(&file, env).flatten().expect("cover index");
        let out = extract_thumbnail(&file, index, env).expect("extracts back out");
        assert!(std::fs::metadata(&out).unwrap().len() > 0, "extracted cover has bytes");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_subtitle_mux_adds_the_track_and_carries_the_cover_through() {
        // Unit-pins the remux bug the live tests caught: an attached cover must survive
        // mux_subtitles (it can't ride `-map 0` — the pass extracts and re-attaches it).
        if !ffmpeg_or_skip("subtitle mux round-trip") {
            return;
        }
        let dir = scratch_dir("muxtrip");
        let ffmpeg_dir = bundled_ffmpeg_dir();
        let env = Env { ffmpeg_dir: ffmpeg_dir.as_deref(), ..Default::default() };
        let file = dir.join("v__abcdefghijk.mkv");
        build_test_mkv(&file);
        let cover = dir.join("cover.jpg");
        let _ = std::process::Command::new(ff_bin("ffmpeg"))
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=c=red:s=32x32:d=1", "-frames:v", "1"])
            .arg(&cover)
            .status();
        assert!(attach_thumbnail(&file, &cover, env));

        let sidecar = dir.join("v.en.vtt");
        std::fs::write(&sidecar, "WEBVTT\n\n00:00.000 --> 00:01.000\nhi\n").unwrap();
        let pick = Pick { key: "en".into(), name: "English".into(), auto: false };
        assert!(mux_subtitles(&file, &[(&pick, sidecar)], 0, env), "mux succeeds");

        let (count, titles) = subtitle_streams(&file, env).expect("probe works");
        assert_eq!((count, titles), (1, vec!["English".to_string()]), "the track landed, titled");
        assert_eq!(has_embedded_thumbnail(&file, env), Some(true), "the cover survived the remux");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn auto_subtitle_tracks_get_their_titles_stamped_in_place() {
        if !ffmpeg_or_skip("auto-title stamping") {
            return;
        }
        let dir = scratch_dir("stamptrip");
        let ffmpeg_dir = bundled_ffmpeg_dir();
        let env = Env { ffmpeg_dir: ffmpeg_dir.as_deref(), ..Default::default() };
        let file = dir.join("v__abcdefghijk.mkv");
        build_test_mkv(&file);
        // Embed a track carrying YouTube's own name for it, as a download would.
        let sidecar = dir.join("v.en.vtt");
        std::fs::write(&sidecar, "WEBVTT\n\n00:00.000 --> 00:01.000\nhi\n").unwrap();
        let auto = Pick { key: "en".into(), name: "English".into(), auto: true };
        // mux_subtitles writes the stamped title for auto picks; build the pre-stamp state
        // (plain "English") with a non-auto pick, then stamp via mark_auto_titles.
        let plain = Pick { key: "en".into(), name: "English".into(), auto: false };
        assert!(mux_subtitles(&file, &[(&plain, sidecar)], 0, env));

        mark_auto_titles(&dir, "abcdefghijk", std::slice::from_ref(&auto), env);
        let (_, titles) = subtitle_streams(&file, env).expect("probe works");
        assert_eq!(titles, ["English (auto-generated)"], "the machine origin is stamped");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ipv4_is_forced_by_default_on_probe_and_download_unless_allow_ipv6() {
        // Default: every network invocation forces IPv4 — a broken IPv6 route otherwise stalls
        // each request ~5s on the happy-eyeballs fallback.
        assert!(seeded(Env::default()).iter().any(|a| a == "--force-ipv4"), "probe forces v4");
        let dl = common(Path::new("/dl"), Env::default(), &[]);
        assert!(dl.iter().any(|a| a == "--force-ipv4"), "download forces v4");
        // `--allow-ipv6` opts back out, on both paths.
        let v6 = Env { allow_ipv6: true, ..Default::default() };
        assert!(!seeded(v6).iter().any(|a| a == "--force-ipv4"), "probe honors --allow-ipv6");
        let dl6 = common(Path::new("/dl"), v6, &[]);
        assert!(!dl6.iter().any(|a| a == "--force-ipv4"), "download honors --allow-ipv6");
    }

    #[test]
    fn the_generic_argv_reuses_the_shared_base_flat_no_subs_no_playlist() {
        let extra = vec!["--yes-playlist".to_string()];
        let argv = generic_argv(
            "https://vimeo.com/12345",
            Path::new("/dl"),
            Env { res: Some(720), extra: &extra, ..Default::default() },
        );
        let text: Vec<String> = argv.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        // Same shared knobs + log files as the YouTube path (metadata, thumbnail, mkv, archive).
        for expected in ["--embed-metadata", "--merge-output-format", "mkv", "--download-archive", "/dl/.dl_video_archive.txt"] {
            assert!(text.contains(&expected.to_string()), "missing {expected}: {text:?}");
        }
        // But no subtitle probing off YouTube, and a flat output template (no folder tree).
        for absent in ["--write-subs", "--write-auto-subs", "--sub-langs", "--embed-subs"] {
            assert!(!text.contains(&absent.to_string()), "{absent} leaked into the generic path");
        }
        let out = text.iter().position(|a| a == "--output").expect("has --output");
        assert_eq!(text[out + 1], "/dl/%(title)s [%(id)s].%(ext)s", "flat generic name under `into`");
        // Quality knob still applies; the URL lands last; `-- --yes-playlist` follows our
        // --no-playlist so a user can override the single-video default.
        assert!(text.contains(&"res:720".to_string()));
        assert_eq!(text.last().unwrap(), "https://vimeo.com/12345");
        let no = text.iter().position(|a| a == "--no-playlist").expect("defaults to single");
        let yes = text.iter().rposition(|a| a == "--yes-playlist").expect("extra passed through");
        assert!(yes > no, "user's --yes-playlist must come after our --no-playlist to win");
    }

    #[test]
    fn the_metadata_seed_carries_cookies_so_gated_content_is_readable_while_probing() {
        // Probes and scans authenticate too — an age-restricted video's subtitle probe or a
        // members-only tab's scan would otherwise run signed-out.
        let seed = seeded(Env { cookies_from_browser: Some("firefox:/store"), ..Default::default() });
        let text: Vec<String> = seed.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        let at = text.iter().position(|a| a == "--cookies-from-browser").expect("cookies in seed");
        assert_eq!(text[at + 1], "firefox:/store");
        // Nothing configured → a bare seed (bundles absent in the test env too).
        assert!(seeded(Env::default()).iter().all(|a| a != "--cookies" && a != "--cookies-from-browser"));
    }

    #[test]
    fn imported_browser_cookies_are_used_but_an_explicit_file_wins() {
        let pair = |argv: &[OsString], flag: &str| {
            argv.iter().position(|a| a == flag).map(|i| argv[i + 1].to_string_lossy().into_owned())
        };
        // A prior --cookie-import with no explicit file: the browser spec is passed.
        let imported = common(
            Path::new("/dl"),
            Env { cookies_from_browser: Some("firefox:/store"), ..Default::default() },
            &[],
        );
        assert_eq!(pair(&imported, "--cookies-from-browser").as_deref(), Some("firefox:/store"));
        assert!(!imported.iter().any(|a| a == "--cookies"));
        // An explicit --cookies file overrides the import — never both.
        let explicit = common(
            Path::new("/dl"),
            Env {
                cookies: Some(Path::new("/c.txt")),
                cookies_from_browser: Some("firefox:/store"),
                ..Default::default()
            },
            &[],
        );
        assert_eq!(pair(&explicit, "--cookies").as_deref(), Some("/c.txt"));
        assert!(!explicit.iter().any(|a| a == "--cookies-from-browser"), "explicit file must win alone");
    }

    #[test]
    fn cookie_dump_count_skips_comments_and_blanks_but_keeps_httponly() {
        // A Netscape dump as yt-dlp writes it: header + comment + blank skipped, `#HttpOnly_`
        // kept (the auth cookies are usually HttpOnly). The store is pre-filtered per site, so
        // the read-back only needs the decryptable total — not a per-domain tally.
        let dump = "# Netscape HTTP Cookie File\n\
                    # a comment\n\
                    \n\
                    #HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t2000000000\tLOGIN_INFO\ttok\n\
                    .google.com\tTRUE\t/\tTRUE\t2000000000\tSID\tsid\n";
        assert_eq!(count_cookie_dump(dump), 2, "two cookie rows, comments/blank excluded");
    }

    #[test]
    fn cookie_dump_count_of_an_empty_or_header_only_dump_is_zero() {
        assert_eq!(count_cookie_dump(""), 0);
        assert_eq!(count_cookie_dump("# Netscape HTTP Cookie File\n\n"), 0);
    }

    #[test]
    fn the_optional_knobs_shape_the_run_and_extras_land_last() {
        let extra = vec!["--merge-output-format".to_string(), "webm/mkv".to_string()];
        let env = Env { audio: true, res: Some(1080), extra: &extra, ..Default::default() };
        let argv = video_argv("https://u", Path::new("/dl"), env, &en_keys());
        assert!(argv.contains(&OsString::from("-x")), "audio-only: {argv:?}");
        assert!(
            !argv.contains(&OsString::from("--embed-subs")),
            "audio can't hold subs — requesting them would strand sidecars: {argv:?}"
        );
        let sort = argv.iter().position(|arg| arg == "-S").expect("-S");
        assert_eq!(argv[sort + 1], OsString::from("res:1080"));
        // Extras sit after every default (so a repeated flag resolves their way) and only
        // the URL follows them.
        let output = argv.iter().position(|arg| arg == "--output").unwrap();
        let webm = argv.iter().position(|arg| arg == "webm/mkv").unwrap();
        assert!(output < webm, "extras must come after the defaults they override");
        assert_eq!(argv.last().unwrap(), &OsString::from("https://u"));
        assert_eq!(argv[argv.len() - 2], OsString::from("webm/mkv"));
    }

    #[test]
    fn each_mode_shapes_its_own_output_tree_and_archive_home() {
        let last = |argv: &[OsString]| argv.last().unwrap().to_string_lossy().into_owned();
        let template = |argv: &[OsString]| {
            let at = argv.iter().position(|a| a == "--output").expect("--output");
            argv[at + 1].to_string_lossy().into_owned()
        };
        let archive = |argv: &[OsString]| {
            let at = argv.iter().position(|a| a == "--download-archive").expect("archive flag");
            argv[at + 1].to_string_lossy().into_owned()
        };
        let langs = en_keys();

        let video = video_argv("https://youtu.be/x", Path::new("/dl"), Env::default(), &langs);
        assert_eq!(template(&video), format!("/dl/{YT_NAME}"));
        assert_eq!(last(&video), "https://youtu.be/x", "the url comes last");

        // A playlist entry: files under the playlist folder, the archive inside it, and the
        // specific entry selected.
        let entry = playlist_argv(
            "https://l",
            Path::new("/dl"),
            Path::new("/dl/My List[PL1]"),
            Env::default(),
            &langs,
            Some("4"),
        );
        assert!(template(&entry).starts_with("/dl/%(playlist_title)s[%(playlist_id)s]/"));
        assert!(template(&entry).contains("%(playlist_index)03d__"));
        assert_eq!(archive(&entry), "/dl/My List[PL1]/.dl_video_archive.txt");
        let items = entry.iter().position(|a| a == "--playlist-items").expect("item selection");
        assert_eq!(entry[items + 1], OsString::from("4"));

        let tab = channel_tab_argv(
            "https://c/videos",
            "videos",
            Path::new("/dl"),
            Path::new("/dl/Chan[UC1]"),
            Env::default(),
            &langs,
            "7",
        );
        assert!(template(&tab).starts_with("/dl/%(uploader)s[%(channel_id)s]/videos/"));
        assert_eq!(archive(&tab), "/dl/Chan[UC1]/.dl_video_archive.txt");
        assert_eq!(last(&tab), "https://c/videos", "the tab url comes last");
    }

    #[test]
    fn the_flat_scan_parses_spots_tombstones_and_takes_the_folder_line() {
        let out = "1\tabc123def45\tA fine video\tMy Playlist\n\
                   2\tdead0000001\t[Private video]\tMy Playlist\n\
                   3\tdead0000002\t[Deleted video]\tMy Playlist\n\
                   My Playlist[PLxyz]\n";
        let scan = parse_scan(out).expect("parses");
        assert_eq!(scan.title, "My Playlist");
        assert_eq!(scan.entries.len(), 3);
        assert_eq!(scan.dirname.as_deref(), Some("My Playlist[PLxyz]"));
        let dead: Vec<&ScanEntry> =
            scan.entries.iter().filter(|e| is_tombstone(&e.title)).collect();
        assert_eq!(dead.len(), 2);
        // A folder line carrying NA means the fields weren't there — no folder is better
        // than an `NA[NA]` one.
        let na = parse_scan("1\tid234567890\tT\tP\nNA[NA]\n").expect("parses");
        assert_eq!(na.dirname, None);
        assert!(parse_scan("").is_none(), "an empty scan is a failed scan");
    }

    #[test]
    fn the_batch_probe_parses_per_entry_and_groups_by_subtitle_keys() {
        // Entry 1: real en+ko. Entry 2: nothing anywhere. Entry 3: auto-only Korean video.
        let out = "1\tidA00000001\tko\t{\"en\": [{\"name\": \"English\"}], \"ko\": [{\"name\": \"Korean\"}]}\t{}\n\
                   2\tidB00000002\tNA\t{}\t{}\n\
                   3\tidC00000003\tko\t{}\t{\"ko\": [{\"name\": \"Korean\"}], \"en\": [{\"name\": \"English\"}]}\n";
        let plan = parse_batch(out);
        assert_eq!(plan[0].id, "idA00000001");
        assert_eq!(keys_of(&plan[0].picks), ["en", "ko"]);
        assert!(plan[1].picks.is_empty());
        assert_eq!(keys_of(&plan[2].picks), ["ko", "en"]);
        assert!(plan[2].picks.iter().all(|pick| pick.auto));
        let groups = group_by_langs(&plan);
        assert_eq!(groups.len(), 3, "three distinct key lists here");
        // Same keys → one group, even when the auto flags differ per entry (real en for one,
        // translated en for another) — the post-pass reads each entry's own picks.
        let twin = [
            Planned { index: "1".into(), id: "a".into(), picks: plan[0].picks.clone() },
            Planned {
                index: "9".into(),
                id: "b".into(),
                picks: plan[0]
                    .picks
                    .iter()
                    .map(|pick| Pick { auto: !pick.auto, ..pick.clone() })
                    .collect(),
            },
        ];
        let merged = group_by_langs(&twin);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].iter().map(|plan| plan.index.as_str()).collect::<Vec<_>>(), ["1", "9"]);
    }

    #[test]
    fn archived_ids_read_the_second_column() {
        let dir = std::env::temp_dir().join(format!("bashrs_arch_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join(ARCHIVE_NAME);
        std::fs::write(&file, "youtube abc123\nyoutube def456\nmalformed\n").unwrap();
        let ids = archived_ids(&file);
        assert!(ids.contains("abc123") && ids.contains("def456"));
        assert_eq!(ids.len(), 2);
        assert!(archived_ids(Path::new("/no/such/archive")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_unplayable_report_traces_each_entry_by_id() {
        let dead = [ScanEntry {
            index: "2".into(),
            id: "dead0000001".into(),
            title: "[Private video]".into(),
        }];
        let refs: Vec<&ScanEntry> = dead.iter().collect();
        let report = unplayable_report("My Playlist", "PLxyz", &refs);
        assert!(report.contains("My Playlist [PLxyz]"));
        assert!(report.contains("#2  [Private video]"));
        assert!(report.contains("https://www.youtube.com/watch?v=dead0000001"));
        assert!(report
            .contains("https://web.archive.org/web/*/https://www.youtube.com/watch?v=dead0000001"));
        assert!(report.contains("https://filmot.com/video/dead0000001"));
    }

    #[test]
    fn a_missing_tab_is_recognized_by_ytdlps_phrasing() {
        assert!(tab_absence("ERROR: [youtube:tab] @x: This channel does not have a streams tab"));
        assert!(!tab_absence("ERROR: [youtube:tab] @x: Unable to download webpage"));
    }

    #[test]
    fn failures_classify_by_ytdlps_phrasing() {
        // Geo — both real YouTube-extractor phrasings, plus the generic raise_geo_restricted
        // default and the region/case variants the widened matcher must still catch.
        for msg in [
            "ERROR: [youtube] x: The uploader has not made this video available in your country",
            "ERROR: [youtube] x: This video is not available from your location due to geo restriction",
            "ERROR: This playlist is likely not available in your region",
            "ERROR: Video is GEO restricted",
        ] {
            assert_eq!(classify_failure(msg), Failure::Geo, "{msg}");
        }
        // Members — the badge reasons YouTube returns, hyphen and space spellings.
        for msg in [
            "ERROR: Join this channel to get access to members-only content like this video",
            "ERROR: This video is available to this channel's members on level: Tier 1",
        ] {
            assert_eq!(classify_failure(msg), Failure::Members, "{msg}");
        }
        // DRM — report_drm's phrasing gets its own class, so the cookie quirk advice can fire.
        assert_eq!(
            classify_failure("ERROR: [youtube] x: This video is DRM protected"),
            Failure::Drm
        );
        // Age — the sign-in gate and the account age-verification wording.
        assert_eq!(classify_failure("ERROR: Sign in to confirm your age"), Failure::AgeRestricted);
        assert_eq!(
            classify_failure("ERROR: This video is age-restricted and YouTube is requiring account age-verification"),
            Failure::AgeRestricted
        );
        // Bot-wall — the anti-automation walls yt-dlp can't solve: YouTube's bot check, TikTok's
        // JS challenge, Google's unusual-traffic notice.
        for msg in [
            "ERROR: [youtube] x: Sign in to confirm you're not a bot. Use --cookies-from-browser",
            "ERROR: [TikTok] x: Unable to solve JS challenge",
            "ERROR: Our systems have detected unusual traffic from your computer network",
        ] {
            assert_eq!(classify_failure(msg), Failure::BotWall, "{msg}");
        }
        // Sensitive / not-for-all-audiences — flagged group-offensive; "for some audiences" is the
        // tell, and it's checked before the login gate even though it also says "Log in for access"
        // (distinct from YouTube's age gate, which says "for some users").
        assert_eq!(
            classify_failure("ERROR: [TikTok] x: This post may not be comfortable for some audiences. Log in for access"),
            Failure::Sensitive
        );
        // Login / private gates — solvable with cookies from an account with access.
        for msg in [
            "ERROR: [TikTok] x: TikTok is requiring login for access to this content",
            "ERROR: [youtube] x: Private video. Sign in if you've been granted access to this video",
        ] {
            assert_eq!(classify_failure(msg), Failure::LoginRequired, "{msg}");
        }
        // A bare 403 stays Other on purpose — ambiguous (rate-limit vs. bot vs. transient), so we
        // don't mislabel it as a bot-wall.
        assert_eq!(classify_failure("ERROR: HTTP Error 403: Forbidden"), Failure::Other);
        assert_eq!(XFF_REGIONS.len(), 12, "a dozen regions, as specified");
    }

    #[test]
    fn geo_ledger_lines_name_the_video_and_carry_the_scrub_key() {
        // Two geo outcomes reach the ledger. The generic path may sweep XFF regions, so its line
        // records every region tried; the YouTube path is IP-enforced (spoofing can't help), so
        // its line names the block and the real fix, no regions. Both must name the video, carry
        // the scrub key in a delimited form, and stay on one line (so scrub_ledger can line-filter).
        let swept = format!(
            "https://vid.example/watch?v=dQw4w9WgXcQ — geo-blocked (tried {})",
            XFF_REGIONS.join(",")
        );
        assert!(ledger_line_refers(&swept, "dQw4w9WgXcQ"), "sweep line carries the scrub key");
        assert!(swept.contains("geo-blocked") && !swept.contains('\n'), "one geo-blocked line");
        for region in XFF_REGIONS {
            assert!(swept.contains(region), "sweep line records region {region}");
        }

        let ip_enforced = "#7 Some Title [dQw4w9WgXcQ] — geo-blocked (enforced by IP; retry from a VPN or --proxy with an IP in an allowed region)".to_string();
        assert!(ip_enforced.contains("#7 Some Title"), "names the video");
        assert!(ledger_line_refers(&ip_enforced, "dQw4w9WgXcQ"), "carries the scrub key");
        assert!(ip_enforced.contains("geo-blocked") && !ip_enforced.contains('\n'), "one geo-blocked line");
        assert!(!ip_enforced.contains("tried"), "no region sweep on the IP-enforced path");
    }

    #[test]
    fn the_age_restricted_line_adapts_to_whether_cookies_were_tried() {
        let without = age_restricted_line("[dQw4w9WgXcQ]", false);
        assert!(without.contains("age-restricted"), "names the gate");
        assert!(without.contains("--cookie-import"), "nudges toward importing cookies");
        assert!(!without.contains("despite"), "the no-cookies case just asks for cookies");

        let with = age_restricted_line("[dQw4w9WgXcQ]", true);
        assert!(with.contains("despite cookies"), "the cookies-present case names the harder gate");
        assert!(with.contains("18+") && with.contains("verify age"), "points at the real fix");
        assert!(with.contains("PO-token"), "names the lever past cookies (necessary ≠ sufficient)");
        assert!(!without.contains("PO-token"), "the plain case keeps the simple fix simple");
        assert!(ledger_line_refers(&with, "dQw4w9WgXcQ"), "still carries the scrub key");
    }

    #[test]
    fn the_drm_line_reaches_for_cookies_first_and_is_terminal_with_them() {
        // The tv-client quirk: cookie-less requests get DRM'd formats, ANY cookies (even a
        // logged-out session) surface non-DRM ones — so cookies are a genuine first fix.
        let without = drm_line("[dQw4w9WgXcQ]", false);
        assert!(without.contains("DRM-protected"), "names the gate");
        assert!(without.contains("--cookie-import"), "cookies are the one real lever");
        let with = drm_line("[dQw4w9WgXcQ]", true);
        assert!(with.contains("even with cookies"), "won't repeat advice already taken");
        assert!(with.contains("doesn't circumvent"), "honest that this is terminal");
        assert!(!with.to_lowercase().contains("retry"), "no futile retry once cookies were tried");
        assert!(ledger_line_refers(&with, "dQw4w9WgXcQ"), "carries the scrub key");
    }

    #[test]
    fn the_login_required_line_points_at_cookies_and_adapts_to_whether_they_were_tried() {
        let without = login_required_line("[dQw4w9WgXcQ]", false);
        assert!(without.contains("--cookie-import"), "nudges toward importing cookies");
        assert!(without.contains("retry"), "with cookies as the real fix, retrying is right advice");

        let with = login_required_line("[dQw4w9WgXcQ]", true);
        assert!(with.contains("lack access") || with.contains("stale"), "names why the cookies didn't help");
        assert!(!with.contains("retry"), "no plain-retry advice once cookies already failed");
        assert!(ledger_line_refers(&with, "dQw4w9WgXcQ"), "carries the scrub key");
    }

    #[test]
    fn the_sensitive_content_line_points_at_an_allowed_account_and_adapts_to_cookies() {
        let without = sensitive_content_line("[dQw4w9WgXcQ]", false);
        assert!(without.contains("sensitive"), "names why it's gated");
        assert!(without.contains("--cookie-import") && without.contains("retry"), "cookies are the fix");

        let with = sensitive_content_line("[dQw4w9WgXcQ]", true);
        assert!(with.contains("sensitive/mature") || with.contains("allowed to view"), "names the account condition");
        assert!(!with.contains("retry"), "no plain-retry once cookies already failed");
        assert!(ledger_line_refers(&with, "dQw4w9WgXcQ"), "carries the scrub key");
    }

    #[test]
    fn the_bot_wall_line_is_honest_that_it_may_be_unsolvable_and_never_says_retry() {
        let line = bot_wall_line("[dQw4w9WgXcQ]");
        assert!(line.contains("anti-bot") || line.contains("CAPTCHA"), "names the difficulty");
        assert!(line.contains("undownloadable"), "is honest it may not be possible");
        assert!(!line.to_lowercase().contains("retry"), "must not tell the user to just download again");
        assert!(line.contains("PO-token"), "names every real lever, including the unintegrated one");
        assert!(ledger_line_refers(&line, "dQw4w9WgXcQ"), "carries the scrub key");
    }

    #[test]
    fn ledger_lines_match_ids_only_in_delimited_forms() {
        assert!(ledger_line_refers("#3 Title [abc123XYZ_-] — members-only", "abc123XYZ_-"));
        assert!(ledger_line_refers("https://www.youtube.com/watch?v=abc123XYZ_- — failed: gone", "abc123XYZ_-"));
        assert!(ledger_line_refers("abc123XYZ_- — geo-blocked (tried US)", "abc123XYZ_-"), "legacy bare-id label");
        // A short id must not clear someone else's entry by matching inside its title.
        assert!(!ledger_line_refers("#4 my abc mixtape [zzzzzzzzzzz] — members-only", "abc"));
    }

    #[test]
    fn the_scrub_clears_now_downloaded_entries_prunes_empty_blocks_and_removes_an_emptied_ledger() {
        let dir = std::env::temp_dir().join(format!("bashrs_scrub_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ledger = dir.join(FAILED_LEDGER);
        std::fs::write(
            &ledger,
            "── 2026-01-01_1200 ──\n\
             #7 Released Later [vidAAAAAAAA] — members-only (channels often release these publicly later)\n\
             ── 2026-01-02_1200 ──\n\
             #2 Still Blocked [vidBBBBBBBB] — geo-blocked (tried US,GB)\n\
             #9 Also Freed [vidCCCCCCCC] — members-only (channels often release these publicly later)\n",
        )
        .unwrap();

        // Run 1: A and C have since downloaded (they're in the archive); B still hasn't.
        std::fs::write(dir.join(ARCHIVE_NAME), "youtube vidAAAAAAAA\nyoutube vidCCCCCCCC\n").unwrap();
        scrub_ledger(&dir);
        let text = std::fs::read_to_string(&ledger).unwrap();
        assert!(!text.contains("vidAAAAAAAA") && !text.contains("vidCCCCCCCC"), "cleared: {text}");
        assert!(text.contains("vidBBBBBBBB"), "unresolved entry stays: {text}");
        assert_eq!(
            text.matches("── ").count(),
            1,
            "the block whose only entry cleared lost its header too: {text}"
        );

        // Run 2: B downloads as well — nothing left, so the ledger file itself goes.
        std::fs::write(dir.join(ARCHIVE_NAME), "youtube vidAAAAAAAA\nyoutube vidBBBBBBBB\nyoutube vidCCCCCCCC\n").unwrap();
        scrub_ledger(&dir);
        assert!(!ledger.exists(), "an emptied ledger is removed");

        // And scrubbing with no ledger (or no archive) is a quiet no-op.
        scrub_ledger(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sidecars_pair_by_lang_key_and_skip_refused_tracks() {
        let dir = std::env::temp_dir().join(format!("bashrs_sidecar_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("20260101__title__abcdefghijk.opus");
        std::fs::write(&file, "").unwrap();
        std::fs::write(dir.join("20260101__title__abcdefghijk.iw-orig.vtt"), "WEBVTT").unwrap();
        let picks = [
            Pick { key: "iw-orig".into(), name: String::new(), auto: true },
            Pick { key: "en".into(), name: String::new(), auto: true }, // 429'd: never arrived
        ];
        let pairs = subtitle_sidecars(&file, &picks);
        assert_eq!(pairs.len(), 1, "missing sidecars are skipped, not errors");
        assert_eq!(pairs[0].0, "subtitles_iw_autogenerated");
        assert!(pairs[0].1.ends_with("20260101__title__abcdefghijk.iw-orig.vtt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn subtitle_tags_round_trip_into_each_audio_container() {
        // The lofty-backed tagger, end to end per family: Vorbis comments (opus), MP4 atoms
        // (m4a), ID3 frames (mp3). Read back through ffprobe — a second, independent tool — so
        // the test isn't lofty checking lofty. The sidecar must be folded in and removed.
        if !ffmpeg_or_skip("audio tag round-trip") {
            return;
        }
        let dir = scratch_dir("audiotags");
        for (ext, codec) in [("opus", "libopus"), ("m4a", "aac"), ("mp3", "libmp3lame")] {
            let file = dir.join(format!("song__abcdefghijk.{ext}"));
            let built = std::process::Command::new(ff_bin("ffmpeg"))
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=1"])
                .args(["-c:a", codec])
                .arg(&file)
                .status()
                .is_ok_and(|status| status.success());
            assert!(built, "could not build the {ext} sample");
            let sidecar = dir.join("song__abcdefghijk.en.vtt");
            std::fs::write(&sidecar, "WEBVTT\n\n00:00.000 --> 00:01.000\nthe words\n").unwrap();
            let picks = vec![Pick { key: "en".into(), name: "English".into(), auto: false }];

            embed_subtitle_tags(&dir, "abcdefghijk", &picks);

            assert!(!sidecar.exists(), "{ext}: the sidecar is folded in and removed");
            // ffprobe models ogg-family tags on the stream and mp3/m4a tags on the format —
            // ask for both levels so every container's convention is covered.
            let tags = std::process::Command::new(ff_bin("ffprobe"))
                .args(["-v", "error", "-show_entries", "format_tags:stream_tags", "-of", "default=nw=1"])
                .arg(&file)
                .output()
                .expect("run ffprobe");
            let tags = String::from_utf8_lossy(&tags.stdout).to_lowercase();
            assert!(
                tags.contains("subtitles_en") && tags.contains("webvtt"),
                "{ext}: the tag and its text must read back: {tags}"
            );
            std::fs::remove_file(&file).unwrap(); // one file per id at a time (find_by_id)
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn subtitle_tags_name_the_language_and_the_machine_origin() {
        let pick = |key: &str, auto: bool| Pick { key: key.into(), name: String::new(), auto };
        assert_eq!(subtitle_tag_name(&pick("en", false)), "subtitles_en");
        assert_eq!(subtitle_tag_name(&pick("en-US", false)), "subtitles_en_us");
        assert_eq!(subtitle_tag_name(&pick("iw-orig", true)), "subtitles_iw_autogenerated");
        assert_eq!(subtitle_tag_name(&pick("en-he", true)), "subtitles_en_he_autogenerated");
    }

    #[test]
    fn the_notable_flag_menu_is_well_formed() {
        assert!(!NOTABLE_FLAGS.is_empty());
        for (flag, blurb) in NOTABLE_FLAGS {
            assert!(flag.starts_with("--"), "{flag}");
            assert!(!blurb.is_empty(), "{flag} needs a blurb");
        }
    }
}
