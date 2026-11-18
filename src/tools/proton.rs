//! Proton, the middleware `gm_run_exe` runs Windows games through: every copy this computer has
//! ([`installed`]) — the system's, wherever Steam or a package put one, and bashrs's own bundle
//! ([`PROTON_DIR`], fetched by [`super::fetch`] when the system has none, or always under
//! `[tools] always_bundle_middleware`) — each read off its own folder: its name and build time
//! from its `version` file, and whether it can run a game on Wayland itself.
//!
//! A Proton is a folder holding a `proton` script and a `version` file. Steam keeps Valve's in its
//! libraries' `steamapps/common`, and other builds (GE-Proton, proton-cachyos, …) in a
//! `compatibilitytools.d`: its own, and the system-wide ones packages install into. Where Steam
//! keeps itself and its libraries ([`steam_roots`], [`libraries`]) is read here once, for
//! `gm_id_steam` too.

use std::path::{Path, PathBuf};

/// The bundle's directory in the tools' home: its [`super::TOOLS`] row's, and where [`installed`]
/// looks for it.
pub(crate) const PROTON_DIR: &str = "proton";

/// One copy of Proton.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Proton {
    /// Its folder, holding the `proton` script.
    pub dir: PathBuf,
    /// Its name, as its `version` file gives it (`cachyos-11.0-20261015-slr`,
    /// `experimental-11.0-20261112-x86_64`, `GE-Proton11-12`).
    pub name: String,
    /// When it was built, in Unix seconds, from the same file: what "newest" goes by.
    pub built: u64,
    /// Whether it can run a game on Wayland itself: it ships Wine's Wayland driver, and its script
    /// switches it on by `PROTON_ENABLE_WAYLAND`.
    pub wayland: bool,
    /// Whether it is bashrs's own bundle.
    pub bundled: bool,
}

/// Every Proton on this computer, newest first: bashrs's bundle and the system's, each once.
pub(crate) fn installed() -> Vec<Proton> {
    let bundle = super::root().join(PROTON_DIR);
    let mut protons: Vec<Proton> = read(&bundle, true).into_iter().collect();
    protons.extend(found_in(&search_dirs(&crate::conf::home(), SYSTEM_TOOL_DIRS)));
    newest_first(protons)
}

/// The Proton in `dir`, wherever it is: one named by its folder.
pub(crate) fn at(dir: &Path) -> Option<Proton> {
    let bundle = std::fs::canonicalize(super::root().join(PROTON_DIR)).ok();
    read(dir, bundle.is_some() && std::fs::canonicalize(dir).ok() == bundle)
}

/// The system's newest Proton, for the bundling plan ([`super::Tool::system`]): bashrs bundles
/// one only where this finds none, unless `[tools] always_bundle_middleware` says to anyway.
pub(super) fn on_system(_probe: &str) -> Option<PathBuf> {
    newest_first(found_in(&search_dirs(&crate::conf::home(), SYSTEM_TOOL_DIRS))).into_iter().next().map(|proton| proton.dir)
}

/// The system-wide `compatibilitytools.d` folders packages install Protons into, which Steam
/// reads beside its own.
const SYSTEM_TOOL_DIRS: &[&str] = &["/usr/share/steam/compatibilitytools.d", "/usr/local/share/steam/compatibilitytools.d"];

/// Where Steam keeps itself under a home: the native install, the Flatpak and the Snap.
const STEAM_ROOTS: &[&str] =
    &[".local/share/Steam", ".var/app/com.valvesoftware.Steam/.local/share/Steam", "snap/steam/common/.local/share/Steam"];

/// Where Steam keeps itself under `home`: each of [`STEAM_ROOTS`] that is there.
pub(crate) fn steam_roots(home: &Path) -> Vec<PathBuf> {
    STEAM_ROOTS.iter().map(|root| home.join(root)).filter(|root| root.is_dir()).collect()
}

/// The libraries of the Steam at `root`: its own folder, then those its `libraryfolders.vdf` lists
/// (its own among them, as Steam writes the file).
pub(crate) fn libraries(root: &Path) -> Vec<PathBuf> {
    let listed = std::fs::read_to_string(root.join("steamapps/libraryfolders.vdf")).unwrap_or_default();
    std::iter::once(root.to_path_buf()).chain(library_paths(&listed)).collect()
}

