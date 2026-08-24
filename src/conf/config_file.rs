//! The user's bashrs configuration — `~/.bashrs/configrs.toml`, edited through
//! `bashrs_configure`'s form (ALT+W; `-e` opens the file itself) and read wherever behavior is
//! user-tunable. [`settings`] and [`with_values`] are what that form reads and writes: both work
//! on the file's LINES, because the comments beside each key are the only documentation it has
//! and a round-trip through `toml` would drop every one of them. Every reader falls back to the
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
/// A real TOML file under `templates/` beside this module (editable as TOML, testable as TOML),
/// embedded at compile time — the installed binary travels alone, so the template can't live on
/// disk at runtime.
pub(crate) const TEMPLATE: &str = include_str!("templates/configrs.toml");

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
            "bashrs: {CONFIG_FILE} was outdated — archived to {} and rewritten; merge your settings back (`bashrs_configure -e`)",
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

/// One tunable as the FILE presents it: where it lives (`section.key`), the comment lines written
/// above it, and its current value.
pub(crate) struct Setting {
    pub path: String,
    /// The comment block introducing it, `#` and indentation stripped — the explanation a form
    /// shows in place of the file the user is no longer reading.
    pub docs: Vec<String>,
    pub enabled: bool,
}

impl Setting {
    /// The `[section]` it sits under, and the bare key — the two halves of [`Setting::path`].
    /// A top-level key (no section) reports an empty section.
    pub fn split(&self) -> (&str, &str) {
        self.path.rsplit_once('.').unwrap_or(("", &self.path))
    }
}

/// Every boolean tunable in `text`, in file order, each with the comment block above it.
///
/// Read from the file's LINES rather than its parsed value, because the comments *are* the
/// documentation and `toml` discards them — a form built from the parsed table could show the
/// keys but not a word about what they do.
///
/// Non-boolean entries are skipped. Every setting is a flag today ([`the_template_parses_and_every_field_holds_a_legal_value`]
/// pins that), and skipping is the honest failure: a caller that renders these can say the file
/// holds more than it showed, where a guess at how to display a string would silently misreport it.
pub(crate) fn settings(text: &str) -> Vec<Setting> {
    let mut found = Vec::new();
    let mut section = String::new();
    let mut docs: Vec<String> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(comment) = trimmed.strip_prefix('#') {
            docs.push(comment.trim().to_string());
        } else if let Some(name) = section_header(trimmed) {
            section = name.to_string();
            docs.clear(); // the file's own header belongs to no setting
        } else if let Some((key, enabled)) = boolean_assignment(trimmed) {
            found.push(Setting { path: qualify(&section, key), docs: std::mem::take(&mut docs), enabled });
        } else {
            docs.clear(); // a blank line, or a setting we don't render, ends the block
        }
    }
    found
}

