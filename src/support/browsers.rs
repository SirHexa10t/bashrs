//! Browser cookie-store discovery and import, backing `dl_yt --cookie-import`. Rather than hand
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

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::support::package_management as pm;

/// A copyable cookie store found on disk, ready for the `--cookie-import` menu.
#[derive(Debug, PartialEq)]
pub struct CookieStore {
    /// Menu line, e.g. `Firefox (native) [default-release]`.
    pub label: String,
    /// yt-dlp's browser name, recorded so `dl_yt` can name the copy back to it.
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

/// A browser family: its store shape, yt-dlp name, the binaries that betray a native install,
/// its Flatpak app id and Snap name, and where its profiles live relative to an app root
/// (Firefox off the home root, Chromium off the XDG-config root).
struct Family {
    kind: Kind,
    label: &'static str,
    ytdlp: &'static str,
    binaries: &'static [&'static str],
    flatpak_id: &'static str,
    snap_name: &'static str,
    subpath: &'static str,
}

const FAMILIES: &[Family] = &[
    Family { kind: Kind::Firefox, label: "Firefox", ytdlp: "firefox", binaries: &["firefox", "firefox-esr"], flatpak_id: "org.mozilla.firefox", snap_name: "firefox", subpath: ".mozilla/firefox" },
    Family { kind: Kind::Chromium, label: "Chrome", ytdlp: "chrome", binaries: &["google-chrome", "google-chrome-stable"], flatpak_id: "com.google.Chrome", snap_name: "", subpath: "google-chrome" },
    Family { kind: Kind::Chromium, label: "Chromium", ytdlp: "chromium", binaries: &["chromium", "chromium-browser"], flatpak_id: "org.chromium.Chromium", snap_name: "chromium", subpath: "chromium" },
    Family { kind: Kind::Chromium, label: "Brave", ytdlp: "brave", binaries: &["brave", "brave-browser"], flatpak_id: "com.brave.Browser", snap_name: "brave", subpath: "BraveSoftware/Brave-Browser" },
    Family { kind: Kind::Chromium, label: "Edge", ytdlp: "edge", binaries: &["microsoft-edge", "microsoft-edge-stable"], flatpak_id: "com.microsoft.Edge", snap_name: "", subpath: "microsoft-edge" },
    Family { kind: Kind::Chromium, label: "Opera", ytdlp: "opera", binaries: &["opera"], flatpak_id: "com.opera.Opera", snap_name: "opera", subpath: "opera" },
    Family { kind: Kind::Chromium, label: "Vivaldi", ytdlp: "vivaldi", binaries: &["vivaldi", "vivaldi-stable"], flatpak_id: "com.vivaldi.Vivaldi", snap_name: "", subpath: "vivaldi" },
];

/// Where `dl_yt` keeps the imported copy, under the given cookies root.
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
    let mut stores = Vec::new();
    let roots = match family.kind {
        Kind::Firefox => pm::app_home_roots(home, family.flatpak_id, family.snap_name),
        Kind::Chromium => pm::app_config_roots(home, family.flatpak_id, family.snap_name),
    };
    for root in roots {
        let base = root.join(family.subpath); // profiles-parent (firefox) or user-data-dir (chromium)
        for profile in profile_dirs(&base) {
            if let Some(files) = cookie_files(family.kind, &profile) {
                stores.push(CookieStore {
                    label: format!("{} ({}) [{}]", family.label, flavor(&root), profile_label(family.kind, &profile)),
                    browser: family.ytdlp,
                    files,
                });
            }
        }
    }
    stores
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

/// The `--cookies-from-browser` spec for a previously imported store (`<browser>:<abs dir>`), or
/// `None` when nothing has been imported. `dl_yt` consults this on every run. yt-dlp's own
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
}
