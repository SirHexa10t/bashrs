//! Browser cookie-store discovery and import, backing `dl --cookie-import`. Rather than hand
//! yt-dlp a live browser profile (whose cookie DB a running browser keeps locked), the import
//! *copies* the store into bashrs's own data dir and points yt-dlp there — a copy has no lock.
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

/// Copy `store` into `cookies_root/store/` (replacing any previous import) and record its
/// browser in `cookies_root/browser.spec`. The copy is what frees the cookies from the running
/// browser's file lock.
pub fn import(store: &CookieStore, cookies_root: &Path) -> std::io::Result<()> {
    let store_dir = cookies_root.join(STORE_SUBDIR);
    let _ = std::fs::remove_dir_all(&store_dir); // a stale prior import must not linger
    std::fs::create_dir_all(&store_dir)?;
    for (src, dest_name) in &store.files {
        std::fs::copy(src, store_dir.join(dest_name))?;
    }
    std::fs::write(cookies_root.join(SPEC_FILE), store.browser)
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
    fn import_flattens_the_store_and_round_trips_a_uniform_spec() {
        let h = FakeHome::new("import");
        h.touch(".config/chromium/Default/Network/Cookies");
        let store = cookie_stores(&h.0).into_iter().find(|s| s.browser == "chromium").unwrap();
        let cookies_root = h.0.join(".bashrs/user-data/browser_cookies");

        assert!(imported_spec(&cookies_root).is_none(), "nothing imported yet");
        import(&store, &cookies_root).unwrap();
        assert!(cookies_root.join("store/Cookies").is_file(), "DB flattened to store/Cookies");
        let spec = imported_spec(&cookies_root).expect("a spec after import");
        assert!(spec.starts_with("chromium:") && spec.ends_with("browser_cookies/store"), "{spec}");

        // Re-importing replaces cleanly — no leftover files from a prior store.
        import(&store, &cookies_root).unwrap();
        assert_eq!(std::fs::read_dir(cookies_root.join("store")).unwrap().count(), 1);
    }

    #[test]
    fn any_browser_installed_notices_a_sandbox_profile_without_cookies() {
        let h = FakeHome::new("installed");
        h.touch(".var/app/org.chromium.Chromium/config/chromium/Default/Preferences");
        assert!(any_browser_installed(&h.0), "a flatpak profile dir counts as installed");
        assert!(cookie_stores(&h.0).is_empty(), "but with no cookie DB there is nothing to import");
    }

    #[test]
    fn re_importing_a_different_family_fully_replaces_the_prior_store() {
        // The exact sequence a user runs when switching browsers: import Chromium, then Firefox.
        // The Chromium `Cookies` must NOT linger beside the fresh `cookies.sqlite` — `import`
        // wipes `store/` first, so the dir reflects only the newest import. (Tested at `import`
        // level so the guarantee holds independent of the caller's post-import validation.)
        let h = FakeHome::new("swap");
        h.touch(".config/google-chrome/Default/Network/Cookies");
        h.touch(".mozilla/firefox/ab.default-release/cookies.sqlite");
        let cookies_root = h.0.join(".bashrs/user-data/browser_cookies");

        let chromium = cookie_stores(&h.0).into_iter().find(|s| s.browser == "chrome").unwrap();
        import(&chromium, &cookies_root).unwrap();
        assert!(cookies_root.join("store/Cookies").is_file(), "chromium DB in place");

        let firefox = cookie_stores(&h.0).into_iter().find(|s| s.browser == "firefox").unwrap();
        import(&firefox, &cookies_root).unwrap();
        let names: Vec<String> = std::fs::read_dir(cookies_root.join("store"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["cookies.sqlite"], "only the new Firefox DB remains — no stale Cookies");
        assert_eq!(imported_spec(&cookies_root).unwrap().split(':').next(), Some("firefox"));
    }

    #[test]
    fn forget_removes_the_store_and_spec_and_is_idempotent() {
        let h = FakeHome::new("forget");
        h.touch(".config/chromium/Default/Network/Cookies");
        let store = cookie_stores(&h.0).into_iter().find(|s| s.browser == "chromium").unwrap();
        let cookies_root = h.0.join(".bashrs/user-data/browser_cookies");
        import(&store, &cookies_root).unwrap();
        assert!(imported_spec(&cookies_root).is_some());

        forget(&cookies_root).unwrap();
        assert!(imported_spec(&cookies_root).is_none(), "spec + store gone after forget");
        assert!(!cookies_root.join(STORE_SUBDIR).exists());
        forget(&cookies_root).unwrap(); // idempotent: a second forget is a clean no-op
    }
}
