//! The stylized-echo engine and the generated `recho` command matrix
//! (`StylizedEchoCommand`). A nested color/style restores the enclosing one when it ends
//! (rather than clearing to the terminal default), and every span starts from a clean
//! slate so styles don't compound — see `_scoped`. The `recho` family are bare,
//! memorable verbs (never `style_`-prefixed).
//!
//! Each command names its style by a `[weight, underline, color]` triple; `_wrap`
//! resolves it against the vocabulary in [`crate::categories::style_vocab`].
//!
//! Only the region between the `GENERATED-STYLE-MATRIX` markers is generated — `build.rs`
//! rewrites it from `style_vocab.rs` during the build (when either changes), so it's never
//! edited by hand. The engine around it (`_wrap`/`_scoped`, `EchoArgs`, `RESET`) is
//! hand-written. Hand-written *commands* that build on this engine live in
//! [`crate::categories::styles`] (e.g. `errcho`), not here.

#[bashrs_macros::category(command = StylizedEchoCommand, prefix = "style_")]
mod commands {
    use clap::Args;
    use crate::categories::style_vocab::{COLORS, UNDERLINES, WEIGHTS};

    const RESET: &str = "\x1b[0m";

    // GENERATED-STYLE-MATRIX-START

    /// echo in bold
    #[unprefixed]
    #[alias("echobo")]
    pub fn boecho(args: EchoArgs) { _styled_echo(["bo", "", ""], &args); }

    /// echo in bold red
    #[unprefixed]
    #[alias("echor")]
    pub fn recho(args: EchoArgs) { _styled_echo(["bo", "", "r"], &args); }

    /// echo in bold green
    #[unprefixed]
    #[alias("echog")]
    pub fn gecho(args: EchoArgs) { _styled_echo(["bo", "", "g"], &args); }

    /// echo in bold blue
    #[unprefixed]
    #[alias("echob")]
    pub fn becho(args: EchoArgs) { _styled_echo(["bo", "", "b"], &args); }

    /// echo in bold cyan
    #[unprefixed]
    #[alias("echoc")]
    pub fn cecho(args: EchoArgs) { _styled_echo(["bo", "", "c"], &args); }

    /// echo in bold yellow
    #[unprefixed]
    #[alias("echoy")]
    pub fn yecho(args: EchoArgs) { _styled_echo(["bo", "", "y"], &args); }

    /// echo in bold orange
    #[unprefixed]
    #[alias("echoor")]
    pub fn orecho(args: EchoArgs) { _styled_echo(["bo", "", "or"], &args); }

    /// echo in bold white
    #[unprefixed]
    #[alias("echow")]
    pub fn wecho(args: EchoArgs) { _styled_echo(["bo", "", "w"], &args); }

    /// echo in bold underlined
    #[unprefixed]
    #[alias("echou")]
    pub fn uecho(args: EchoArgs) { _styled_echo(["bo", "u", ""], &args); }

    /// echo in bold underlined red
    #[unprefixed]
    #[alias("echour")]
    pub fn urecho(args: EchoArgs) { _styled_echo(["bo", "u", "r"], &args); }

    /// echo in bold underlined green
    #[unprefixed]
    #[alias("echoug")]
    pub fn ugecho(args: EchoArgs) { _styled_echo(["bo", "u", "g"], &args); }

    /// echo in bold underlined blue
    #[unprefixed]
    #[alias("echoub")]
    pub fn ubecho(args: EchoArgs) { _styled_echo(["bo", "u", "b"], &args); }

    /// echo in bold underlined cyan
    #[unprefixed]
    #[alias("echouc")]
    pub fn ucecho(args: EchoArgs) { _styled_echo(["bo", "u", "c"], &args); }

    /// echo in bold underlined yellow
    #[unprefixed]
    #[alias("echouy")]
    pub fn uyecho(args: EchoArgs) { _styled_echo(["bo", "u", "y"], &args); }

    /// echo in bold underlined orange
    #[unprefixed]
    #[alias("echouor")]
    pub fn uorecho(args: EchoArgs) { _styled_echo(["bo", "u", "or"], &args); }

    /// echo in bold underlined white
    #[unprefixed]
    #[alias("echouw")]
    pub fn uwecho(args: EchoArgs) { _styled_echo(["bo", "u", "w"], &args); }

    /// echo in dark
    #[unprefixed]
    #[alias("echoda")]
    pub fn daecho(args: EchoArgs) { _styled_echo(["da", "", ""], &args); }

