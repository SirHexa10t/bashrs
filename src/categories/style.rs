//! Colored / styled `echo` commands. These are intentionally bare, memorable verbs
//! (`recho`, `gecho`, …) rather than `style_`-prefixed — they're meant to be typed
//! constantly and to read naturally inside scripts.

#[bashrs_macros::category(command = StyleCommand, prefix = "style_")]
mod commands {
    use clap::Args;

    // ANSI SGR codes (a slice of the usual palette): the styles and colors used below,
    // plus the reset that closes them. Add a const + a command to expose another.
    const RESET: &str = "\x1b[0m";
    const BOLD: &str = "\x1b[1m";
    const ULINE: &str = "\x1b[4m";
    const RED: &str = "\x1b[31m";
    const GREEN: &str = "\x1b[32m";
    const YELLOW: &str = "\x1b[33m";
    const BLUE: &str = "\x1b[34m";
    const CYAN: &str = "\x1b[36m";
    const GREY: &str = "\x1b[37m"; // "white"
    const ORANGE: &str = "\x1b[38;5;208m";

    /// echo in bold red
    #[unprefixed]
    pub fn recho(args: EchoArgs) { _emit(&[BOLD, RED], &args, false); }

    /// echo in bold green
    #[unprefixed]
    pub fn gecho(args: EchoArgs) { _emit(&[BOLD, GREEN], &args, false); }

    /// echo in bold blue
    #[unprefixed]
    pub fn becho(args: EchoArgs) { _emit(&[BOLD, BLUE], &args, false); }

    /// echo in bold cyan
    #[unprefixed]
    pub fn cecho(args: EchoArgs) { _emit(&[BOLD, CYAN], &args, false); }

    /// echo in bold yellow
    #[unprefixed]
    pub fn yecho(args: EchoArgs) { _emit(&[BOLD, YELLOW], &args, false); }

    /// echo in bold orange
    #[unprefixed]
    pub fn orecho(args: EchoArgs) { _emit(&[BOLD, ORANGE], &args, false); }

    /// echo in bold white
    #[unprefixed]
    pub fn wecho(args: EchoArgs) { _emit(&[BOLD, GREY], &args, false); }

    /// echo underlined
    #[unprefixed]
    pub fn uecho(args: EchoArgs) { _emit(&[BOLD, ULINE], &args, false); }

    /// echo in bold red, to stderr
    #[unprefixed]
    pub fn errcho(args: EchoArgs) { _emit(&[BOLD, RED], &args, true); }

    /// Words to print, joined with spaces (like `echo`).
    #[derive(Args)]
    pub struct EchoArgs {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        pub words: Vec<String>,
    }

    /// Wrap the text (words joined with spaces) in the given SGR codes, then a reset,
    /// and print it — to stderr when `err`, else stdout.
    fn _emit(codes: &[&str], args: &EchoArgs, err: bool) {
        let line = format!("{}{}{RESET}", codes.concat(), args.words.join(" "));
        if err {
            eprintln!("{line}");
        } else {
            println!("{line}");
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn wraps_text_in_codes_and_resets() {
            let args = EchoArgs { words: vec!["hello".into(), "world".into()] };
            // `_emit` prints; here we just re-derive the line it builds to check the shape.
            let line = format!("{}{}{RESET}", [BOLD, RED].concat(), args.words.join(" "));
            assert_eq!(line, "\x1b[1m\x1b[31mhello world\x1b[0m");
        }
    }
}
