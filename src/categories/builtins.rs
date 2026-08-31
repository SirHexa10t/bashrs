//! Overrides of shell builtins — same name, the builtin's behavior, plus what you wanted it to
//! do anyway.
//!
//! Everything here is inherently a `#[shell_body]` command: an override exists to act on the
//! *calling* shell (a child process can't `cd` its parent), so each entry is a shell function
//! carrying the builtin's exact name (`#[unprefixed]`), reaching the real thing through the
//! `builtin` keyword — which is also every override's escape hatch: `builtin cd` is always the
//! plain one. Anything beyond a one-liner belongs in the binary, piped or called, not in the
//! body (see `shell_def` for the pattern).
//!
//! Related but deliberately elsewhere: `..` (filesystem) is a shortcut *named* like nothing the
//! shell owns, not an override of something it does.

#[bashrs_macros::category(command = BuiltinsCommand, prefix = "overwritten_")]
mod commands {

    /// `cd`, then print what you landed in: `ls -AF`, directories first, colours on
    ///
    /// The follow-up runs only when the `cd` itself succeeded, and `builtin cd "$@"` keeps every
    /// native behavior — `cd -`, `-P`/`-L`, `CDPATH` — exactly as it was. Problematic filenames
    /// arrive escaped by GNU ls's own `--quoting-style=shell-escape`: the same paste-back-safe
    /// discipline `lll` implements through `support::shell_quote` (bare when safe, quoted
    /// otherwise, control characters as `$'…'`; GNU spells the odd case differently — `"it's"`
    /// where `lll` would write `$'it\'s'` — but every spelling reads back as the exact name),
    /// with the `-F` type markers and colour spans kept outside the quotes.
    #[unprefixed]
    #[shell_body(r#"builtin cd "$@" && ls -AF --color=always --group-directories-first --quoting-style=shell-escape"#)]
    pub fn cd() {}
}