    /// echo in dark red
    #[unprefixed]
    #[alias("echodar")]
    pub fn darecho(args: EchoArgs) { _styled_echo(["da", "", "r"], &args); }

    /// echo in dark green
    #[unprefixed]
    #[alias("echodag")]
    pub fn dagecho(args: EchoArgs) { _styled_echo(["da", "", "g"], &args); }

    /// echo in dark blue
    #[unprefixed]
    #[alias("echodab")]
    pub fn dabecho(args: EchoArgs) { _styled_echo(["da", "", "b"], &args); }

    /// echo in dark cyan
    #[unprefixed]
    #[alias("echodac")]
    pub fn dacecho(args: EchoArgs) { _styled_echo(["da", "", "c"], &args); }

    /// echo in dark yellow
    #[unprefixed]
    #[alias("echoday")]
    pub fn dayecho(args: EchoArgs) { _styled_echo(["da", "", "y"], &args); }

    /// echo in dark orange
    #[unprefixed]
    #[alias("echodaor")]
    pub fn daorecho(args: EchoArgs) { _styled_echo(["da", "", "or"], &args); }

    /// echo in dark white
    #[unprefixed]
    #[alias("echodaw")]
    pub fn dawecho(args: EchoArgs) { _styled_echo(["da", "", "w"], &args); }

    /// echo in dark underlined
    #[unprefixed]
    #[alias("echodau")]
    pub fn dauecho(args: EchoArgs) { _styled_echo(["da", "u", ""], &args); }

    /// echo in dark underlined red
    #[unprefixed]
    #[alias("echodaur")]
    pub fn daurecho(args: EchoArgs) { _styled_echo(["da", "u", "r"], &args); }

    /// echo in dark underlined green
    #[unprefixed]
    #[alias("echodaug")]
    pub fn daugecho(args: EchoArgs) { _styled_echo(["da", "u", "g"], &args); }

    /// echo in dark underlined blue
    #[unprefixed]
    #[alias("echodaub")]
    pub fn daubecho(args: EchoArgs) { _styled_echo(["da", "u", "b"], &args); }

    /// echo in dark underlined cyan
    #[unprefixed]
    #[alias("echodauc")]
    pub fn daucecho(args: EchoArgs) { _styled_echo(["da", "u", "c"], &args); }

    /// echo in dark underlined yellow
    #[unprefixed]
    #[alias("echodauy")]
    pub fn dauyecho(args: EchoArgs) { _styled_echo(["da", "u", "y"], &args); }

    /// echo in dark underlined orange
    #[unprefixed]
    #[alias("echodauor")]
    pub fn dauorecho(args: EchoArgs) { _styled_echo(["da", "u", "or"], &args); }

    /// echo in dark underlined white
    #[unprefixed]
    #[alias("echodauw")]
    pub fn dauwecho(args: EchoArgs) { _styled_echo(["da", "u", "w"], &args); }
    // GENERATED-STYLE-MATRIX-END

    /// Words to print, joined with spaces (like `echo`).
    #[derive(Args)]
    pub struct EchoArgs {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        pub words: Vec<String>,
    }

