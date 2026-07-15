//! Browser cookie-store discovery and per-site import layout, backing `dl --cookie-import`.
//! Rather than hand yt-dlp a live browser profile (whose cookie DB a running browser keeps
//! locked), the import writes a pared-down copy into bashrs's own data dir and points yt-dlp
//! there — a copy has no lock, and paring it to just the target site's cookies keeps every
//! other site's (banking, email, …) off bashrs's disk. This module owns the pure parts: finding
//! browser stores, the target-site registry + host resolution ([`resolve_target`]), and the
//! per-site store layout ([`reset_site`] / [`write_spec`] / [`imported_spec`] / [`forget`]). The
//! actual domain-filtered copy runs the bundled python and so lives a layer up, in the driver
//! ([`crate::drivers::youtube::filter_cookie_db`]) — `support` sits below `tools`.
//!
//! Where a browser keeps its cookies is a product of three things: the browser family (Firefox
//! stores a plaintext `cookies.sqlite`; Chromium keeps an encrypted `Cookies` DB — under
//! `<profile>/Network/` on current versions, `<profile>/` on old ones), the *profile* (people
//! sign into YouTube under `Default`, `Profile 1`, a named Firefox profile, …), and the package
//! manager that installed it (native, or a Flatpak/Snap sandbox that redirects the app's home).
//! The install-location piece comes from [`crate::support::package_management`], so a manager
//! added there widens this scan for free. Every profile with a cookie DB becomes its own import
//! candidate, so a login in a non-default profile is still reachable.
//!
//! A family searches two kinds of root, each contributed by the package-manager registry and so
//! already native/Flatpak/Snap-aware: *home* roots (`$HOME`, the sandbox homes) carry dotted
//! dirs like `.mozilla/firefox`; *XDG-config* roots (`~/.config`, `$XDG_CONFIG_HOME`, the
//! sandbox config dirs) carry dirs like `google-chrome`. Chromium browsers live only under the
//! config roots. Firefox lives under BOTH: the historical `~/.mozilla/firefox` (home) and, since
//! FF147 adopted the XDG base-directory spec, `~/.config/mozilla/firefox` (config) — a family
//! that declares both subpaths is scanned in both places, and the source-path dedup collapses
//! any overlap. yt-dlp only reads a fixed set of browser names, but because the import *copies*
//! the DB and yt-dlp then reads it by format, any Firefox-format browser can ride in under the
//! `firefox` name (its `cookies.sqlite` is plaintext — no keyring in play), which is how the
//! Firefox forks below are supported.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::support::package_management as pm;

/// A copyable cookie store found on disk, ready for the `--cookie-import` menu.
#[derive(Debug, PartialEq)]
pub struct CookieStore {
    /// Menu line, e.g. `Firefox (native) [default-release]`.
    pub label: String,
    /// yt-dlp's browser name, recorded so `dl` can name the copy back to it.
    pub browser: &'static str,
    /// Files to copy, as `(source, destination filename)`. Flattened into the store dir so
    /// yt-dlp's recursive search finds the DB regardless of its original nesting, and the
    /// `--cookies-from-browser <browser>:<store>` spec is uniform across families.
    pub files: Vec<(PathBuf, &'static str)>,
}

/// The two cookie-store shapes.
#[derive(Clone, Copy)]
enum Kind {
    /// Plaintext `cookies.sqlite`, one per profile directory.
    Firefox,
    /// `Cookies` DB (`<profile>/Network/Cookies` on current Chromium, `<profile>/Cookies` on
    /// old); on Linux it's decrypted via the desktop keyring, not a copied key file.
    Chromium,
}

/// A browser family: its store shape, the yt-dlp browser name its copied DB rides in under (a
/// value from yt-dlp's `SUPPORTED_BROWSERS` — for Firefox forks that's just `firefox`, since the
/// plaintext `cookies.sqlite` reads the same; a Chromium fork must name a real Chromium browser
/// whose keyring matches, e.g. ungoogled-chromium rides in as `chromium`), the binaries that
/// betray a native install, its Flatpak app id and Snap name (empty when it has none), and where
/// its profiles sit relative to an app root: `home_subpath` off the home roots (dotted, e.g.
/// `.mozilla/firefox`), `config_subpath` off the XDG-config roots (e.g. `google-chrome`). Either
/// may be empty. `search_roots` is for Firefox-based browsers whose profile nests unpredictably
/// deep (Tor/Mullvad): $HOME-relative top dirs walked recursively for a `cookies.sqlite`.
struct Family {
    kind: Kind,
    label: &'static str,
    ytdlp: &'static str,
    binaries: &'static [&'static str],
    flatpak_id: &'static str,
    snap_name: &'static str,
    home_subpath: &'static str,
    config_subpath: &'static str,
    search_roots: &'static [&'static str],
}

// `..NO_SANDBOX_OR_HOME` fills the fields a plain config-root Chromium browser leaves empty, so
// each row states only what distinguishes it.
const NO_SANDBOX_OR_HOME: Family = Family {
    kind: Kind::Chromium, label: "", ytdlp: "", binaries: &[],
    flatpak_id: "", snap_name: "", home_subpath: "", config_subpath: "", search_roots: &[],
};

