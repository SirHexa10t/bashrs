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
//!
//! Fish reads quotes differently enough to need its own spelling, `FishWord` (and `quote_fish`
//! for the all-in-one): `$'…'` is not fish syntax at all, and fish's `'…'` treats `\\` and `\'`
//! as escapes where bash's takes them literally. `lll` and `shell_args_print` spell for whichever
//! of the two shells they are run from; the fish sourcefile's generator writes fish's.

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

/// Quote `s` as a single fish word, always quoted — fish's counterpart of [`quote`]: a whole
/// [`FishWord`].
pub(crate) fn quote_fish(s: &str) -> String {
    let mut word = FishWord::default();
    let mut out = String::new();
    for ch in s.chars() {
        word.push(ch, false, &mut out);
    }
    word.finish(&mut out);
    out
}

/// One fish word, spelled a character at a time — [`push_ansi_c`]'s counterpart, so that `lll` can
/// spell around `ls`'s colour codes, which it hands to `out` itself. Inside fish's single quotes
/// everything is literal except two escapes, `\\` and `\'`, so those are the only characters spelled
/// differently there. What fish's quotes cannot hold — a control character, or a space where the
/// caller's layout cannot have one (`escape_spaces`) — is written between quoted runs as fish's own
/// unquoted escape (`\n`, `\x20`, or `\uNNNN` for a control character past ASCII, where `\xNN` would
/// mean a raw byte): fish joins adjacent pieces into one word, so `a`, newline, `b` reads back from
/// `'a'\n'b'` unchanged. A quote opens only before a character it holds, so escapes in a row share
/// the gap between two runs.
#[derive(Default)]
pub(crate) struct FishWord {
    /// A quote is open on `out`.
    open: bool,
    /// Something has been written: an empty word still needs its `''` to be seen.
    started: bool,
}

impl FishWord {
    pub(crate) fn push(&mut self, ch: char, escape_spaces: bool, out: &mut String) {
        self.started = true;
        let escape = match ch {
            '\n' => Some(r"\n".to_string()),
            '\t' => Some(r"\t".to_string()),
            '\r' => Some(r"\r".to_string()),
            ' ' if escape_spaces => Some(r"\x20".to_string()),
            other if other.is_control() && other.is_ascii() => Some(format!(r"\x{:02x}", other as u32)),
            other if other.is_control() => Some(format!(r"\u{:04x}", other as u32)),
            _ => None,
        };
        if let Some(escape) = escape {
            if std::mem::take(&mut self.open) {
                out.push('\'');
            }
            out.push_str(&escape);
            return;
        }
        if !std::mem::replace(&mut self.open, true) {
            out.push('\'');
        }
        match ch {
            '\\' => out.push_str(r"\\"),
            '\'' => out.push_str(r"\'"),
            other => out.push(other),
        }
    }

    /// Close the word: the open quote, or — for a word with nothing in it — the `''` that shows it.
    pub(crate) fn finish(self, out: &mut String) {
        match (self.started, self.open) {
            (false, _) => out.push_str("''"),
            (true, true) => out.push('\''),
            (true, false) => {}
        }
    }
}

