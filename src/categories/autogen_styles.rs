//! The generated stylized-echo command matrix (`StylizedEchoCommand`) — bare, memorable verbs
//! (`recho`, `becho`, …), never `style_`-prefixed. Each names its style with a typed [`BasicLook`]
//! ([`crate::support::theme`]), printed via the styling engine in [`crate::support::doc_style`].
//!
//! The whole matrix between the `GENERATED-STYLE-MATRIX` markers is generated — `build.rs` rewrites
//! it from `theme.rs` during the build (when either changes), so this file is never
//! edited by hand. Each shim forwards to [`_styled_echo`](crate::categories::styles::_styled_echo);
//! `EchoArgs` lives in [`crate::support::args`]. Hand-written style *commands* (e.g. `errcho`) live
//! in [`crate::categories::styles`].

#[bashrs_macros::category(command = StylizedEchoCommand, prefix = "style_")]
mod commands {
    use crate::categories::styles::_styled_echo;
    use crate::support::args::EchoArgs;
    use crate::support::generator_basis::BasicLook;
    use crate::support::theme::{Basic, Underline, Weight};

    // GENERATED-STYLE-MATRIX-START

    /// echo in bold
    #[unprefixed]
    #[alias("echobo")]
    pub fn boecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Bold, underline: Underline::None, colour: Basic::None }, &args); }

    /// echo in bold red
    #[unprefixed]
    #[alias("echor")]
    pub fn recho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Bold, underline: Underline::None, colour: Basic::Red }, &args); }

    /// echo in bold green
    #[unprefixed]
    #[alias("echog")]
    pub fn gecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Bold, underline: Underline::None, colour: Basic::Green }, &args); }

    /// echo in bold blue
    #[unprefixed]
    #[alias("echob")]
    pub fn becho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Bold, underline: Underline::None, colour: Basic::Blue }, &args); }

    /// echo in bold cyan
    #[unprefixed]
    #[alias("echoc")]
    pub fn cecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Bold, underline: Underline::None, colour: Basic::Cyan }, &args); }

    /// echo in bold yellow
    #[unprefixed]
    #[alias("echoy")]
    pub fn yecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Bold, underline: Underline::None, colour: Basic::Yellow }, &args); }

    /// echo in bold orange
    #[unprefixed]
    #[alias("echoor")]
    pub fn orecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Bold, underline: Underline::None, colour: Basic::Orange }, &args); }

    /// echo in bold white
    #[unprefixed]
    #[alias("echow")]
    pub fn wecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Bold, underline: Underline::None, colour: Basic::White }, &args); }

    /// echo in bold magenta
    #[unprefixed]
    #[alias("echom")]
    pub fn mecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Bold, underline: Underline::None, colour: Basic::Magenta }, &args); }

    /// echo in bold underlined
    #[unprefixed]
    #[alias("echou")]
    pub fn uecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Bold, underline: Underline::Underlined, colour: Basic::None }, &args); }

    /// echo in bold underlined red
    #[unprefixed]
    #[alias("echour")]
    pub fn urecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Bold, underline: Underline::Underlined, colour: Basic::Red }, &args); }

    /// echo in bold underlined green
    #[unprefixed]
    #[alias("echoug")]
    pub fn ugecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Bold, underline: Underline::Underlined, colour: Basic::Green }, &args); }

    /// echo in bold underlined blue
    #[unprefixed]
    #[alias("echoub")]
    pub fn ubecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Bold, underline: Underline::Underlined, colour: Basic::Blue }, &args); }

    /// echo in bold underlined cyan
    #[unprefixed]
    #[alias("echouc")]
    pub fn ucecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Bold, underline: Underline::Underlined, colour: Basic::Cyan }, &args); }

    /// echo in bold underlined yellow
    #[unprefixed]
    #[alias("echouy")]
    pub fn uyecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Bold, underline: Underline::Underlined, colour: Basic::Yellow }, &args); }

    /// echo in bold underlined orange
    #[unprefixed]
    #[alias("echouor")]
    pub fn uorecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Bold, underline: Underline::Underlined, colour: Basic::Orange }, &args); }

    /// echo in bold underlined white
    #[unprefixed]
    #[alias("echouw")]
    pub fn uwecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Bold, underline: Underline::Underlined, colour: Basic::White }, &args); }

    /// echo in bold underlined magenta
    #[unprefixed]
    #[alias("echoum")]
    pub fn umecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Bold, underline: Underline::Underlined, colour: Basic::Magenta }, &args); }

    /// echo in dark
    #[unprefixed]
    #[alias("echoda")]
    pub fn daecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Dark, underline: Underline::None, colour: Basic::None }, &args); }

    /// echo in dark red
    #[unprefixed]
    #[alias("echodar")]
    pub fn darecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Dark, underline: Underline::None, colour: Basic::Red }, &args); }

    /// echo in dark green
    #[unprefixed]
    #[alias("echodag")]
    pub fn dagecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Dark, underline: Underline::None, colour: Basic::Green }, &args); }

    /// echo in dark blue
    #[unprefixed]
    #[alias("echodab")]
    pub fn dabecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Dark, underline: Underline::None, colour: Basic::Blue }, &args); }

    /// echo in dark cyan
    #[unprefixed]
    #[alias("echodac")]
    pub fn dacecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Dark, underline: Underline::None, colour: Basic::Cyan }, &args); }

    /// echo in dark yellow
    #[unprefixed]
    #[alias("echoday")]
    pub fn dayecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Dark, underline: Underline::None, colour: Basic::Yellow }, &args); }

    /// echo in dark orange
    #[unprefixed]
    #[alias("echodaor")]
    pub fn daorecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Dark, underline: Underline::None, colour: Basic::Orange }, &args); }

    /// echo in dark white
    #[unprefixed]
    #[alias("echodaw")]
    pub fn dawecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Dark, underline: Underline::None, colour: Basic::White }, &args); }

    /// echo in dark magenta
    #[unprefixed]
    #[alias("echodam")]
    pub fn damecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Dark, underline: Underline::None, colour: Basic::Magenta }, &args); }

    /// echo in dark underlined
    #[unprefixed]
    #[alias("echodau")]
    pub fn dauecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Dark, underline: Underline::Underlined, colour: Basic::None }, &args); }

    /// echo in dark underlined red
    #[unprefixed]
    #[alias("echodaur")]
    pub fn daurecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Dark, underline: Underline::Underlined, colour: Basic::Red }, &args); }

    /// echo in dark underlined green
    #[unprefixed]
    #[alias("echodaug")]
    pub fn daugecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Dark, underline: Underline::Underlined, colour: Basic::Green }, &args); }

    /// echo in dark underlined blue
    #[unprefixed]
    #[alias("echodaub")]
    pub fn daubecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Dark, underline: Underline::Underlined, colour: Basic::Blue }, &args); }

    /// echo in dark underlined cyan
    #[unprefixed]
    #[alias("echodauc")]
    pub fn daucecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Dark, underline: Underline::Underlined, colour: Basic::Cyan }, &args); }

    /// echo in dark underlined yellow
    #[unprefixed]
    #[alias("echodauy")]
    pub fn dauyecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Dark, underline: Underline::Underlined, colour: Basic::Yellow }, &args); }

    /// echo in dark underlined orange
    #[unprefixed]
    #[alias("echodauor")]
    pub fn dauorecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Dark, underline: Underline::Underlined, colour: Basic::Orange }, &args); }

    /// echo in dark underlined white
    #[unprefixed]
    #[alias("echodauw")]
    pub fn dauwecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Dark, underline: Underline::Underlined, colour: Basic::White }, &args); }

    /// echo in dark underlined magenta
    #[unprefixed]
    #[alias("echodaum")]
    pub fn daumecho(args: EchoArgs) { _styled_echo(BasicLook { weight: Weight::Dark, underline: Underline::Underlined, colour: Basic::Magenta }, &args); }
    // GENERATED-STYLE-MATRIX-END
}
