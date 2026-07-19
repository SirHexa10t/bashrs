//! bashrs's own configuration domain — the pieces that define the user's bashrs environment
//! under `~/.bashrs/` (whose root path this module owns: [`bashrs_home`]): the sourcefile's
//! shell settings (environment/prompt exports, keybinds, the load greeting), assembled by
//! [`crate::cli`], and the user's hand-edited configuration file ([`config_file`] —
//! `configrs.toml`, opened by `bashrs_configure`/ALT+W and read wherever behavior is tunable).
//!
//! Sits below [`crate::tools`] in the crate layering (see [`crate`]'s diagram): tools reads the
//! config, so nothing here may import upward. Subsystem-derived shell lines (command wrappers,
//! tool PATH/aliases, the raw-shell session commands) are contributed by their owners instead.

pub mod config_file;
pub mod environment;
pub mod greeting;
pub mod install;
pub mod keybinds;

use std::path::PathBuf;

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

#[cfg(test)]
mod tests {
    #[test]
    fn bashrs_home_is_the_dot_dir_under_home() {
        assert!(super::bashrs_home().ends_with(".bashrs"));
    }
}