const FAMILIES: &[Family] = &[
    // --- Firefox and its forks (plaintext cookies.sqlite; all ride in as `firefox`). Profiles
    // live under `.mozilla/firefox` off the home roots AND, since FF147's XDG move, under
    // `mozilla/firefox` off the config roots; the forks keep their own single dotted home dir.
    Family { kind: Kind::Firefox, label: "Firefox", ytdlp: "firefox", binaries: &["firefox", "firefox-esr"], flatpak_id: "org.mozilla.firefox", snap_name: "firefox", home_subpath: ".mozilla/firefox", config_subpath: "mozilla/firefox", search_roots: &[] },
    Family { kind: Kind::Firefox, label: "LibreWolf", ytdlp: "firefox", binaries: &["librewolf"], flatpak_id: "io.gitlab.librewolf-community", snap_name: "", home_subpath: ".librewolf", config_subpath: "", search_roots: &[] },
    Family { kind: Kind::Firefox, label: "Zen", ytdlp: "firefox", binaries: &["zen", "zen-browser"], flatpak_id: "app.zen_browser.zen", snap_name: "", home_subpath: ".zen", config_subpath: "", search_roots: &[] },
    Family { kind: Kind::Firefox, label: "Waterfox", ytdlp: "firefox", binaries: &["waterfox"], flatpak_id: "", snap_name: "", home_subpath: ".waterfox", config_subpath: "", search_roots: &[] },
    Family { kind: Kind::Firefox, label: "FireDragon", ytdlp: "firefox", binaries: &["firedragon"], flatpak_id: "", snap_name: "", home_subpath: ".firedragon", config_subpath: "", search_roots: &[] },
    // Tor & Mullvad are Firefox-ESR-based but keep the profile buried under
    // `…/TorBrowser/Data/Browser/<profile>/`, below an arch-named dir a fixed subpath can't
    // spell — so they're found by a bounded recursive walk from a small, well-known top dir
    // (torbrowser-launcher's data dir; the browsers' Flatpak app homes). Cookies here are
    // unlikely (few sign into YouTube over Tor) but harmless to offer. Portable tarball
    // installs, extracted to an arbitrary path, can't be auto-found.
    Family { kind: Kind::Firefox, label: "Tor Browser", ytdlp: "firefox", binaries: &["tor-browser", "torbrowser-launcher"], flatpak_id: "", snap_name: "", home_subpath: "", config_subpath: "", search_roots: &[".local/share/torbrowser", ".var/app/org.torproject.torbrowser-launcher"] },
    Family { kind: Kind::Firefox, label: "Mullvad Browser", ytdlp: "firefox", binaries: &["mullvad-browser"], flatpak_id: "", snap_name: "", home_subpath: "", config_subpath: "", search_roots: &[".local/share/mullvad-browser", ".var/app/net.mullvad.MullvadBrowser"] },
    // --- Chromium family (encrypted Cookies DB; each rides under its own yt-dlp name so the
    // right keyring is used). Profiles live under `config_subpath` off the config roots.
    Family { label: "Chrome", ytdlp: "chrome", binaries: &["google-chrome", "google-chrome-stable"], flatpak_id: "com.google.Chrome", config_subpath: "google-chrome", ..NO_SANDBOX_OR_HOME },
    Family { label: "Chromium", ytdlp: "chromium", binaries: &["chromium", "chromium-browser"], flatpak_id: "org.chromium.Chromium", snap_name: "chromium", config_subpath: "chromium", ..NO_SANDBOX_OR_HOME },
    // ungoogled-chromium: native shares ~/.config/chromium with Chromium (deduped by source
    // path); the Flathub build has its own app id, so its Flatpak profile is found and labeled.
    // Rides in as `chromium` — it keeps Chromium's "Chromium Safe Storage" keyring.
    Family { label: "Ungoogled Chromium", ytdlp: "chromium", binaries: &["ungoogled-chromium"], flatpak_id: "io.github.ungoogled_software.ungoogled_chromium", config_subpath: "chromium", ..NO_SANDBOX_OR_HOME },
    Family { label: "Brave", ytdlp: "brave", binaries: &["brave", "brave-browser"], flatpak_id: "com.brave.Browser", snap_name: "brave", config_subpath: "BraveSoftware/Brave-Browser", ..NO_SANDBOX_OR_HOME },
    Family { label: "Edge", ytdlp: "edge", binaries: &["microsoft-edge", "microsoft-edge-stable"], flatpak_id: "com.microsoft.Edge", config_subpath: "microsoft-edge", ..NO_SANDBOX_OR_HOME },
    Family { label: "Opera", ytdlp: "opera", binaries: &["opera"], flatpak_id: "com.opera.Opera", snap_name: "opera", config_subpath: "opera", ..NO_SANDBOX_OR_HOME },
    Family { label: "Vivaldi", ytdlp: "vivaldi", binaries: &["vivaldi", "vivaldi-stable"], flatpak_id: "com.vivaldi.Vivaldi", config_subpath: "vivaldi", ..NO_SANDBOX_OR_HOME },
    Family { label: "Whale", ytdlp: "whale", binaries: &["naver-whale-stable", "naver-whale"], config_subpath: "naver-whale", ..NO_SANDBOX_OR_HOME },
];

/// Depth cap for the `search_roots` recursive walk — Tor/Mullvad bury the profile ~8 levels
/// below their top dir (`tbb/<arch>/…/TorBrowser/Data/Browser/<profile>/`); a little headroom
/// covers layout drift without letting a mispointed root wander a large tree.
const SEARCH_MAX_DEPTH: usize = 10;

/// Where `dl` keeps the imported copy, under the given cookies root.
const STORE_SUBDIR: &str = "store";
/// The one-line marker recording which yt-dlp browser the copy came from.
const SPEC_FILE: &str = "browser.spec";

