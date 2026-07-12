//! The canonical registry of system package managers — one source of truth so a manager added
//! here is seen everywhere at once. Two subsystems build on it: the `packages_*` commands
//! ([`crate::categories::packages`]) drive upgrades over the managers detected here, and the
//! browser-cookie scan ([`crate::support::browsers`]) derives its app-data search roots from
//! each manager's [`Store`] kind. Centralizing the list is deliberate: the scan once hardcoded
//! Flatpak/Snap and silently omitted Nix — with the enumeration shared, a gap in one consumer
//! can't hide from the other (a cross-check test in `packages` pins the two together).

use std::path::{Path, PathBuf};

use crate::support::exec;

/// Where a manager places an application's per-user data — the only distinction the cookie scan
/// cares about, since it decides where a browser's profile (and its cookie store) will live.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Store {
    /// Standard XDG/home locations (`~/.config`, `~/.mozilla`, …) — apt, dnf, pacman, nix,
    /// brew, and every from-source install share these.
    Native,
    /// Flatpak's per-app sandbox home, `~/.var/app/<app-id>/` (XDG config is `config/`, dotless).
    Flatpak,
    /// Snap's per-snap trees, `~/snap/<name>/{common,current}/`.
    Snap,
}

/// One package manager: the binary that detects it, how it stores app data, and any binary
/// directories outside `PATH` where it may still live (Nix and Homebrew only reach `PATH` once
/// the shell has sourced their environment).
pub struct Manager {
    pub command: &'static str,
    pub store: Store,
    /// Extra binary roots to probe beyond `PATH`. `~/`-prefixed entries expand against `$HOME`.
    pub bin_roots: &'static [&'static str],
}

/// Nix's profile binary roots — the user profile, the multi-user default, and the NixOS system
/// profile. Shared by both nix entries.
const NIX_ROOTS: &[&str] =
    &["~/.nix-profile/bin", "/nix/var/nix/profiles/default/bin", "/run/current-system/sw/bin"];

/// Every package manager bashrs knows. The `packages` category attaches an upgrade recipe to
/// each (a test enforces the 1:1 pairing); the cookie scan reads each one's [`Store`].
pub const MANAGERS: &[Manager] = &[
    Manager { command: "apt", store: Store::Native, bin_roots: &[] },
    Manager { command: "dnf", store: Store::Native, bin_roots: &[] },
    Manager { command: "yay", store: Store::Native, bin_roots: &[] }, // AUR helper — installs pacman-style, native data
    Manager { command: "pacman", store: Store::Native, bin_roots: &[] },
    Manager { command: "zypper", store: Store::Native, bin_roots: &[] },
    Manager { command: "apk", store: Store::Native, bin_roots: &[] },
    Manager { command: "guix", store: Store::Native, bin_roots: &[] },
    Manager { command: "nix-env", store: Store::Native, bin_roots: NIX_ROOTS },
    Manager { command: "nix", store: Store::Native, bin_roots: NIX_ROOTS },
    Manager { command: "snap", store: Store::Snap, bin_roots: &[] },
    Manager { command: "flatpak", store: Store::Flatpak, bin_roots: &[] },
    Manager { command: "brew", store: Store::Native, bin_roots: &["~/.linuxbrew/bin", "/home/linuxbrew/.linuxbrew/bin"] },
];

/// Expand a `bin_roots` entry (`~/`-relative or absolute) against `home`.
fn expand(home: &Path, root: &str) -> PathBuf {
    match root.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(root),
    }
}

/// Whether `command` is available — on `PATH`, or in any manager's extra binary roots.
fn command_present(home: &Path, command: &str, bin_roots: &[&str]) -> bool {
    exec::on_path(command) || bin_roots.iter().any(|root| expand(home, root).join(command).exists())
}

/// The managers present on this system (`command_present`), in registry order.
pub fn present(home: &Path) -> Vec<&'static Manager> {
    MANAGERS.iter().filter(|m| command_present(home, m.command, m.bin_roots)).collect()
}

/// Whether a native binary `bin` exists anywhere bashrs looks — `PATH` plus every manager's
/// extra roots (so a Nix- or Homebrew-installed program is found even when its environment
/// isn't sourced). Backs [`crate::support::browsers`]'s native-install detection.
pub fn native_binary_present(home: &Path, bin: &str) -> bool {
    exec::on_path(bin)
        || MANAGERS.iter().any(|m| m.bin_roots.iter().any(|r| expand(home, r).join(bin).exists()))
}