/// The folders whose sub-folders may be Protons: each Steam root's `compatibilitytools.d`, the
/// `steamapps/common` of each of its [`libraries`], and the system-wide `system_dirs`.
fn search_dirs(home: &Path, system_dirs: &[&str]) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for root in steam_roots(home) {
        dirs.push(root.join("compatibilitytools.d"));
        dirs.extend(libraries(&root).into_iter().map(|library| library.join("steamapps/common")));
    }
    dirs.extend(system_dirs.iter().map(PathBuf::from));
    dirs
}

/// The libraries a `libraryfolders.vdf` lists: the value of each `"path"` key, its doubled
/// backslashes undone (Valve's KeyValues format escapes them).
fn library_paths(vdf: &str) -> Vec<PathBuf> {
    vdf.lines()
        .filter_map(|line| line.trim().strip_prefix("\"path\""))
        .filter_map(|value| value.trim().strip_prefix('"')?.strip_suffix('"').map(|path| PathBuf::from(path.replace("\\\\", "\\"))))
        .collect()
}

/// The Protons among the sub-folders of `dirs`, each once however many ways it was reached (a
/// library listed twice, `~/.steam/root` beside `~/.local/share/Steam`).
fn found_in(dirs: &[PathBuf]) -> Vec<Proton> {
    let mut seen = std::collections::HashSet::new();
    dirs.iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| read(&entry.path(), false))
        .filter(|proton| seen.insert(std::fs::canonicalize(&proton.dir).unwrap_or_else(|_| proton.dir.clone())))
        .collect()
}

/// `protons` ordered newest build first, a tie by name.
fn newest_first(mut protons: Vec<Proton>) -> Vec<Proton> {
    protons.sort_by(|a, b| b.built.cmp(&a.built).then_with(|| a.name.cmp(&b.name)));
    protons
}

/// The Proton in `dir`, if it holds one: a `proton` script, and a `version` file reading
/// `<build time> <name>`.
fn read(dir: &Path, bundled: bool) -> Option<Proton> {
    if !dir.join("proton").is_file() {
        return None;
    }
    let version = std::fs::read_to_string(dir.join("version")).ok()?;
    let (built, name) = version.lines().next()?.trim().split_once(' ')?;
    Some(Proton {
        dir: dir.to_path_buf(),
        name: name.trim().to_string(),
        built: built.parse().ok()?,
        wayland: wayland_capable(dir),
        bundled,
    })
}

/// Whether the Proton in `dir` can run a game on Wayland itself: it ships Wine's Wayland driver,
/// and its script reads the switch that turns it on.
fn wayland_capable(dir: &Path) -> bool {
    let driver = WAYLAND_DRIVER.iter().any(|path| dir.join(path).is_file());
    driver && std::fs::read_to_string(dir.join("proton")).is_ok_and(|script| script.contains(WAYLAND_SWITCH))
}

/// Where a Proton keeps Wine's Wayland driver, by the layouts Protons have had.
const WAYLAND_DRIVER: &[&str] = &["files/lib/wine/x86_64-unix/winewayland.so", "files/lib64/wine/x86_64-unix/winewayland.so"];

