//! The user's bashrs configuration — `~/.bashrs/configrs.toml`, hand-edited (ALT+W opens it via
//! `bashrs_configure`) and read wherever behavior is user-tunable. Every reader falls back to the
//! documented default when the file or its key is absent, so a fresh install needs no file at all;
//! [`ensure_current`] writes the commented [`TEMPLATE`] on first use — and when the settings
//! themselves have changed shape (the template carries keys the file lacks), it archives the old
//! file as `configrs.toml.<time>.old` and writes a fresh one, leaving the merge to the user
//! (nagged by [`stale_config_notice`] in the sourcefile until the `.old` is gone).

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::support::preferences;

/// The configuration file's name under `~/.bashrs` — the one place it's spelled.
pub(crate) const CONFIG_FILE: &str = "configrs.toml";

/// `~/.bashrs/configrs.toml`.
pub(crate) fn path() -> PathBuf {
    super::bashrs_home().join(CONFIG_FILE)
}

/// The starter file written on first use: every tunable at its default, explanation beside it.
/// A real TOML file beside this module (editable as TOML, testable as TOML), embedded at compile
/// time — the installed binary travels alone, so the template can't live on disk at runtime.
pub(crate) const TEMPLATE: &str = include_str!("configrs.toml");

/// Make the config file exist *and match the current settings shape*; returns its path.
/// Missing → written from [`TEMPLATE`]. Present but lacking keys the template has (bashrs added
/// or renamed settings) → archived as `configrs.toml.<time>.old` and rewritten fresh; merging the
/// old values back is the user's move. A file with *extra* keys is left alone — user additions are
/// harmless, and archiving them would loop on every compile.
pub fn ensure_current() -> std::io::Result<PathBuf> {
    let path = path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if !path.exists() {
        std::fs::write(&path, TEMPLATE)?;
        return Ok(path);
    }
    if needs_migration(&std::fs::read_to_string(&path)?) {
        let archived =
            path.with_file_name(format!("{CONFIG_FILE}.{}.old", preferences::datehour_stamp()));
        std::fs::rename(&path, &archived)?;
        std::fs::write(&path, TEMPLATE)?;
        eprintln!(
            "bashrs: {CONFIG_FILE} was outdated — archived to {} and rewritten; merge your settings back (ALT+W)",
            archived.display()
        );
    }
    Ok(path)
}

/// Whether `current` lacks settings the template carries (or doesn't parse at all — archiving a
/// broken file beats silently running on defaults forever).
fn needs_migration(current: &str) -> bool {
    let Ok(file) = toml::from_str::<toml::Value>(current) else { return true };
    let template: toml::Value = toml::from_str(TEMPLATE).expect("the template is valid TOML");
    !key_paths(&template).is_subset(&key_paths(&file))
}

/// Every `section.key` (and bare top-level key) in a parsed config — the file's settings shape.
fn key_paths(config: &toml::Value) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    if let Some(table) = config.as_table() {
        for (name, value) in table {
            match value.as_table() {
                Some(section) => keys.extend(section.keys().map(|key| format!("{name}.{key}"))),
                None => {
                    keys.insert(name.clone());
                }
            }
        }
    }
    keys
}

/// The sourcefile's stale-config nag: while an archived config (`configrs*.old` — the prefix also
/// matches archives from before the name gained `.toml.`) sits unmerged in `~/.bashrs`, say so in
/// red on every shell start. `find`-based on purpose — a bare glob errors noisily in zsh when
/// nothing matches — and with `-H`, since `~/.bashrs` may be a symlink (e.g. into a dotfiles repo)
/// that plain `find` would refuse to descend into. The trailing `|| true` matters: this is the
/// sourcefile's last line, and without it a clean start (no archive) would leave the whole source
/// with exit status 1.
pub(crate) fn stale_config_notice() -> String {
    let stem = CONFIG_FILE.split('.').next().unwrap_or(CONFIG_FILE);
    format!(
        "[ -n \"$(find -H \"$HOME/.bashrs\" -maxdepth 1 -name '{stem}*.old' -print -quit 2>/dev/null)\" ] && \
         errcho \"bashrs: an archived config (~/.bashrs/{stem}*.old) awaits merging into {CONFIG_FILE} (ALT+W); delete the .old file once done\" \
         || true\n"
    )
}