/// `text` with each `(path, value)` applied.
///
/// A line-level rewrite, deliberately: every comment, blank line, key ordering and any setting the
/// form never showed survives exactly as the user wrote it. Round-tripping through `toml` would
/// return a valid file with all of that thrown away — which for a file whose entire usability is
/// its comments would be a silent act of vandalism.
///
/// A path the file doesn't carry is ignored rather than appended: the template defines the
/// settings, not the caller.
pub(crate) fn with_values(text: &str, values: &[(String, bool)]) -> String {
    let mut section = String::new();
    let mut out = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(name) = section_header(trimmed) {
            section = name.to_string();
        } else if let Some((key, _)) = boolean_assignment(trimmed) {
            if let Some((_, wanted)) = values.iter().find(|(path, _)| *path == qualify(&section, key)) {
                let indent = &line[..line.len() - trimmed.len()];
                out.push_str(&format!("{indent}{key} = {wanted}\n"));
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    // `lines()` cannot tell a final newline from its absence; restore whichever the file had.
    if !text.ends_with('\n') {
        out.pop();
    }
    out
}

/// The name in a `[section]` line, if that's what this is.
fn section_header(trimmed: &str) -> Option<&str> {
    trimmed.strip_prefix('[')?.strip_suffix(']')
}

/// The key and value of a `key = true|false` line, if that's what this is.
fn boolean_assignment(trimmed: &str) -> Option<(&str, bool)> {
    let (key, value) = trimmed.split_once('=')?;
    Some((key.trim(), value.trim().parse().ok()?))
}

/// `section.key`, or a bare key when there's no section.
fn qualify(section: &str, key: &str) -> String {
    if section.is_empty() { key.to_string() } else { format!("{section}.{key}") }
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
    // The command name is set off with single quotes, NOT backticks: this whole string is a
    // double-quoted argument to `errcho`, and a backtick there is command substitution — bash
    // would RUN `bashrs_configure -e` (opening an editor) on every interactive shell start while
    // an archive lingered, which is the opposite of an unobtrusive nag. Single quotes are literal
    // inside double quotes, so they display and do nothing.
    format!(
        "[ -n \"$(find -H \"$HOME/.bashrs\" -maxdepth 1 -name '{stem}*.old' -print -quit 2>/dev/null)\" ] && \
         errcho \"bashrs: an archived config (~/.bashrs/{stem}*.old) awaits merging into {CONFIG_FILE} \
         (run 'bashrs_configure -e' to merge, then delete the .old file)\" \
         || true\n"
    )
}

/// Bundle the programming languages (python + uv) even when the system provides them
/// (`[tools] always_bundle_languages`, default `true` — bashrs runs on these environments).
pub(crate) fn always_bundle_languages() -> bool {
    read().and_then(|config| flag(&config, "tools", "always_bundle_languages")).unwrap_or(true)
}

/// Bundle the utility programs (ffmpeg, yt-dlp, deno) even when the system provides them
/// (`[tools] always_bundle_utilities`, default `true` — the project pins its own tool versions
/// rather than trusting whatever the system carries).
pub(crate) fn always_bundle_utilities() -> bool {
    read().and_then(|config| flag(&config, "tools", "always_bundle_utilities")).unwrap_or(true)
}

/// Install the optional extras — tools bashrs doesn't need for its own commands but can set up
/// for you (`[tools] install_extras`, default `false`). Opt-in on purpose: unlike the
/// `always_bundle_*` groups, nothing depends on these, so the safe default is to add nothing. When
/// on, each is still bundled only where the system lacks it. Currently just git-lfs.
pub(crate) fn install_extras() -> bool {
    read().and_then(|config| flag(&config, "tools", "install_extras")).unwrap_or(false)
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
    fn the_template_parses_and_every_field_holds_a_legal_value() {
        // The template's job is shape, not policy: every tunable must exist and carry one of its
        // legal values — all current fields are booleans, and `flag` yields `None` for a non-bool
        // — but the SPECIFIC defaults are the template author's business and may change freely.
        let config: toml::Value = toml::from_str(TEMPLATE).expect("template must stay valid TOML");
        for key in ["always_bundle_languages", "always_bundle_utilities", "install_extras"] {
            assert!(
                flag(&config, "tools", key).is_some(),
                "template must carry `[tools] {key}` with a true/false value"
            );
        }
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

    /// The form is built from these, so every template flag must arrive — carrying the comment
    /// block that explains it, since in a form that text is all the user gets.
    #[test]
    fn settings_reads_every_template_flag_with_its_explanation() {
        let found = settings(TEMPLATE);
        let paths: Vec<&str> = found.iter().map(|s| s.path.as_str()).collect();
        assert_eq!(
            paths,
            ["tools.always_bundle_languages", "tools.always_bundle_utilities", "tools.install_extras"]
        );
        for setting in &found {
            assert_eq!(setting.split().0, "tools");
            assert!(!setting.docs.is_empty(), "{} lost its explanation", setting.path);
            assert!(setting.docs.iter().all(|line| !line.starts_with('#')), "`#` is stripped");
        }
        // The file's own header belongs to no setting — it is cleared at the `[tools]` line.
        assert!(
            !found[0].docs.iter().any(|line| line.contains("edit by hand")),
            "the file header must not be attributed to the first flag: {:?}",
            found[0].docs
        );
    }

    /// The whole reason for a line-level rewrite: this file's usability IS its comments, and a
    /// round-trip through `toml` would return a valid file with every one of them gone.
    #[test]
    fn writing_values_changes_only_the_values() {
        // Flip each flag to its opposite, whatever its default — the template mixes true and
        // false defaults, so "flip and expect all off" would be wrong.
        let original = settings(TEMPLATE);
        let flipped: Vec<(String, bool)> =
            original.iter().map(|setting| (setting.path.clone(), !setting.enabled)).collect();
        let rewritten = with_values(TEMPLATE, &flipped);

        // Every comment and blank line survives, in order; only the value lines differ.
        let (before, after): (Vec<&str>, Vec<&str>) =
            (TEMPLATE.lines().collect(), rewritten.lines().collect());
        assert_eq!(before.len(), after.len(), "no line added or dropped");
        for (was, now) in before.iter().zip(&after) {
            if was.trim_start().starts_with('#') || was.trim().is_empty() || was.starts_with('[') {
                assert_eq!(was, now, "untouched lines must be byte-identical");
            }
        }
        // And every value really did move to its opposite, as the file reads them back.
        for (was, now) in original.iter().zip(settings(&rewritten)) {
            assert_eq!(now.enabled, !was.enabled, "{} did not flip", now.path);
        }
        assert!(!needs_migration(&rewritten), "the rewrite keeps the settings shape intact");
    }

    #[test]
    fn writing_leaves_alone_what_it_was_not_asked_about() {
        let text = "[tools]\nalways_bundle_languages = true\nalways_bundle_utilities = true\n";
        // A path the file doesn't carry is ignored, not appended.
        let ghost = with_values(text, &[("tools.invented".to_string(), true)]);
        assert_eq!(ghost, text);
        // A flag not named keeps its value while its neighbour changes.
        let one = with_values(text, &[("tools.always_bundle_languages".to_string(), false)]);
        assert_eq!(
            one,
            "[tools]\nalways_bundle_languages = false\nalways_bundle_utilities = true\n"
        );
    }

    /// Same key under two sections is two settings, and only the addressed one moves — the reason
    /// paths are qualified rather than bare keys.
    #[test]
    fn identically_named_keys_in_different_sections_stay_distinct() {
        let text = "[a]\nverbose = true\n[b]\nverbose = true\n";
        assert_eq!(
            settings(text).iter().map(|s| s.path.clone()).collect::<Vec<_>>(),
            ["a.verbose", "b.verbose"]
        );
        assert_eq!(
            with_values(text, &[("b.verbose".to_string(), false)]),
            "[a]\nverbose = true\n[b]\nverbose = false\n"
        );
    }

    /// Everything that isn't a flag is passed over, not mangled: the caller says the file holds
    /// more than it showed, and `-e` opens it. Indentation and a missing final newline survive too.
    #[test]
    fn non_boolean_entries_and_file_shape_are_preserved() {
        let text = "[tools]\n  spaced = true\nname = \"ada\"\ncount = 3\nlist = [1, 2]";
        let found = settings(text);
        assert_eq!(
            found.iter().map(|s| s.path.clone()).collect::<Vec<_>>(),
            ["tools.spaced"],
            "only the flag is offered"
        );
        let rewritten = with_values(text, &[("tools.spaced".to_string(), false)]);
        assert_eq!(rewritten, "[tools]\n  spaced = false\nname = \"ada\"\ncount = 3\nlist = [1, 2]");
        assert!(!rewritten.ends_with('\n'), "a file with no trailing newline keeps none");
        assert!(with_values("x = true\n", &[]).ends_with('\n'), "and one with a newline keeps it");
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

    /// The nag is a message, and a message must not DO anything. A backtick would make bash run
    /// whatever it wrapped (this string is a double-quoted `errcho` argument) on every interactive
    /// shell start — which is exactly how naming the fix as `` `bashrs_configure -e` `` once opened
    /// an editor in the user's face each session. It still has to name the command, so that stays.
    #[test]
    fn the_stale_notice_names_the_fix_without_executing_anything() {
        let notice = stale_config_notice();
        assert!(
            !notice.contains('`'),
            "a backtick in this errcho argument is command substitution, run every prompt: {notice}"
        );
        // No `$(` either, for the same reason — the only command substitution allowed is the
        // deliberate `$(find …)` guard, so any OTHER must be absent. (There's exactly one `$(`.)
        assert_eq!(notice.matches("$(").count(), 1, "only the find-guard may substitute: {notice}");
        assert!(notice.contains("bashrs_configure -e"), "it must still name the fix: {notice}");
    }
}
