//! Rendering a string as a shell word a person can read and paste back — the quoting shared by
//! the two places bashrs shows raw strings to a human: `lll`'s file-name column
//! ([`crate::categories::filesystem`]) and `shell_args_print`
//! ([`crate::categories::shell`]). Both must turn an arbitrary string into something bash reads
//! back as the exact bytes; only the framing differs, and that framing stays with each caller —
//! `lll` preserves `ls`'s colour spans and `-F` type markers and lives in a two-space-delimited
//! table, `shell_args_print` always quotes and has neither. The three of those are why they can't
//! be one function (stripping a trailing `@` as a type marker would corrupt an argument that ends
//! in one), so what lives here is the shared *core*, not a merged command.
//!
//! Three spellings, in ascending capability — the choice of which is [`is_bare_safe`] /
//! [`needs_ansi_c`]; the rendering is [`push_ansi_c`] (and [`quote`] for the common all-in-one):
//! - **bare** — only [`BARE_SAFE`] characters, which bash reads back literally; safe to print
//!   untouched (a policy the caller opts into — `lll` does, an always-quoting list does not).
//! - **`'…'`** — literal throughout: spaces, `$`, `*`, `"`, `\` all pass through. The common form.
//! - **`$'…'`** (ANSI-C) — the only form that can hold a control character (a newline shows as
//!   `\n`, never an actual line break) or a single quote (which `'…'` cannot contain at all).

/// Characters a word may hold and still be read back literally by bash — no shell metacharacter,
/// no space, no control byte. Anything outside this set means the word has to be quoted to be
/// unambiguous.
const BARE_SAFE: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-+,:@%";

/// Whether `plain` can be printed with no quoting at all: non-empty and every character in
/// [`BARE_SAFE`]. (Empty is *not* bare-safe — it would vanish; it needs `''` to be seen.)
pub(crate) fn is_bare_safe(plain: &str) -> bool {
    !plain.is_empty() && plain.chars().all(|ch| BARE_SAFE.contains(ch))
}

/// Whether `plain` holds something `'…'` cannot express — a control character, or a single quote
/// — and so must be spelled with `$'…'` instead. (A caller with its own layout reasons to force
/// `$'…'`, like `lll`'s two-space column divider, ORs that in on top of this.)
pub(crate) fn needs_ansi_c(plain: &str) -> bool {
    plain.chars().any(char::is_control) || plain.contains('\'')
}

/// Append `ch` to `out` as it must appear *inside* a `$'…'` (ANSI-C) literal. `escape_spaces`
/// turns a space into `\x20` — for a caller whose layout can't tolerate a literal space (`lll`'s
/// two-space column divider would cut the name); a caller quoting for its own sake leaves spaces
/// alone, since inside the quotes they're already unambiguous.
///
/// Exposed as a per-character step, not a whole-string pass, because `lll` must apply it *around*
/// `ls`'s colour escapes (mapping only outside them) while an ordinary caller applies it to every
/// character — same rule, different iteration.
pub(crate) fn push_ansi_c(ch: char, escape_spaces: bool, out: &mut String) {
    match ch {
        '\n' => out.push_str("\\n"),
        '\t' => out.push_str("\\t"),
        '\r' => out.push_str("\\r"),
        '\\' => out.push_str("\\\\"),
        '\'' => out.push_str("\\'"),
        ' ' if escape_spaces => out.push_str("\\x20"),
        // Every `char::is_control()` char is U+0000–U+001F or U+007F–U+009F, so two hex digits
        // always suffice; non-control Unicode passes through as itself, readable.
        other if other.is_control() => out.push_str(&format!("\\x{:02x}", other as u32)),
        other => out.push(other),
    }
}

