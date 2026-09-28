//! bashrs's own configuration domain — the pieces that define the user's bashrs environment
//! under `~/.bashrs/` (whose root path this module owns: [`bashrs_home`]): the sourcefile's
//! shell settings (environment/prompt exports, keybinds, the load greeting), assembled by
//! [`crate::cli`], and the user's hand-edited configuration file ([`config_file`] —
//! `configrs.toml`, opened by `brs_configure`/ALT+W and read wherever behavior is tunable).
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
/// trigger it. Kept trivial in shell (`[ "$?" -eq N ]`) to parse on any bash.
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

/// `~/.bashrs` — the root of everything bashrs keeps on disk (binary, sourcefile, configrs.toml,
/// bundled tools, companion clones). The one place the directory name is spelled in Rust; the
/// shell-side spellings (`"$HOME/.bashrs/…"`, expanded at use time) stay literal by design.
pub(crate) fn bashrs_home() -> PathBuf {
    home().join(".bashrs")
}

/// `~/.bashrs/user-data` — persistent state bashrs accumulates on the user's behalf, as opposed
/// to the binary/config/tooling it installs. Currently the imported browser cookies for `dl`.
pub(crate) fn user_data_dir() -> PathBuf {
    bashrs_home().join("user-data")
}

/// `~/.bashrs/sourcefile.sh` — the generated shell file. Derived here so its writer
/// ([`install::install_shell`]) and its reader (`brs_sourcefile`) can't drift apart; the
/// unexpanded `$HOME/…` form the rc files are wired with is [`install`]'s own concern.
pub(crate) fn sourcefile() -> PathBuf {
    bashrs_home().join("sourcefile.sh")
}

/// `~/.bashrs/stainless_comfy` — where the companion repos are cloned
/// ([`crate::drivers::stainless`]). The Rust-side spelling lives here with the other
/// `~/.bashrs/<subdir>` names; the generated aliases keep their own `$HOME/…` literal, expanded
/// by the shell at use time.
pub(crate) fn clones_dir() -> PathBuf {
    bashrs_home().join("stainless_comfy")
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
    #[test]
    fn bashrs_home_is_the_dot_dir_under_home() {
        assert!(super::bashrs_home().ends_with(".bashrs"));
    }

    #[test]
    fn the_named_paths_all_hang_off_that_one_root() {
        // Every `~/.bashrs/<subdir>` name is spelled here exactly once, so a rename of the root —
        // or of any subdir — is a one-line change rather than a hunt across layers.
        let home = super::bashrs_home();
        assert_eq!(super::sourcefile(), home.join("sourcefile.sh"));
        assert_eq!(super::user_data_dir(), home.join("user-data"));
        assert_eq!(super::clones_dir(), home.join("stainless_comfy"));
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