    /// The SGR sub-code mapped to `key` in `map` (a `&[(criterion, code, word)]`), or `""`.
    fn _lookup(map: &[(&'static str, &'static str, &'static str)], key: &str) -> &'static str {
        map.iter().find(|(k, _, _)| *k == key).map_or("", |(_, v, _)| *v)
    }

    /// Resolve a `[weight, underline, color]` triple to its full SGR escape by looking
    /// each criterion up in its map and joining the non-empty codes with `;`. e.g.
    /// `["bo", "u", "r"]` → `"\x1b[1;4;31m"`, `["bo", "", ""]` → `"\x1b[1m"`.
    pub(crate) fn _wrap(criteria: [&str; 3]) -> String {
        let [w, u, c] = criteria;
        let sgr = [_lookup(WEIGHTS, w), _lookup(UNDERLINES, u), _lookup(COLORS, c)]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(";");
        format!("\x1b[{sgr}m")
    }

    /// Print the words scoped in the style named by `criteria` (see `_wrap`, `_scoped`).
    fn _styled_echo(criteria: [&str; 3], args: &EchoArgs) {
        println!("{}", _scoped(&_wrap(criteria), &args.words.join(" ")));
    }

    /// Style `text` with `codes`, keeping nested styles scoped instead of compounded.
    ///
    /// Scopes are encoded in the stream itself, so nesting survives across the separate
    /// processes of `recho "$(gecho …)"`: a span *opens* with `RESET + codes` (the reset
    /// is a clean start; the codes set this style) and *closes* with a lone `RESET`. So we
    /// re-assert `codes` after each *closing* reset already in `text` (a nested span that
    /// ended), and leave *opening* resets (a reset immediately followed by an SGR) alone.
    /// Both are ordinary ANSI — harmless when not re-processed — so the output renders the
    /// same in a terminal, a pipe, a file, or another `recho`.
    pub(crate) fn _scoped(codes: &str, text: &str) -> String {
        let mut out = format!("{RESET}{codes}"); // open: clean start + this style
        let mut rest = text;
        while let Some(i) = rest.find(RESET) {
            let cut = i + RESET.len();
            out.push_str(&rest[..cut]); // text up to and including the reset
            rest = &rest[cut..];
            if !rest.starts_with("\x1b[") {
                out.push_str(codes); // a nested span closed here → restore my style
            }
        }
        out.push_str(rest);
        out.push_str(RESET); // close: lone reset
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        // ANSI shorthands for readable expectations.
        const RED: &str = "\x1b[1;31m";
        const GREEN: &str = "\x1b[1;32m";
        const BLUE: &str = "\x1b[1;34m";
        const ULINE: &str = "\x1b[1;4m";

        #[test]
        fn wrap_assembles_the_escape_from_the_criteria_maps() {
            assert_eq!(_wrap(["bo", "", "r"]), "\x1b[1;31m"); // bold red
            assert_eq!(_wrap(["bo", "", ""]), "\x1b[1m"); // bold only (boecho)
            assert_eq!(_wrap(["da", "u", "r"]), "\x1b[2;4;31m"); // dark underlined red
        }

        // --- `_scoped`: ported from the relevant cases of _draw_formatted.py -------

        #[test]
        fn plain_text_is_wrapped_and_reset() {
            // ~ python `one_word` / `no_tag`.
            assert_eq!(_scoped(RED, "hi"), format!("{RESET}{RED}hi{RESET}"));
        }

        #[test]
        fn empty_text_is_just_the_style_and_reset() {
            // ~ python `nothing`.
            assert_eq!(_scoped(RED, ""), format!("{RESET}{RED}{RESET}"));
        }

        #[test]
        fn a_closing_reset_restores_the_enclosing_style() {
            // ~ python `switching_styles_and_reset`: after a nested span ends, resume.
            let out = _scoped(RED, &format!("red {} y", _scoped(GREEN, "g")));
            assert_eq!(out, format!("{RESET}{RED}red {RESET}{GREEN}g{RESET}{RED} y{RESET}"));
        }

        #[test]
        fn a_nested_span_starts_clean() {
            // ~ python `switching_color` (clean start): a nested underline renders on the
            // default color, not the enclosing red.
            let out = _scoped(RED, &format!("r {}", _scoped(ULINE, "u")));
            assert!(out.contains(&format!("{RESET}{ULINE}u{RESET}")), "underline span not clean: {out}");
        }

        #[test]
        fn an_already_processed_reset_re_asserts_the_style() {
            // ~ python `previously_scanned`: a bare reset already in the text is treated as
            // a scope close, so the style resumes after it.
            let out = _scoped(RED, "a \x1b[0m b");
            assert_eq!(out, format!("{RESET}{RED}a {RESET}{RED} b{RESET}"));
        }

        #[test]
        fn deeply_nested_spans_unwind_to_each_enclosing_style() {
            // red › green › blue, each restoring its parent on close.
            let inner = _scoped(GREEN, &format!("b {} d", _scoped(BLUE, "c")));
            let out = _scoped(RED, &format!("a {inner} e"));
            assert_eq!(
                out,
                format!("{RESET}{RED}a {RESET}{GREEN}b {RESET}{BLUE}c{RESET}{GREEN} d{RESET}{RED} e{RESET}"),
            );
        }

        #[test]
        fn sibling_spans_each_restore_the_enclosing_style() {
            let out = _scoped(RED, &format!("{} mid {}", _scoped(GREEN, "g"), _scoped(BLUE, "b")));
            assert!(out.contains(&format!("{GREEN}g{RESET}")), "green sibling not scoped: {out}");
            assert!(out.contains(&format!("{BLUE}b{RESET}")), "blue sibling not scoped: {out}");
            assert!(out.contains(&format!("{RED} mid {RESET}")), "middle should be red: {out}");
        }
    }
}
