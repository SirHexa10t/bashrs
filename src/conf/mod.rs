//! bashrs's own configuration domain — the pieces that define the user's bashrs environment
//! under `~/.bashrs/` (whose layout this module owns: [`bashrs_home`], and every [`Place`] in
//! it): the sourcefile's shell settings (environment/prompt exports, keybinds, the load
//! greeting), assembled by [`crate::cli`], and the user's hand-edited configuration file
//! ([`config_file`], opened by `brs_configure`/ALT+W and read wherever behavior is tunable).
//!
//! Sits below [`crate::tools`] in the crate layering (see [`crate`]'s diagram): tools reads the
//! config, so nothing here may import upward. Subsystem-derived shell lines (command wrappers,
//! tool PATH/aliases, the raw-shell session commands) are contributed by their owners instead.

pub mod config_file;
pub mod environment;
pub mod greeting;
pub mod install;
pub mod keybinds;

use std::path::{Path, PathBuf};

/// Exit code a command returns to ask its generated wrapper to run its
/// `#[after]` action (e.g. start a fresh shell). It's distinct from success (0)
/// and failure (non-zero), so clap's `--help`/`-h` — which exit 0 — never
/// trigger it. Kept trivial in shell (`[ "$code" -eq N ]`) to parse on any bash.
/// Part of the wrapper↔binary protocol, so it lives here in the sourcefile's
/// domain — used by both the [`crate::cli`] generator and the commands that
/// signal a reload.
pub const RELOAD_EXIT_CODE: i32 = 97;


/// The user's home directory (empty path if somehow unset — callers join onto it). The one
/// place bashrs reads `$HOME` in Rust.
pub(crate) fn home() -> PathBuf {
    #[allow(deprecated)] // std::env::home_dir is un-deprecated on current Rust; the project relies on it
    std::env::home_dir().unwrap_or_default()
}

// --- What bashrs keeps in ~/.bashrs --------------------------------------------------------

/// `~/.bashrs`'s name under `$HOME` — the one spelling of it; [`Place::location`] holds the rest.
const BASHRS_DIR: &str = ".bashrs";

/// `~/.bashrs` — the root of everything bashrs keeps on disk, each thing in it a [`Place`].
pub(crate) fn bashrs_home() -> PathBuf {
    home().join(BASHRS_DIR)
}

/// `~/.bashrs` as the generated shell spells it: `$HOME/.bashrs`, `$HOME` expanded each time the
/// line runs, so a file sourced under another `HOME` (`sudo_bashrs`) reaches that home's.
pub(crate) fn bashrs_home_in_shell() -> String {
    format!("$HOME/{BASHRS_DIR}")
}

/// `~/.bashrs` as a message shows it to a person.
pub(crate) fn bashrs_home_shown() -> String {
    format!("~/{BASHRS_DIR}")
}

/// Everything bashrs keeps in `~/.bashrs`, by what it is. Each name is spelled once, in
/// [`Place::location`], and every spelling of a place's path is made from it: on disk
/// ([`Place::path`], or [`Place::under`] another `~/.bashrs`, as the install's tests use), in the
/// generated shell ([`Place::in_shell`]) and in a message ([`Place::shown`]). Renaming one is a
/// one-line change — plus a row in `install`'s user-directory moves, so that an existing
/// install's copy follows it.
///
/// The user's files split by who chose them: [`UserAppdata`](Place::UserAppdata) holds what the
/// apps keep for the user, [`UserCustom`](Place::UserCustom) what the user writes and comes back
/// to. A new kind of user file goes in whichever of the two says who chose it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Place {
    /// The installed binary, which every generated function calls.
    Binary,
    /// The generated file bash and zsh source.
    Sourcefile,
    /// The generated file fish sources, through its conf.d hook.
    FishSourcefile,
    /// bashrs's completion files for the names fish completes too, written by the install.
    FishCompletions,
    /// The user's hand-edited configuration ([`config_file`]).
    Config,
    /// The bundled tools ([`crate::tools`]).
    Tools,
    /// Where the companion repos are cloned ([`crate::drivers::stainless`]).
    Clones,
    /// What the apps keep for the user: state bashrs accumulates on the user's behalf, as opposed
    /// to the binary, config and tooling it installs, and to what the user writes.
    UserAppdata,
    /// `dl`'s imported browser cookies, one directory per site.
    BrowserCookies,
    /// The vendor list `net_local` downloads.
    OuiCache,
    /// What the user writes and comes back to, as opposed to what the apps choose to keep.
    UserCustom,
    /// The sequencer binds profiles a bare `autokey_apply` applies; the install creates it.
    SequencerProfiles,
    /// The devmounts config a bare `symlink_devs` applies; the install creates it.
    DevmountsConfigs,
}

