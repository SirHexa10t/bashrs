//! The stylized-echo command matrix (`StylizedEchoCommand`) — bare, memorable verbs (`recho`,
//! `becho`, …), never `style_`-prefixed. Each names its style by a `[weight, underline, color]`
//! triple, printed via the styling engine in [`crate::support::doc_style`].
//!
//! Only the region between the `GENERATED-STYLE-MATRIX` markers is generated — `build.rs` rewrites
//! it from `style_vocab.rs` during the build (when either changes), so it's never edited by hand.
//! `EchoArgs` and `_styled_echo` are hand-written. Hand-written style *commands* that build on the
//! engine live in [`crate::categories::styles`] (e.g. `errcho`).

#[bashrs_macros::category(command = StylizedEchoCommand, prefix = "style_")]
mod commands {
    use crate::support::doc_style::{_scoped, _wrap};
    use clap::Args;

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

    /// echo in bold magenta
    #[unprefixed]
    #[alias("echom")]
    pub fn mecho(args: EchoArgs) { _styled_echo(["bo", "", "m"], &args); }

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

    /// echo in bold underlined magenta
    #[unprefixed]
    #[alias("echoum")]
    pub fn umecho(args: EchoArgs) { _styled_echo(["bo", "u", "m"], &args); }

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

    /// echo in dark magenta
    #[unprefixed]
    #[alias("echodam")]
    pub fn damecho(args: EchoArgs) { _styled_echo(["da", "", "m"], &args); }

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

    /// echo in dark underlined magenta
    #[unprefixed]
    #[alias("echodaum")]
    pub fn daumecho(args: EchoArgs) { _styled_echo(["da", "u", "m"], &args); }
    // GENERATED-STYLE-MATRIX-END

    /// Words to print, joined with spaces (like `echo`).
    #[derive(Args)]
    pub struct EchoArgs {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        pub words: Vec<String>,
    }

    /// Print the words scoped in the style named by `criteria` (see [`crate::support::doc_style`]).
    fn _styled_echo(criteria: [&str; 3], args: &EchoArgs) {
        println!("{}", _scoped(&_wrap(criteria), &args.words.join(" ")));
    }
}
