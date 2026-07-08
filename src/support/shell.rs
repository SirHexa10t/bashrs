//! In-process analogues of shell facilities. [`captured`] funnels a command's printed output into a
//! string rather than letting it reach the terminal — the analogue of command substitution (`$(…)`),
//! for commands that feed one command's output into another (unit tests also use it to assert on
//! printed output). [`stdout_color`] is the `--color=auto` policy: the one place that decides when
//! stdout gets colour.

use std::io::IsTerminal;

use termcolor::{Buffer, ColorChoice};

/// Stdout's colour mode under the `--color=auto` rule: coloured when stdout is a terminal, plain
/// when piped. Shared by every printer that colours matches (`g`'s, `gg`'s).
pub(crate) fn stdout_color() -> ColorChoice {
    if std::io::stdout().is_terminal() {
        ColorChoice::Always
    } else {
        ColorChoice::Never
    }
}

/// Run `write` against an in-memory colour sink and return everything it printed, as text. Colour is
/// stripped (`Buffer::no_color`) and non-UTF-8 output is rendered lossily, so the result reads as
/// plain text. Anything that prints through a `termcolor::WriteColor` sink can be captured this way.
// Only the unit tests call this so far; it's kept un-gated (not `#[cfg(test)]`) as the shared capture
// primitive for the output-composing commands still to come — drop the `allow` once one lands.
#[allow(dead_code)]
pub(crate) fn captured(write: impl FnOnce(&mut Buffer)) -> String {
    let mut buf = Buffer::no_color();
    write(&mut buf);
    String::from_utf8_lossy(buf.as_slice()).into_owned()
}