/// Quote `s` as a single shell word, ALWAYS quoted (never bare) — for a map or list where uniform
/// quoting makes an empty string (`''`) and the edges of a spaced value impossible to miss. Spaces
/// stay literal inside the quotes (no `escape_spaces`): there's no column divider to protect, and
/// a visible space is the point. A caller that wants the bare-when-safe tier adds it with
/// [`is_bare_safe`]; this is the two-tier `'…'`/`$'…'` core.
pub(crate) fn quote(s: &str) -> String {
    if needs_ansi_c(s) {
        let mut inner = String::new();
        for ch in s.chars() {
            push_ansi_c(ch, false, &mut inner);
        }
        format!("$'{inner}'")
    } else {
        format!("'{s}'")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_safe_is_printable_words_only() {
        assert!(is_bare_safe("main.rs") && is_bare_safe("a-b_c.2") && is_bare_safe("x@y%z"));
        assert!(!is_bare_safe(""), "empty is not bare — it must show as ''");
        assert!(!is_bare_safe("a b"), "a space forces quoting");
        assert!(!is_bare_safe("$HOME") && !is_bare_safe("*.rs") && !is_bare_safe("a'b"));
    }

    #[test]
    fn ansi_c_is_needed_exactly_for_controls_and_single_quotes() {
        assert!(needs_ansi_c("a\nb") && needs_ansi_c("a\tb") && needs_ansi_c("it's"));
        assert!(!needs_ansi_c("a b") && !needs_ansi_c("$x") && !needs_ansi_c("plain"));
    }

    /// A plain value still gets quotes — the map is uniform, and an empty value (`''`) is only
    /// visible because of it.
    #[test]
    fn quote_always_wraps_and_keeps_spaces_visible() {
        assert_eq!(quote("hello"), "'hello'");
        assert_eq!(quote(""), "''", "an empty string must not vanish");
        assert_eq!(quote("a b"), "'a b'", "the space is inside the quotes, visible");
        assert_eq!(quote("a  b"), "'a  b'", "a run of spaces is kept, not collapsed");
        // The characters `'…'` swallows literally — the reason it's the common case.
        assert_eq!(quote("$HOME"), "'$HOME'");
        assert_eq!(quote("*.rs"), "'*.rs'");
        assert_eq!(quote(r#"a"b\c"#), r#"'a"b\c'"#);
    }

    /// A single quote can't live inside `'…'`, so such a value flips to `$'…'` — the case the
    /// naive wrapper gets wrong (`'it's'` is unbalanced).
    #[test]
    fn quote_switches_to_ansi_c_for_a_single_quote() {
        assert_eq!(quote("it's"), r"$'it\'s'");
        assert_eq!(quote("'"), r"$'\''");
        assert_eq!(quote("''"), r"$'\'\''", "the result is what bash reads back: '' → two quotes");
    }

    /// Control characters spell out, so a value stays one line — a raw newline would split it.
    #[test]
    fn quote_escapes_control_characters_visibly() {
        assert_eq!(quote("a\nb"), r"$'a\nb'", "newline shows as \\n, not a line break");
        assert_eq!(quote("a\tb"), r"$'a\tb'");
        assert_eq!(quote("a\rb"), r"$'a\rb'");
        assert_eq!(quote("\x07"), r"$'\x07'", "a bell has no friendly name → \\xNN");
        assert_eq!(quote("a\\nb"), r"'a\nb'", "a LITERAL backslash-n stays in plain quotes");
        assert_eq!(quote("tab\there\\"), r"$'tab\there\\'", "a backslash in $'…' must double");
        assert_eq!(quote("café"), "'café'", "non-control Unicode is itself, never escaped");
    }

    /// `push_ansi_c` honours `escape_spaces` — the one axis on which `lll` (which must, to protect
    /// its column divider) and an ordinary caller differ.
    #[test]
    fn push_ansi_c_escapes_spaces_only_when_asked() {
        let mut kept = String::new();
        push_ansi_c(' ', false, &mut kept);
        assert_eq!(kept, " ", "left literal when the caller has no layout to protect");
        let mut escaped = String::new();
        push_ansi_c(' ', true, &mut escaped);
        assert_eq!(escaped, "\\x20", "escaped when a space would break the layout");
    }
}