/// Bundle the programming languages (python + uv) even when the system provides them
/// (`[tools] always_bundle_languages`, default `true` — bashrs runs on these environments).
pub(crate) fn always_bundle_languages() -> bool {
    read().and_then(|config| flag(&config, "tools", "always_bundle_languages")).unwrap_or(true)
}

/// Bundle the utility programs (ffmpeg) even when the system provides them
/// (`[tools] always_bundle_utilities`, default `false` — bundle only what's missing).
pub(crate) fn always_bundle_utilities() -> bool {
    read().and_then(|config| flag(&config, "tools", "always_bundle_utilities")).unwrap_or(false)
}

/// The parsed config file. A missing file is the normal all-defaults case; a file that doesn't
/// parse is reported (once per reader) and treated the same, rather than silently half-applied.
fn read() -> Option<toml::Value> {
    let text = std::fs::read_to_string(path()).ok()?;
    match toml::from_str(&text) {
        Ok(config) => Some(config),
        Err(err) => {
            eprintln!("bashrs: {CONFIG_FILE} is not valid TOML ({err}); using the defaults");
            None
        }
    }
}

/// The boolean at `[section] key`, if present.
fn flag(config: &toml::Value, section: &str, key: &str) -> Option<bool> {
    config.get(section)?.get(key)?.as_bool()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_template_parses_and_carries_the_documented_defaults() {
        let config: toml::Value = toml::from_str(TEMPLATE).expect("template must stay valid TOML");
        assert_eq!(flag(&config, "tools", "always_bundle_languages"), Some(true));
        assert_eq!(flag(&config, "tools", "always_bundle_utilities"), Some(false));
    }

    #[test]
    fn absent_keys_and_sections_read_as_none() {
        let empty: toml::Value = toml::from_str("").unwrap();
        assert_eq!(flag(&empty, "tools", "always_bundle_languages"), None);
        let other: toml::Value = toml::from_str("[tools]\nunrelated = true").unwrap();
        assert_eq!(flag(&other, "tools", "always_bundle_languages"), None);
    }

    #[test]
    fn migration_triggers_on_missing_template_keys_only() {
        // The template itself, or the template plus user additions: current — left alone.
        assert!(!needs_migration(TEMPLATE));
        assert!(
            !needs_migration(&format!("{TEMPLATE}\nmy_custom_key = 1\n[my_section]\nx = 2\n")),
            "user additions must not archive-loop the file on every compile"
        );
        // A pre-rename file (old key shape), or a broken file: archived and rewritten.
        assert!(needs_migration("[tools]\nalways_bundle = false\n"));
        assert!(needs_migration("not [valid toml"));
    }

    #[test]
    fn key_paths_capture_the_settings_shape() {
        let value: toml::Value =
            toml::from_str("top = 1\n[tools]\na = true\nb = 2\n[other]\nc = 'x'\n").unwrap();
        let keys: Vec<String> = key_paths(&value).into_iter().collect();
        assert_eq!(keys, ["other.c", "tools.a", "tools.b", "top"]);
    }

    #[test]
    fn the_stale_notice_uses_symlink_following_find_and_errcho() {
        let notice = stale_config_notice();
        assert!(
            notice.contains("find -H \"$HOME/.bashrs\""),
            "glob-free (zsh-safe) and symlink-following (~/.bashrs may be a dotfiles symlink): {notice}"
        );
        // `configrs*.old`, not `configrs.toml.*.old`: archives from before the name carried
        // `.toml.` must keep nagging too.
        assert!(notice.contains("-name 'configrs*.old'"), "{notice}");
        assert!(notice.contains("errcho"), "{notice}");
        assert!(notice.trim_end().ends_with("|| true"), "the last sourcefile line must exit 0: {notice}");
    }
}