/// Every importable cookie store on the system: each browser family, at every install location
/// ([`pm::app_home_roots`] / [`pm::app_config_roots`]), across every profile that actually holds
/// a cookie DB. Sorted for a stable menu; duplicates (the same DB reached two ways) collapse.
pub fn cookie_stores(home: &Path) -> Vec<CookieStore> {
    let mut found: Vec<CookieStore> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for family in FAMILIES {
        for store in family_stores(family, home) {
            // Dedup by the source DB (the first file), canonicalized so two candidate paths
            // reaching one real file count once.
            let key = std::fs::canonicalize(&store.files[0].0).unwrap_or_else(|_| store.files[0].0.clone());
            if seen.insert(key) {
                found.push(store);
            }
        }
    }
    found.sort_by(|a, b| a.label.cmp(&b.label));
    found
}

fn family_stores(family: &Family, home: &Path) -> Vec<CookieStore> {
    // Candidate profile dirs come from two mechanisms: the subpath scan (immediate children of
    // each known profiles-parent) and, for deeply-nested browsers, a recursive walk of the
    // search roots. Both feed the same per-profile store builder.
    let mut profiles: Vec<PathBuf> =
        profile_parents(family, home).iter().flat_map(|base| profile_dirs(base)).collect();
    for root in family.search_roots {
        profiles.extend(profile_dirs_recursive(&home.join(root)));
    }
    profiles
        .into_iter()
        .filter_map(|profile| {
            cookie_files(family.kind, &profile).map(|files| CookieStore {
                label: format!("{} ({}) [{}]", family.label, flavor(&profile), profile_label(family.kind, &profile)),
                browser: family.ytdlp,
                files,
            })
        })
        .collect()
}

/// Every profiles-parent directory to scan for `family`: `home_subpath` off each home root, and
/// `config_subpath` off each XDG-config root. A browser with only one of the two (every Chromium
/// browser; a fork with no XDG variant) contributes from just that side. The roots themselves are
/// native/Flatpak/Snap-aware (see [`pm::app_home_roots`] / [`pm::app_config_roots`]).
fn profile_parents(family: &Family, home: &Path) -> Vec<PathBuf> {
    let mut bases = Vec::new();
    if !family.home_subpath.is_empty() {
        for root in pm::app_home_roots(home, family.flatpak_id, family.snap_name) {
            bases.push(root.join(family.home_subpath));
        }
    }
    if !family.config_subpath.is_empty() {
        for root in pm::app_config_roots(home, family.flatpak_id, family.snap_name) {
            bases.push(root.join(family.config_subpath));
        }
    }
    bases
}

/// Directories below `root` (bounded to [`SEARCH_MAX_DEPTH`]) that directly hold a Firefox
/// `cookies.sqlite` — for browsers whose profile nests unpredictably deep under a small, known
/// top dir (Tor/Mullvad). Empty when `root` is absent, so listing a root for an uninstalled
/// browser costs nothing.
fn profile_dirs_recursive(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth == 0 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for path in entries.flatten().map(|entry| entry.path()).filter(|path| path.is_dir()) {
            if path.join("cookies.sqlite").is_file() {
                out.push(path.clone());
            }
            walk(&path, depth - 1, out);
        }
    }
    let mut out = Vec::new();
    walk(root, SEARCH_MAX_DEPTH, &mut out);
    out
}

/// The files making up one profile's store, or `None` when it has no cookie DB. Firefox: the
/// plaintext DB. Chromium: the (possibly `Network/`-nested) DB, flattened to `Cookies`.
fn cookie_files(kind: Kind, profile: &Path) -> Option<Vec<(PathBuf, &'static str)>> {
    match kind {
        Kind::Firefox => {
            let db = profile.join("cookies.sqlite");
            db.is_file().then(|| vec![(db, "cookies.sqlite")])
        }
        Kind::Chromium => {
            // Current Chromium nests the DB under `Network/`; older versions keep it flat.
            let db = [profile.join("Network/Cookies"), profile.join("Cookies")]
                .into_iter()
                .find(|path| path.is_file())?;
            Some(vec![(db, "Cookies")])
        }
    }
}

/// The immediate subdirectories of `dir` (each a candidate browser profile); empty if `dir`
/// doesn't exist or can't be read.
fn profile_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries.flatten().map(|entry| entry.path()).filter(|path| path.is_dir()).collect()
}

/// The human profile name for a menu label: Chromium dirs read as-is (`Default`, `Profile 1`);
/// Firefox dirs shed their random salt (`8f7d2.default-release` → `default-release`).
fn profile_label(kind: Kind, profile: &Path) -> String {
    let name = profile.file_name().unwrap_or_default().to_string_lossy();
    match kind {
        Kind::Firefox => name.split_once('.').map(|(_, rest)| rest).unwrap_or(&name).to_string(),
        Kind::Chromium => name.into_owned(),
    }
}

/// The install flavor a matched root implies, for the menu label.
fn flavor(root: &Path) -> &'static str {
    let text = root.to_string_lossy();
    if text.contains("/.var/app/") {
        "flatpak"
    } else if text.contains("/snap/") {
        "snap"
    } else {
        "native"
    }
}

/// Whether any known browser looks installed (native binary, or a sandbox profile dir present),
/// even without a cookie store yet — lets the caller tell "no browsers" from "browsers, but you
/// haven't signed in anywhere yet".
pub fn any_browser_installed(home: &Path) -> bool {
    FAMILIES.iter().any(|family| {
        family.binaries.iter().any(|bin| pm::native_binary_present(home, bin))
            || pm::app_home_roots(home, family.flatpak_id, family.snap_name)
                .iter()
                .skip(1) // skip the native home (covered by the binary check)
                .any(|root| root.is_dir())
    })
}