/// `s` as one fish word, as the generated fish file writes a literal: bare when it is made only of
/// letters, digits, `_`, `-` and `/` — none of them special to fish; the very words `fish_indent`
/// strips the quotes from, which is where the rule comes from and how a test checks it — and
/// [`quote_fish`]ed otherwise. Quoting a word does not change what a command receives, so the two
/// spellings mean the same; the bare one reads more easily.
pub(crate) fn fish_word(s: &str) -> String {
    let plain = !s.is_empty() && s.chars().all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '-' | '/'));
    if plain { s.to_string() } else { quote_fish(s) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule `fish_word` follows is `fish_indent`'s, so `fish_indent` is asked: for every
    /// sample, the spelling `fish_word` chose comes back from `fish_indent` untouched.
    #[test]
    fn fish_words_are_spelled_as_fish_indent_spells_them() {
        let samples = [
            "plain", "a/b", "/abs/path", "a-b", "a_b", "-a", "a1", "é", "a.b", "a+b", "a,b", "a@b",
            "a%b", "a:b", "a=b", "a~b", "~a", "#a", "a!b", "a?b", "a*b", "a[b", "a{b", "a$b", "a(b",
            "a b", "it's", "", r"a\b",
        ];
        let script: String = samples.iter().map(|sample| format!("echo {}\n", fish_word(sample))).collect();
        let mut child = std::process::Command::new("fish_indent")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("fish_indent (fish's formatter) is needed to check fish spelling");
        use std::io::Write as _;
        child.stdin.take().expect("piped").write_all(script.as_bytes()).unwrap();
        let indented = child.wait_with_output().unwrap();
        assert_eq!(String::from_utf8_lossy(&indented.stdout), script, "fish_indent respelled a word");
        assert_eq!(fish_word("plain"), "plain");
        assert_eq!(fish_word("a.b"), "'a.b'", "quoted where fish_indent keeps quotes");
    }

    /// The two escapes fish's quotes know are the two characters spelled differently; everything
    /// else bash's `'…'` would leave alone, fish's leaves alone too.
    #[test]
    fn fish_quoting_escapes_only_what_fish_quotes_interpret() {
        assert_eq!(quote_fish("hello"), "'hello'");
        assert_eq!(quote_fish(""), "''", "an empty string must not vanish");
        assert_eq!(quote_fish("it's"), r"'it\'s'", "no `$'…'` — fish has no such syntax");
        assert_eq!(quote_fish(r"a\b"), r"'a\\b'", "a backslash doubles: `\\` IS an escape in fish quotes");
        assert_eq!(quote_fish(r"ends with\"), r"'ends with\\'", "or it would escape the closing quote");
        assert_eq!(quote_fish("$HOME *.rs {a,b} ~ (cmd)"), "'$HOME *.rs {a,b} ~ (cmd)'", "all literal in quotes");
    }

    /// A control character leaves the quotes, so a value stays on one line and still reads back
    /// as one word.
    #[test]
    fn fish_quoting_writes_control_characters_as_unquoted_escapes() {
        assert_eq!(quote_fish("a\nb"), r"'a'\n'b'");
        assert_eq!(quote_fish("a\tb"), r"'a'\t'b'");
        assert_eq!(quote_fish("\x07"), r"\x07", "a bell has no friendly name → \\xNN, and no empty quotes around it");
        assert_eq!(quote_fish("\u{85}"), r"\u0085", "past ASCII \\xNN would be a raw byte");
        assert_eq!(quote_fish("a\n\nb\n"), r"'a'\n\n'b'\n", "escapes in a row share one gap");
        assert_eq!(quote_fish("café"), "'café'", "non-control Unicode is itself, never escaped");
    }

    /// The one axis on which `lll` differs: its two-space column divider would cut a name with a
    /// run of spaces, so its spaces leave the quotes too.
    #[test]
    fn a_fish_word_escapes_spaces_only_when_asked() {
        let spell = |s: &str, escape_spaces| {
            let mut word = FishWord::default();
            let mut out = String::new();
            s.chars().for_each(|ch| word.push(ch, escape_spaces, &mut out));
            word.finish(&mut out);
            out
        };
        assert_eq!(spell("two  spaces", false), "'two  spaces'");
        assert_eq!(spell("two  spaces", true), r"'two'\x20\x20'spaces'");
    }

    /// The claim that matters, checked by fish itself rather than by these spellings: every
    /// quoted word reads back as exactly the string it came from.
    #[test]
    fn fish_quoted_words_read_back_unchanged_in_fish() {
        let hostile = [
            "plain", "", "a b", "it's", r"a\b", r"a\\b", r"ends with\", "'", r"\'", "$HOME", "*.rs",
            "{a,b}", "~/x", "(echo no)", "a\nb", "tab\there", "\x07bell", "\u{85}next line", "café", "\n",
            "a\n\nb\n", "~", "#not a comment",
        ];
        for original in hostile {
            let out = std::process::Command::new("fish")
                .args(["--no-config", "-c", &format!("printf '%s' {}", quote_fish(original))])
                .output()
                .expect("fish is needed to check fish quoting");
            assert!(out.status.success(), "fish rejected the quoting of {original:?}: {}", String::from_utf8_lossy(&out.stderr));
            assert_eq!(String::from_utf8_lossy(&out.stdout), original, "{original:?} came back changed");
        }
    }

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