impl Place {
    /// Every place, for the checks that go over them all.
    #[cfg(test)]
    pub(crate) const ALL: &'static [Place] = &[
        Place::Binary,
        Place::Sourcefile,
        Place::FishSourcefile,
        Place::FishCompletions,
        Place::Config,
        Place::Tools,
        Place::Clones,
        Place::UserAppdata,
        Place::BrowserCookies,
        Place::OuiCache,
        Place::UserCustom,
        Place::SequencerProfiles,
        Place::DevmountsConfigs,
    ];

    /// Where it sits — right in `~/.bashrs` (`None`) or in another place — and its own name.
    const fn location(self) -> (Option<Place>, &'static str) {
        match self {
            Place::Binary => (None, "bashrs"),
            Place::Sourcefile => (None, "sourcefile.sh"),
            Place::FishSourcefile => (None, "sourcefile.fish"),
            Place::FishCompletions => (None, "fish_completions"),
            Place::Config => (None, "configrs.toml"),
            Place::Tools => (None, "tools"),
            Place::Clones => (None, "stainless_comfy"),
            Place::UserAppdata => (None, "user-appdata"),
            Place::BrowserCookies => (Some(Place::UserAppdata), "browser_cookies"),
            Place::OuiCache => (Some(Place::UserAppdata), "oui.tsv"),
            Place::UserCustom => (None, "user-custom"),
            Place::SequencerProfiles => (Some(Place::UserCustom), "sequencer"),
            Place::DevmountsConfigs => (Some(Place::UserCustom), "devmounts"),
        }
    }

    /// Its own name.
    pub(crate) const fn name(self) -> &'static str {
        self.location().1
    }

    /// Its path inside `~/.bashrs`.
    pub(crate) fn relative(self) -> PathBuf {
        match self.location() {
            (None, name) => PathBuf::from(name),
            (Some(parent), name) => parent.relative().join(name),
        }
    }

    /// Where it is on this machine.
    pub(crate) fn path(self) -> PathBuf {
        self.under(&bashrs_home())
    }

    /// Where it is in the `~/.bashrs` at `bashrs_home`.
    pub(crate) fn under(self, bashrs_home: &Path) -> PathBuf {
        bashrs_home.join(self.relative())
    }

    /// Its path as the generated shell spells it, under [`bashrs_home_in_shell`].
    pub(crate) fn in_shell(self) -> String {
        format!("{}/{}", bashrs_home_in_shell(), self.relative().display())
    }

    /// Its path as a message shows it, under [`bashrs_home_shown`].
    pub(crate) fn shown(self) -> String {
        format!("{}/{}", bashrs_home_shown(), self.relative().display())
    }
}