/// The environment variable that switches a Proton's Wayland driver on.
pub(crate) const WAYLAND_SWITCH: &str = "PROTON_ENABLE_WAYLAND";

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A scratch directory for one test, gone afterwards.
    pub(crate) fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bashrs_proton_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A stand-in Proton in `dir`: a `proton` script (`body` its text), a `version` file, and Wine's
    /// Wayland driver when `wayland` says so.
    pub(crate) fn fake_proton(dir: &Path, version: &str, wayland: bool, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        let script = dir.join("proton");
        std::fs::write(&script, body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(dir.join("version"), format!("{version}\n")).unwrap();
        if wayland {
            let driver = dir.join(WAYLAND_DRIVER[0]);
            std::fs::create_dir_all(driver.parent().unwrap()).unwrap();
            std::fs::write(driver, "").unwrap();
        }
    }

    /// A Proton is read off its own files: the name and build time from `version`, and Wayland
    /// only where both the driver and the switch are there. A folder missing either file is none.
    #[test]
    fn a_proton_is_read_off_its_version_file_and_its_driver() {
        let base = scratch("read");
        fake_proton(&base.join("cachy"), "1792530497 cachyos-11.0-20261015-slr", true, "x = 'PROTON_ENABLE_WAYLAND'\n");
        fake_proton(&base.join("valve"), "1794497090 experimental-11.0-20261112-x86_64", false, "# no switch\n");
        fake_proton(&base.join("driver-no-switch"), "1 odd", true, "# no switch\n");
        let cachy = read(&base.join("cachy"), false).unwrap();
        assert_eq!((cachy.name.as_str(), cachy.built, cachy.wayland), ("cachyos-11.0-20261015-slr", 1792530497, true));
        assert!(!read(&base.join("valve"), false).unwrap().wayland);
        assert!(!read(&base.join("driver-no-switch"), false).unwrap().wayland, "a driver nothing switches on is no use");
        assert!(read(&base.join("valve"), true).unwrap().bundled);

        std::fs::create_dir_all(base.join("no-version")).unwrap();
        std::fs::write(base.join("no-version/proton"), "").unwrap();
        std::fs::create_dir_all(base.join("no-script")).unwrap();
        std::fs::write(base.join("no-script/version"), "1 x\n").unwrap();
        std::fs::create_dir_all(base.join("bad-version")).unwrap();
        std::fs::write(base.join("bad-version/proton"), "").unwrap();
        std::fs::write(base.join("bad-version/version"), "soon\n").unwrap();
        for not_one in ["no-version", "no-script", "bad-version", "nowhere"] {
            assert_eq!(read(&base.join(not_one), false), None, "{not_one}");
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The places searched: each Steam root's compatibility tools and libraries (its own and the
    /// ones `libraryfolders.vdf` adds), then the system's; every Proton found once, newest first,
    /// and anything that is no Proton (a Steam runtime, a game) passed over.
    #[test]
    fn protons_are_found_in_steams_libraries_and_tool_folders_newest_first() {
        let home = scratch("search");
        let steam = home.join(".local/share/Steam");
        let other_library = home.join("games-disk");
        let system = home.join("usr-share-steam-compatibilitytools.d");
        std::fs::create_dir_all(steam.join("steamapps")).unwrap();
        std::fs::write(
            steam.join("steamapps/libraryfolders.vdf"),
            format!(
                "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
                steam.display(),
                other_library.display()
            ),
        )
        .unwrap();
        fake_proton(&steam.join("steamapps/common/Proton - Experimental"), "300 experimental-11.0", false, "");
        fake_proton(&other_library.join("steamapps/common/Proton 9.0 (Beta)"), "100 proton-9.0", false, "");
        fake_proton(&steam.join("compatibilitytools.d/GE-Proton11-12"), "200 GE-Proton11-12", true, "PROTON_ENABLE_WAYLAND");
        fake_proton(&system.join("proton-cachyos-slr"), "250 cachyos-11.0", true, "PROTON_ENABLE_WAYLAND");
        std::fs::create_dir_all(steam.join("steamapps/common/SteamLinuxRuntime_4")).unwrap();
        std::fs::create_dir_all(steam.join("steamapps/common/Some Game")).unwrap();
        std::os::unix::fs::symlink(&steam, home.join("steam-again")).unwrap();

        let system_dir = system.to_str().unwrap();
        let mut dirs = search_dirs(&home, &[system_dir]);
        dirs.push(home.join("steam-again/compatibilitytools.d")); // the same tools, reached again
        let names: Vec<String> = newest_first(found_in(&dirs)).into_iter().map(|proton| proton.name).collect();
        assert_eq!(names, ["experimental-11.0", "cachyos-11.0", "GE-Proton11-12", "proton-9.0"]);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A library path is the `"path"` value as Valve writes it, backslashes undone.
    #[test]
    fn library_paths_are_read_from_the_vdf() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/home/u/.local/share/Steam\"\n\t\t\"label\"\t\t\"\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"/mnt/games\\\\x\"\n\t}\n}\n";
        assert_eq!(library_paths(vdf), [PathBuf::from("/home/u/.local/share/Steam"), PathBuf::from("/mnt/games\\x")]);
        assert!(library_paths("").is_empty());
    }
}