// ============================================================
// Target sites — which cookie domains a download needs
// ============================================================
// `--cookie-import <target>` and every `dl <url>` run resolve their argument to a target site:
// the set of cookie domains worth extracting, and a stable key naming the per-site store dir.
// A copied store holds ONLY these domains' cookies — never the whole browser DB — so an import
// for one site can't leak your banking/email cookies to disk. A site whose auth genuinely spans a
// second registrable domain (Twitter/X: x.com + legacy twitter.com) lists both.

/// A known site: its `--cookie-import` keyword and `dl` routing, plus the cookie domains to keep.
struct Site {
    /// The keyword (`--cookie-import youtube`) and the per-site store dir name.
    name: &'static str,
    /// Host suffixes that route to this site — a URL or domain whose host equals one of these
    /// (or is a subdomain of it) selects this entry. `youtu.be` routes to `youtube`.
    aliases: &'static [&'static str],
    /// The cookie domains to extract. Usually just the site's own domain; list a second only when
    /// auth genuinely lives there (Twitter/X: `x.com` + legacy `twitter.com`). Deliberately NOT
    /// `google.com` for YouTube — yt-dlp reads YouTube auth from the `youtube.com` jar alone (the
    /// Google-SSO cookies are mirrored there), so adding it would copy the whole Google session to
    /// disk for nothing.
    cookie_domains: &'static [&'static str],
}

/// The curated known sites. `--cookie-import`'s help lists these names (a test keeps the two in
/// sync); anything not here is handled as a bare domain. Order is the help/listing order.
const SITES: &[Site] = &[
    Site { name: "youtube", aliases: &["youtube.com", "youtu.be", "youtube-nocookie.com"], cookie_domains: &["youtube.com"] },
    Site { name: "tiktok", aliases: &["tiktok.com"], cookie_domains: &["tiktok.com"] },
    Site { name: "facebook", aliases: &["facebook.com", "fb.watch"], cookie_domains: &["facebook.com"] },
    Site { name: "instagram", aliases: &["instagram.com"], cookie_domains: &["instagram.com"] },
    Site { name: "twitter", aliases: &["twitter.com", "x.com"], cookie_domains: &["twitter.com", "x.com"] },
    Site { name: "reddit", aliases: &["reddit.com"], cookie_domains: &["reddit.com"] },
    Site { name: "vimeo", aliases: &["vimeo.com"], cookie_domains: &["vimeo.com"] },
    Site { name: "twitch", aliases: &["twitch.tv"], cookie_domains: &["twitch.tv"] },
    Site { name: "niconico", aliases: &["nicovideo.jp", "nico.ms"], cookie_domains: &["nicovideo.jp"] },
    Site { name: "bilibili", aliases: &["bilibili.com", "b23.tv"], cookie_domains: &["bilibili.com"] },
    Site { name: "patreon", aliases: &["patreon.com"], cookie_domains: &["patreon.com"] },
    Site { name: "nebula", aliases: &["nebula.tv", "watchnebula.com"], cookie_domains: &["nebula.tv", "watchnebula.com"] },
    Site { name: "bbc", aliases: &["bbc.co.uk", "bbc.com"], cookie_domains: &["bbc.co.uk", "bbc.com"] },
];

/// Informal keywords that resolve to a curated site whose store key differs from the word people
/// type — the Twitter→X rebrand is the one that matters (`x` → the `twitter` store). Each value is
/// a site `name` in [`SITES`]; a test keeps every one pointing at a real site.
const KEYWORD_ALIASES: &[(&str, &str)] = &[("x", "twitter")];

/// A resolved cookie target: the per-site store dir name (`key`), the cookie domains to keep, and
/// a human `label` for messages. For a known site the key is its curated name; for anything else
/// it's the bare registrable domain, so an unknown site still gets its own isolated store.
#[derive(Debug, PartialEq)]
pub struct SiteTarget {
    pub key: String,
    pub domains: Vec<String>,
    pub label: String,
}

/// The comma-joined known-site names, for `--cookie-import`'s help text and the sync test.
pub fn known_site_names() -> String {
    SITES.iter().map(|s| s.name).collect::<Vec<_>>().join(", ")
}

/// Whether `input` is a cookie target we can actually resolve — a known site (keyword, informal
/// alias like `x`, or a URL/domain matching one), or a proper domain/URL whose host carries a dot.
/// A bare word that's neither (e.g. `tiktok2`) maps to no cookie domain, so `--cookie-import`
/// rejects it up front instead of building an empty store.
pub fn is_importable_target(input: &str) -> bool {
    site_for(input).is_some() || host_of(input.trim()).contains('.')
}

/// The curated [`Site`] an input names — a keyword by `name` (or an informal alias like `x` →
/// twitter), or a URL/domain by its host matching an alias. `None` for anything not curated. This
/// is the shared "which known site is this?" recognition: [`resolve_target`] and
/// [`is_importable_target`] use it today, and it's the seam a future caller can reuse to fold a
/// site's many addresses back to the one it owns.
fn site_for(input: &str) -> Option<&'static Site> {
    let input = input.trim();
    // A keyword matches by `name`, after mapping any informal alias (x → twitter) onto it.
    let canonical = KEYWORD_ALIASES
        .iter()
        .find(|(alias, _)| alias.eq_ignore_ascii_case(input))
        .map_or(input, |(_, name)| name);
    if let Some(site) = SITES.iter().find(|s| s.name.eq_ignore_ascii_case(canonical)) {
        return Some(site);
    }
    // Otherwise treat it as a URL/domain and match its host against the sites' aliases.
    SITES.iter().find(|s| host_matches_any(&host_of(input), s.aliases))
}