/// The path a REPLACEMENT of `path` should write to: the file it links to when `path` is a
/// symlink to an existing regular file (followed to the end — chains and relative targets
/// included), and `path` itself otherwise.
///
/// Files under `~/.bashrs` may be the user's own symlinks — the binary pointed at a build output,
/// the config at a dotfiles repo. A replacement that unlinks or renames the path would cut such a
/// link and leave a regular file behind; resolving first sends the new content where the link
/// points, and the link survives. A dangling link, a link to a directory, a regular file and a
/// missing path all come back unchanged, so the caller's own behavior stands for those.
///
/// Only needed where a write replaces the directory entry — an unlink-first (the running
/// binary), a rename, a write-temp-then-rename. `fs::write` truncates in place, which already
/// writes through a link. Never for `tools/`: its `bin/` links are bashrs's OWN, replaced as
/// links on every sync, and resolving one would write the new entry over the tool it points at.
pub(crate) fn replacement_target(path: &Path) -> PathBuf {
    let is_link = std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink());
    match is_link.then(|| std::fs::canonicalize(path)) {
        Some(Ok(target)) if target.is_file() => target,
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::Place;

    /// The one name pinned here: every install has its files under it, so renaming it would leave
    /// them all behind.
    #[test]
    fn bashrs_home_is_the_dot_dir_under_home() {
        assert!(super::bashrs_home().ends_with(".bashrs"));
    }

    /// [`Place::ALL`] holds every place, each once and in declaration order. A new place fails the
    /// match below until it's given its position — and then this, until it's in `ALL`.
    #[test]
    fn all_holds_every_place() {
        let position = |place: Place| match place {
            Place::Binary => 0,
            Place::Sourcefile => 1,
            Place::FishSourcefile => 2,
            Place::FishCompletions => 3,
            Place::Config => 4,
            Place::Tools => 5,
            Place::Clones => 6,
            Place::UserAppdata => 7,
            Place::BrowserCookies => 8,
            Place::OuiCache => 9,
            Place::UserCustom => 10,
            Place::SequencerProfiles => 11,
            Place::DevmountsConfigs => 12,
        };
        let positions: Vec<usize> = Place::ALL.iter().map(|&place| position(place)).collect();
        assert_eq!(positions, (0..13).collect::<Vec<_>>());
    }

    /// Each name is one path component that every shell reads as it stands, so that a spelling of
    /// a path built from them is the path, quoted or not; and no two places are one.
    #[test]
    fn every_place_has_a_plain_name_and_a_path_of_its_own() {
        let plain = |name: &str| {
            !name.is_empty()
                && name != "."
                && name != ".."
                && name.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        };
        assert!(plain(super::BASHRS_DIR));
        let mut paths = std::collections::HashSet::new();
        for &place in Place::ALL {
            assert!(plain(place.name()), "{place:?}: {:?}", place.name());
            assert!(paths.insert(place.relative()), "{place:?} shares its path");
        }
    }

    /// Every spelling of a place is of the same path: on disk under `~/.bashrs`, in the shell
    /// under `$HOME`'s, in a message under `~`'s.
    #[test]
    fn every_spelling_of_a_place_names_the_same_path() {
        let home = super::bashrs_home();
        for &place in Place::ALL {
            let relative = place.relative();
            assert_eq!(place.path(), home.join(&relative), "{place:?}");
            assert_eq!(place.under(Path::new("/elsewhere")), Path::new("/elsewhere").join(&relative));
            assert_eq!(place.in_shell(), format!("{}/{}", super::bashrs_home_in_shell(), relative.display()));
            assert_eq!(place.shown(), format!("{}/{}", super::bashrs_home_shown(), relative.display()));
        }
        assert!(super::bashrs_home_in_shell().starts_with("$HOME/") && super::bashrs_home_shown().starts_with("~/"));
    }

    /// The user's files sit by who chose them: what the user writes in `UserCustom`, what the apps
    /// keep for them in `UserAppdata`.
    #[test]
    fn the_users_files_sit_by_who_chose_them() {
        for written in [Place::SequencerProfiles, Place::DevmountsConfigs] {
            assert_eq!(written.location().0, Some(Place::UserCustom), "{written:?}");
        }
        for kept in [Place::BrowserCookies, Place::OuiCache] {
            assert_eq!(kept.location().0, Some(Place::UserAppdata), "{kept:?}");
        }
    }

    /// A replacement goes where a live link points — through chains and relative links alike —
    /// and anything that is not a link to an existing file is handed back for the caller's own
    /// behavior: a regular file, a missing path, a dangling link, a link to a directory.
    #[test]
    fn a_replacement_goes_through_a_live_link_and_nothing_else() {
        use std::os::unix::fs::symlink;
        let dir = std::env::temp_dir().join(format!("bashrs_replace_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("dotfiles")).unwrap();
        std::fs::write(dir.join("dotfiles/configrs.toml"), "x").unwrap();
        let real = dir.join("dotfiles/configrs.toml").canonicalize().unwrap();

        symlink(&real, dir.join("absolute")).unwrap();
        symlink("dotfiles/configrs.toml", dir.join("relative")).unwrap();
        symlink(dir.join("absolute"), dir.join("chained")).unwrap();
        for through in ["absolute", "relative", "chained"] {
            assert_eq!(super::replacement_target(&dir.join(through)), real, "{through}");
        }

        std::fs::write(dir.join("plain"), "x").unwrap();
        symlink(dir.join("no/such/file"), dir.join("dangling")).unwrap();
        symlink(dir.join("dotfiles"), dir.join("to_a_dir")).unwrap();
        for unchanged in ["plain", "missing", "dangling", "to_a_dir"] {
            let path = dir.join(unchanged);
            assert_eq!(super::replacement_target(&path), path, "{unchanged}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
