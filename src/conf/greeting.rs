//! What a sourcefile prints when it is sourced in a terminal's first shell — a one-line load
//! confirmation, then, while an archived config waits to be merged, the warning saying so. The
//! generator writes both into each file, resolved — the words, the keys bound, the styling — so the
//! shell prints them itself and a shell start runs no process for them; whether to print them, the
//! file tells in the shell (`cli::sourcefile`'s greeting). Rejected: the binary printing them
//! (~0.9 ms at each terminal's start), and the styled-echo functions (`gecho`, `boecho`: a process
//! each).

use crate::conf::config_file;
use crate::conf::keybinds::{self, Chord};
use crate::support::doc_style::{_scoped, _wrap};
use crate::support::theme::{Basic, Weight};

/// The greeting, styled as `gecho` styles its words — bold green — with each key in bold alone, as
/// `boecho` writes it. The keys are read from the bindings, so the greeting can only name the keys
/// that are bound.
pub(crate) fn text() -> String {
    let bold = |text: &str| _scoped(&_wrap(&[&Weight::Bold]), text);
    let words = format!(
        "Loaded Bashrs! Press {} to view your functions! Press {} to edit configurations!",
        bold(&key_for("brs_sourcefile")),
        bold(&key_for("brs_configure"))
    );
    _scoped(&_wrap(&[&Weight::Bold, &Basic::Green]), &words)
}

/// The stale-config warning, in `errcho`'s bold red — printed on stderr, last, so that it is the
/// last thing before the prompt.
pub(crate) fn warning() -> String {
    _scoped(&_wrap(&[&Weight::Bold, &Basic::Red]), &config_file::stale_config_warning())
}

/// The chord bound to `command`, as the greeting spells keys: `ALT+H`. The command's own name where
/// no key runs it, which a test rules out.
fn key_for(command: &str) -> String {
    keybinds::bindings()
        .iter()
        .find(|(_, bound, _)| *bound == command)
        .map_or_else(|| command.to_string(), |(sequence, _, _)| Chord::parse(sequence).label().to_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The words, as a reader sees them: both keys named, as the bindings bind them.
    #[test]
    fn greets_with_the_keys_that_are_bound() {
        let shown = console::strip_ansi_codes(&text()).into_owned();
        assert_eq!(shown, "Loaded Bashrs! Press ALT+H to view your functions! Press ALT+W to edit configurations!");
        assert_eq!(key_for("brs_sourcefile"), "ALT+H");
        assert_eq!(key_for("no_such_command"), "no_such_command", "an unbound command keeps its name");
    }

    /// The styling `gecho` and `boecho` gave it in shell, byte for byte: bold green, each key in
    /// bold alone, the green restored after each.
    #[test]
    fn styles_the_greeting_as_gecho_and_boecho_did() {
        let green = "\x1b[0m\x1b[1;32m";
        let key = |label: &str| format!("\x1b[0m\x1b[1m{label}\x1b[0m");
        let expected = format!(
            "{green}Loaded Bashrs! Press {}{green_again} to view your functions! Press {}{green_again} to edit configurations!\x1b[0m",
            key("ALT+H"),
            key("ALT+W"),
            green_again = "\x1b[1;32m"
        );
        assert_eq!(text(), expected);
    }

    /// The warning is the config's own words, in bold red.
    #[test]
    fn warns_in_errcho_s_bold_red() {
        assert_eq!(warning(), format!("\x1b[0m\x1b[1;31m{}\x1b[0m", config_file::stale_config_warning()));
    }
}