/// Every candidate *home* root an app's data might sit under — the native `$HOME`, plus each
/// sandbox manager's per-app home. Firefox-family profiles hang off these (`.mozilla/...`).
pub fn app_home_roots(home: &Path, flatpak_id: &str, snap_name: &str) -> Vec<PathBuf> {
    let mut roots = vec![home.to_path_buf()];
    for manager in MANAGERS {
        roots.extend(sandbox_roots(manager.store, home, flatpak_id, snap_name));
    }
    roots
}

/// Every candidate *XDG-config* root — the native `~/.config`, plus each sandbox manager's
/// config location (Flatpak's dotless `config/`, Snap's `.config` under both trees, and the
/// sandbox home itself, since some snaps drop config there directly). Chromium-family profiles
/// hang off these. Generous by design: non-existent candidates are filtered by the caller.
pub fn app_config_roots(home: &Path, flatpak_id: &str, snap_name: &str) -> Vec<PathBuf> {
    let mut roots = vec![home.join(".config")];
    for manager in MANAGERS {
        for base in sandbox_roots(manager.store, home, flatpak_id, snap_name) {
            match manager.store {
                Store::Flatpak => roots.push(base.join("config")),
                Store::Snap => {
                    roots.push(base.join(".config"));
                    roots.push(base); // some snaps place config at the tree root
                }
                Store::Native => {}
            }
        }
    }
    roots
}

/// The per-app home root(s) for one sandbox store — empty for `Native` (its home is `$HOME`,
/// added once by the callers) and for a store whose app id/name this browser doesn't have.
fn sandbox_roots(store: Store, home: &Path, flatpak_id: &str, snap_name: &str) -> Vec<PathBuf> {
    match store {
        Store::Native => Vec::new(),
        Store::Flatpak if !flatpak_id.is_empty() => vec![home.join(".var/app").join(flatpak_id)],
        Store::Snap if !snap_name.is_empty() => {
            ["common", "current"].iter().map(|tree| home.join("snap").join(snap_name).join(tree)).collect()
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nix_and_homebrew_declare_roots_beyond_path() {
        let has_roots = |cmd: &str| MANAGERS.iter().find(|m| m.command == cmd).unwrap().bin_roots;
        assert!(has_roots("nix").contains(&"~/.nix-profile/bin"));
        assert!(!has_roots("brew").is_empty());
        assert!(has_roots("apt").is_empty(), "PATH-only managers declare no extra roots");
    }

    #[test]
    fn native_binary_is_found_in_a_managers_extra_root_without_path() {
        let home = std::env::temp_dir().join(format!("bashrs_pm_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".nix-profile/bin")).unwrap();
        std::fs::write(home.join(".nix-profile/bin/firefox"), "").unwrap();
        assert!(native_binary_present(&home, "firefox"));
        assert!(!native_binary_present(&home, "no_such_browser_xyz"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn sandbox_roots_are_produced_only_for_apps_that_have_them() {
        let home = Path::new("/home/u");
        // Flatpak home for a known id; snap has both trees; native adds nothing here.
        assert_eq!(
            sandbox_roots(Store::Flatpak, home, "org.mozilla.firefox", ""),
            [PathBuf::from("/home/u/.var/app/org.mozilla.firefox")]
        );
        assert_eq!(
            sandbox_roots(Store::Snap, home, "", "firefox"),
            [PathBuf::from("/home/u/snap/firefox/common"), PathBuf::from("/home/u/snap/firefox/current")]
        );
        assert!(sandbox_roots(Store::Flatpak, home, "", "").is_empty(), "no id → no root");
        assert!(sandbox_roots(Store::Native, home, "x", "y").is_empty());
    }

    #[test]
    fn config_roots_cover_native_and_each_sandboxs_quirks() {
        let home = Path::new("/home/u");
        let roots = app_config_roots(home, "org.chromium.Chromium", "chromium");
        assert!(roots.contains(&PathBuf::from("/home/u/.config")), "native XDG config");
        assert!(
            roots.contains(&PathBuf::from("/home/u/.var/app/org.chromium.Chromium/config")),
            "flatpak's dotless config dir"
        );
        assert!(
            roots.contains(&PathBuf::from("/home/u/snap/chromium/common/.config"))
                && roots.contains(&PathBuf::from("/home/u/snap/chromium/common")),
            "snap: both the .config subdir and the tree root"
        );
    }

    #[test]
    fn home_roots_lead_with_the_native_home() {
        let home = Path::new("/home/u");
        let roots = app_home_roots(home, "org.mozilla.firefox", "firefox");
        assert_eq!(roots[0], home, "native home first");
        assert!(roots.contains(&PathBuf::from("/home/u/.var/app/org.mozilla.firefox")));
        assert!(roots.contains(&PathBuf::from("/home/u/snap/firefox/common")));
    }
}