/// Resolve a `--cookie-import` argument — a site keyword (`youtube`), a domain (`tiktok.com`),
/// or a full URL (`https://www.tiktok.com/@u/video/1`) — to its [`SiteTarget`]. Also used per
/// `dl <url>` run to find which imported store (if any) serves the download's host.
pub fn resolve_target(input: &str) -> SiteTarget {
    if let Some(site) = site_for(input) {
        return site.target();
    }
    // Unknown → its own registrable domain, isolated store, only that domain's cookies.
    let domain = registrable_domain(&host_of(input.trim()));
    SiteTarget { key: domain.clone(), domains: vec![domain.clone()], label: domain }
}

impl Site {
    fn target(&self) -> SiteTarget {
        SiteTarget {
            key: self.name.to_string(),
            domains: self.cookie_domains.iter().map(|d| d.to_string()).collect(),
            label: self.name.to_string(),
        }
    }
}

/// True when `host` equals one of `domains` or is a subdomain of it (dot-boundary match, so
/// `evil-youtube.com` and `youtube.com.evil.test` don't match `youtube.com`).
fn host_matches_any(host: &str, domains: &[&str]) -> bool {
    domains.iter().any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

/// The lowercased host of a URL or bare domain — scheme, userinfo, port, path, and a leading
/// `www.` stripped. A value with no host (a bare path) yields `""`.
pub fn host_of(input: &str) -> String {
    let after_scheme = input.split_once("://").map_or(input, |(_, rest)| rest);
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or(authority); // drop any user:pass@
    let host = host.split(':').next().unwrap_or(host).to_lowercase(); // drop any :port
    host.strip_prefix("www.").unwrap_or(&host).to_string()
}

/// The registrable domain of a host by the simple last-two-labels rule (`m.example.com` →
/// `example.com`). Good for the common single-part TLDs; a multi-part TLD (`bbc.co.uk`) resolves
/// one label short (`co.uk`) — acceptable, since known sites match by alias before reaching here
/// and the fallback only scopes an unknown site's own store.
fn registrable_domain(host: &str) -> String {
    let labels: Vec<&str> = host.split('.').filter(|l| !l.is_empty()).collect();
    match labels.len() {
        0 => String::new(),
        n => labels[n.saturating_sub(2)..].join("."),
    }
}

/// Clear `site_dir` and create its empty `store/`, returned ready for a filtered import to write
/// the pared-down cookie DB into. Wiping first means a re-import of a site never mingles with the
/// previous one (and switching browsers for a site replaces cleanly).
pub fn reset_site(site_dir: &Path) -> std::io::Result<PathBuf> {
    let _ = std::fs::remove_dir_all(site_dir); // a stale prior import must not linger
    let store_dir = site_dir.join(STORE_SUBDIR);
    std::fs::create_dir_all(&store_dir)?;
    Ok(store_dir)
}

/// Record which yt-dlp browser a site's imported store came from, in `site_dir/browser.spec` —
/// the marker [`imported_spec`] reads to build the `--cookies-from-browser` spec.
pub fn write_spec(site_dir: &Path, browser: &str) -> std::io::Result<()> {
    std::fs::write(site_dir.join(SPEC_FILE), browser)
}

/// Drop a previously imported store — the copied DB and the spec marker both. Used to roll back
/// an import that read back as entirely unusable (e.g. a keyring-locked Chromium store yielding
/// zero cookies): leaving it would make every future `dl` run attempt the same doomed read,
/// so forgetting it keeps later runs clean until a working store is imported. Idempotent.
pub fn forget(cookies_root: &Path) -> std::io::Result<()> {
    let _ = std::fs::remove_file(cookies_root.join(SPEC_FILE));
    match std::fs::remove_dir_all(cookies_root.join(STORE_SUBDIR)) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// The `--cookies-from-browser` spec for a previously imported store (`<browser>:<abs dir>`), or
/// `None` when nothing has been imported. `dl` consults this on every run. yt-dlp's own
/// recursive search locates the DB within the dir, so the spec is the same for every family.
pub fn imported_spec(cookies_root: &Path) -> Option<String> {
    let browser = std::fs::read_to_string(cookies_root.join(SPEC_FILE)).ok()?;
    let browser = browser.trim();
    let store_dir = cookies_root.join(STORE_SUBDIR);
    let has_files = std::fs::read_dir(&store_dir).ok()?.flatten().next().is_some();
    (!browser.is_empty() && has_files).then(|| format!("{browser}:{}", store_dir.display()))
}

/// The cookie-DB family for a yt-dlp browser name: Firefox keeps its cookies in `moz_cookies`,
/// every Chromium variant in `cookies`. The import filter and the expiry check both key off this.
pub(crate) fn store_kind(browser: &str) -> &'static str {
    if browser == "firefox" {
        "firefox"
    } else {
        "chromium"
    }
}

/// The imported store's cookie DB file and its [`store_kind`] family, or `None` when nothing's
/// imported. The store dir holds exactly one file (the filtered DB), so we take it; the family
/// comes from the recorded spec. Lets a caller read the DB's plaintext columns (cookie expiry)
/// without yt-dlp or decryption. Sibling of [`imported_spec`].
pub fn imported_db(cookies_root: &Path) -> Option<(PathBuf, &'static str)> {
    let browser = std::fs::read_to_string(cookies_root.join(SPEC_FILE)).ok()?;
    let kind = store_kind(browser.trim());
    let db = std::fs::read_dir(cookies_root.join(STORE_SUBDIR))
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.is_file())?;
    Some((db, kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway `$HOME` for a test, cleaned on drop.
    struct FakeHome(PathBuf);
    impl FakeHome {
        fn new(tag: &str) -> Self {
            let home = std::env::temp_dir().join(format!("bashrs_browsers_{tag}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&home);
            std::fs::create_dir_all(&home).unwrap();
            FakeHome(home)
        }
        fn touch(&self, rel: &str) -> PathBuf {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "x").unwrap();
            path
        }
    }
    impl Drop for FakeHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn labels(stores: &[CookieStore]) -> Vec<&str> {
        stores.iter().map(|s| s.label.as_str()).collect()
    }

    #[test]
    fn firefox_stores_are_found_native_and_sandboxed_across_profiles() {
        let h = FakeHome::new("ff");
        h.touch(".mozilla/firefox/aa.default/cookies.sqlite");
        h.touch(".mozilla/firefox/bb.default-release/cookies.sqlite");
        h.touch(".mozilla/firefox/Crash Reports/InstallTime"); // not a profile: no cookies.sqlite
        h.touch(".var/app/org.mozilla.firefox/.mozilla/firefox/cc.default-release/cookies.sqlite");
        let stores = cookie_stores(&h.0);
        // Every profile with a DB, each install flavor — the Crash Reports dir excluded.
        // Sorted lexicographically by label (deterministic menu order).
        assert_eq!(
            labels(&stores),
            [
                "Firefox (flatpak) [default-release]",
                "Firefox (native) [default-release]",
                "Firefox (native) [default]",
            ]
        );
        assert!(stores.iter().all(|s| s.files == vec![(s.files[0].0.clone(), "cookies.sqlite")]));
    }

    #[test]
    fn firefox_xdg_config_location_is_found_ff147_plus() {
        // FF147 adopted the XDG base-directory spec: profiles moved from ~/.mozilla/firefox to
        // ~/.config/mozilla/firefox. A current install lives only there.
        let h = FakeHome::new("ffxdg");
        h.touch(".config/mozilla/firefox/aa.default-release/cookies.sqlite");
        let stores = cookie_stores(&h.0);
        assert_eq!(labels(&stores), ["Firefox (native) [default-release]"]);
        assert!(stores[0].files[0].0.ends_with(".config/mozilla/firefox/aa.default-release/cookies.sqlite"));
    }

    #[test]
    fn firefox_legacy_and_xdg_locations_dedupe_when_both_exist() {
        // A machine mid-migration may carry both trees; the same profile reached two ways must
        // not appear twice. (Distinct profiles in each still both show.)
        let h = FakeHome::new("ffboth");
        h.touch(".mozilla/firefox/aa.default-release/cookies.sqlite");
        h.touch(".config/mozilla/firefox/bb.dev-edition-default/cookies.sqlite");
        let stores = cookie_stores(&h.0);
        assert_eq!(
            labels(&stores),
            ["Firefox (native) [default-release]", "Firefox (native) [dev-edition-default]"]
        );
    }

    #[test]
    fn a_firefox_fork_is_found_under_its_own_home_dir() {
        // LibreWolf (and the other forks) keep a plaintext cookies.sqlite like Firefox but under
        // their own dotted home dir, and ride in under the `firefox` yt-dlp name.
        let h = FakeHome::new("librewolf");
        h.touch(".librewolf/aa.default/cookies.sqlite");
        let store = cookie_stores(&h.0).into_iter().find(|s| s.label.starts_with("LibreWolf")).expect("librewolf");
        assert_eq!(store.browser, "firefox", "reads as a Firefox store");
        assert_eq!(store.label, "LibreWolf (native) [default]");
    }

    #[test]
    fn ungoogled_chromium_flatpak_is_found_and_labeled() {
        // The Flathub build has its own app id (native shares ~/.config/chromium with Chromium),
        // so its Flatpak profile is what distinguishes it. Rides in as `chromium`.
        let h = FakeHome::new("ungoogled");
        h.touch(".var/app/io.github.ungoogled_software.ungoogled_chromium/config/chromium/Default/Network/Cookies");
        let store = cookie_stores(&h.0).into_iter().find(|s| s.label.starts_with("Ungoogled")).expect("ungoogled");
        assert_eq!(store.browser, "chromium", "rides in under the chromium keyring");
        assert_eq!(store.label, "Ungoogled Chromium (flatpak) [Default]");
    }

    #[test]
    fn firedragon_is_found_as_a_firefox_fork() {
        let h = FakeHome::new("firedragon");
        h.touch(".firedragon/aa.default/cookies.sqlite");
        let store = cookie_stores(&h.0).into_iter().find(|s| s.label.starts_with("FireDragon")).expect("firedragon");
        assert_eq!(store.browser, "firefox");
        assert_eq!(store.label, "FireDragon (native) [default]");
    }

    #[test]
    fn tor_and_mullvad_deeply_nested_profiles_are_found_by_recursive_search() {
        let h = FakeHome::new("tor");
        // Tor Browser (torbrowser-launcher native): profile buried below an arch dir.
        h.touch(".local/share/torbrowser/tbb/x86_64/tor-browser/Browser/TorBrowser/Data/Browser/profile.default/cookies.sqlite");
        // Mullvad Browser via Flatpak: nested under its app data dir.
        h.touch(".var/app/net.mullvad.MullvadBrowser/Browser/MullvadBrowser/Data/Browser/profile.default/cookies.sqlite");
        let stores = cookie_stores(&h.0);
        let labels = labels(&stores);
        assert!(labels.contains(&"Tor Browser (native) [default]"), "{labels:?}");
        assert!(labels.iter().any(|l| l.starts_with("Mullvad Browser (flatpak)")), "{labels:?}");
    }

    #[test]
    fn recursive_search_is_bounded_and_absent_roots_cost_nothing() {
        let h = FakeHome::new("norecurse");
        // No torbrowser/mullvad dirs at all → the recursive roots simply yield nothing.
        assert!(cookie_stores(&h.0).is_empty());
        // A cookies.sqlite buried deeper than the depth cap is not surfaced (guards a mispointed
        // root from walking a huge tree): 11 levels down under a search root.
        let deep = (0..11).map(|i| format!("d{i}")).collect::<Vec<_>>().join("/");
        h.touch(&format!(".local/share/torbrowser/{deep}/cookies.sqlite"));
        assert!(cookie_stores(&h.0).is_empty(), "beyond SEARCH_MAX_DEPTH stays unfound");
    }

    #[test]
    fn modern_chromium_cookies_under_network_are_found() {
        let h = FakeHome::new("crnet");
        h.touch(".config/chromium/Default/Network/Cookies"); // current layout
        let store = cookie_stores(&h.0).into_iter().find(|s| s.browser == "chromium").expect("store");
        assert_eq!(store.label, "Chromium (native) [Default]");
        assert!(store.files[0].0.ends_with("Default/Network/Cookies"), "{:?}", store.files);
        assert_eq!(store.files[0].1, "Cookies", "flattened to the name yt-dlp searches for");
    }

    #[test]
    fn legacy_chromium_cookies_flat_in_the_profile_are_found() {
        let h = FakeHome::new("crold");
        h.touch(".config/google-chrome/Default/Cookies"); // pre-2021 layout
        let store = cookie_stores(&h.0).into_iter().find(|s| s.browser == "chrome").expect("store");
        assert!(store.files[0].0.ends_with("Default/Cookies"));
    }

    #[test]
    fn every_chromium_profile_with_cookies_is_offered() {
        let h = FakeHome::new("crprofiles");
        h.touch(".config/chromium/Default/Network/Cookies");
        h.touch(".config/chromium/Profile 1/Network/Cookies");
        h.touch(".config/chromium/System Profile/Preferences"); // no cookies → excluded
        assert_eq!(
            labels(&cookie_stores(&h.0)),
            ["Chromium (native) [Default]", "Chromium (native) [Profile 1]"]
        );
    }

    #[test]
    fn nothing_is_found_on_a_bare_home() {
        let h = FakeHome::new("bare");
        assert!(cookie_stores(&h.0).is_empty());
        assert!(!any_browser_installed(&h.0));
    }

    #[test]
    fn any_browser_installed_notices_a_sandbox_profile_without_cookies() {
        let h = FakeHome::new("installed");
        h.touch(".var/app/org.chromium.Chromium/config/chromium/Default/Preferences");
        assert!(any_browser_installed(&h.0), "a flatpak profile dir counts as installed");
        assert!(cookie_stores(&h.0).is_empty(), "but with no cookie DB there is nothing to import");
    }

    // The DB filtering itself is python-backed (verified live); these cover the pure-fs store
    // layout the driver writes into and the caller reads back.

    #[test]
    fn store_layout_round_trips_a_uniform_spec_and_reset_wipes_it() {
        let h = FakeHome::new("layout");
        let site_dir = h.0.join(".bashrs/user-data/browser_cookies/youtube");
        assert!(imported_spec(&site_dir).is_none(), "nothing imported yet");

        let store_dir = reset_site(&site_dir).unwrap();
        std::fs::write(store_dir.join("cookies.sqlite"), "db").unwrap(); // stand-in for a filtered DB
        write_spec(&site_dir, "firefox").unwrap();
        let spec = imported_spec(&site_dir).expect("a spec once a store exists");
        assert!(spec.starts_with("firefox:") && spec.ends_with("youtube/store"), "{spec}");

        // A re-import wipes the whole site dir first — a stale DB from a prior browser can't linger.
        let store_dir2 = reset_site(&site_dir).unwrap();
        assert_eq!(std::fs::read_dir(&store_dir2).unwrap().count(), 0, "store emptied on reset");
        assert!(imported_spec(&site_dir).is_none(), "spec gone until re-written");
    }

    #[test]
    fn per_site_dirs_keep_different_sites_and_browsers_apart() {
        // A user can import youtube from Firefox and tiktok from Chrome — the two stores coexist
        // in their own subdirs and each resolves independently.
        let base = FakeHome::new("persite").0.join("browser_cookies");
        let yt = base.join("youtube");
        let tk = base.join("tiktok");
        std::fs::write(reset_site(&yt).unwrap().join("cookies.sqlite"), "ff").unwrap();
        write_spec(&yt, "firefox").unwrap();
        std::fs::write(reset_site(&tk).unwrap().join("Cookies"), "cr").unwrap();
        write_spec(&tk, "chrome").unwrap();
        assert!(imported_spec(&yt).unwrap().starts_with("firefox:"));
        assert!(imported_spec(&tk).unwrap().starts_with("chrome:"));
        forget(&yt).unwrap();
        assert!(imported_spec(&yt).is_none(), "forgetting youtube leaves tiktok alone");
        assert!(imported_spec(&tk).is_some());
    }

    #[test]
    fn forget_removes_the_store_and_spec_and_is_idempotent() {
        let site_dir = FakeHome::new("forget").0.join("browser_cookies/vimeo");
        std::fs::write(reset_site(&site_dir).unwrap().join("cookies.sqlite"), "db").unwrap();
        write_spec(&site_dir, "firefox").unwrap();
        assert!(imported_spec(&site_dir).is_some());

        forget(&site_dir).unwrap();
        assert!(imported_spec(&site_dir).is_none(), "spec + store gone after forget");
        assert!(!site_dir.join(STORE_SUBDIR).exists());
        forget(&site_dir).unwrap(); // idempotent: a second forget is a clean no-op
    }

    #[test]
    fn resolve_target_maps_keywords_domains_and_urls_to_sites() {
        // A keyword, a domain, and a URL all resolve to the youtube site — keeping only youtube.com
        // cookies (never google.com: yt-dlp reads YouTube auth from the youtube.com jar alone, so
        // pulling google.com would copy the user's whole Google session to disk for nothing).
        for input in ["youtube", "youtube.com", "https://www.youtube.com/watch?v=x", "https://youtu.be/x"] {
            let t = resolve_target(input);
            assert_eq!(t.key, "youtube", "{input}");
            assert_eq!(t.domains, ["youtube.com"], "youtube keeps only its own domain: {input}");
        }
        // A URL to a known site isolates by keyword; twitter aliases x.com.
        assert_eq!(resolve_target("https://x.com/i/status/1").key, "twitter");
        // An unknown site → its own registrable domain, only that domain.
        let unknown = resolve_target("https://videos.example.co/watch/9");
        assert_eq!(unknown.key, "example.co");
        assert_eq!(unknown.domains, ["example.co"]);
    }

    #[test]
    fn added_sites_resolve_by_keyword_and_url_including_dual_domain_ones() {
        // Single-domain additions: the keyword lands on the site's own domain, a URL routes by alias.
        assert_eq!(resolve_target("bilibili").domains, ["bilibili.com"]);
        assert_eq!(resolve_target("https://www.bilibili.com/video/BV1").key, "bilibili");
        assert_eq!(resolve_target("niconico").domains, ["nicovideo.jp"]);
        assert_eq!(resolve_target("patreon").domains, ["patreon.com"]);
        // Nebula keeps its current and legacy domains.
        assert_eq!(resolve_target("nebula").domains, ["nebula.tv", "watchnebula.com"]);
        // BBC spans two registrable domains; a bbc.co.uk URL matches by alias, so it keeps BOTH —
        // and never falls back to the last-two-labels rule, which would wrongly yield just `co.uk`.
        let bbc = resolve_target("https://www.bbc.co.uk/iplayer/episode/x");
        assert_eq!(bbc.key, "bbc");
        assert_eq!(bbc.domains, ["bbc.co.uk", "bbc.com"]);
        // Every added keyword is importable.
        for kw in ["niconico", "bilibili", "patreon", "nebula", "bbc"] {
            assert!(is_importable_target(kw), "{kw}");
        }
    }

    #[test]
    fn importable_targets_are_keywords_or_dotted_hosts_not_bare_words() {
        assert!(is_importable_target("tiktok")); // known keyword
        assert!(is_importable_target("example.com")); // bare domain
        assert!(is_importable_target("https://www.tiktok.com/@u/video/1")); // URL
        assert!(!is_importable_target("tiktok2")); // a bare word, not a keyword, no dot
        assert!(!is_importable_target("com")); // not a domain on its own
    }

    #[test]
    fn host_of_strips_scheme_userinfo_port_path_and_www() {
        assert_eq!(host_of("https://www.youtube.com/watch?v=x"), "youtube.com");
        assert_eq!(host_of("http://user:pass@Host.EXAMPLE.com:8080/a/b"), "host.example.com");
        assert_eq!(host_of("youtu.be/abc"), "youtu.be"); // scheme-less
        assert_eq!(host_of("tiktok.com"), "tiktok.com"); // bare domain
        assert_eq!(host_of("/local/path.mp4"), ""); // no host
    }

    #[test]
    fn known_site_names_stay_in_sync_with_the_help_text() {
        // `--cookie-import`'s help hardcodes this list; if a site is added/removed here, update
        // the DlArgs help too (this guard makes the drift a test failure, not a silent mismatch).
        assert_eq!(
            known_site_names(),
            "youtube, tiktok, facebook, instagram, twitter, reddit, vimeo, twitch, niconico, bilibili, patreon, nebula, bbc"
        );
    }

    #[test]
    fn keyword_aliases_resolve_to_real_sites_and_x_means_twitter() {
        for (alias, name) in KEYWORD_ALIASES {
            assert!(SITES.iter().any(|s| s.name == *name), "alias {alias} points at unknown site {name}");
        }
        assert_eq!(resolve_target("x").key, "twitter"); // the Twitter→X rebrand keyword
        assert!(is_importable_target("x"), "x resolves via the twitter alias, so it's importable");
    }

    #[test]
    fn imported_db_finds_the_store_file_and_its_family() {
        assert_eq!(store_kind("firefox"), "firefox");
        assert_eq!(store_kind("chrome"), "chromium");

        let site_dir = FakeHome::new("impdb").0.join("browser_cookies/tiktok");
        assert!(imported_db(&site_dir).is_none(), "nothing imported yet");
        let store_dir = reset_site(&site_dir).unwrap();
        std::fs::write(store_dir.join("cookies.sqlite"), "db").unwrap();
        write_spec(&site_dir, "firefox").unwrap();
        let (db, kind) = imported_db(&site_dir).expect("a db once imported");
        assert!(db.ends_with("cookies.sqlite"), "{db:?}");
        assert_eq!(kind, "firefox");
    }
}
