//! Keybinds emitted as `bind` lines in the sourcefiles by the [`crate::cli`] generator: key →
//! shell-function mappings, all in [`bindings`] — ALT+L's desktop restart too, a command
//! (`proc_restart_desktop`) that tells which desktop runs when it is pressed, so the sourcefiles
//! bind it like any other key.
//!
//! The keys are written once, in readline's notation, and `Chord` reads that notation for
//! everything else: the keyboard label the comments show, and the key name fish's `bind` takes.
//! `fish_bind` is fish's counterpart of readline's macro — the bound command lands on the command
//! line and runs as if typed — through a helper the fish file defines for its keybinds and erases
//! after them (`fish_bind_helper`).

use crate::support::shell_dialect::Shell;

/// `(readline key sequence, shell function to run)` pairs.
///
/// `\en` is ALT+N (ESC-n). The function is run as if typed at the prompt, so the
/// generator appends the Enter (`\n`) that executes it.
pub fn bindings() -> &'static [(&'static str, &'static str)] {
    &[
        (r"\en", "shell_new"),         // ALT+N → start a fresh shell session
        (r"\e\C-n", "shell_bare"),     // CTRL+ALT+N → fresh session WITHOUT bashrs (one-shot; any new shell arms again)
        (r"\eh", "brs_sourcefile"), // ALT+H → run brs_sourcefile
        (r"\ew", "brs_configure"),  // ALT+W → the settings form
        (r"\eq", "brs_compile"),    // ALT+Q → run brs_compile
        (r"\el", "proc_restart_desktop"), // ALT+L → restart / reload the running desktop environment
        // Picking new chords, learned the hard way (each confirmed through a pty / `bind -p`):
        // CTRL+SHIFT+<key> is byte-identical to CTRL+<key> — a terminal cannot encode Shift
        // beside a control character — and CTRL+Q (0x11) is XON, eaten by the tty's flow
        // control before readline ever sees it. ALT+SHIFT+<key> arrives intact but collides
        // with the ALT+SHIFT keyboard-layout switch common on desktops. ALT+F/B/D/N/L are
        // readline's own (word motion and friends) — ALT+N and ALT+L given over to bashrs on
        // purpose; ALT+Z/W/H/Q/V were free.
    ]
}

/// A key chord, read from the readline notation [`bindings`] is written in: `\e` is Alt, `\C-` is
/// Ctrl (in either order), and an uppercase key letter means Shift was part of the chord.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Chord {
    ctrl: bool,
    alt: bool,
    shift: bool,
    /// The key itself, lowercase.
    key: char,
}

impl Chord {
    pub(crate) fn parse(sequence: &str) -> Self {
        let (mut ctrl, mut alt) = (false, false);
        let mut rest = sequence;
        loop {
            if let Some(after) = rest.strip_prefix(r"\e") {
                alt = true;
                rest = after;
            } else if let Some(after) = rest.strip_prefix(r"\C-") {
                ctrl = true;
                rest = after;
            } else {
                break;
            }
        }
        let key = rest.chars().next().unwrap_or('?');
        Chord { ctrl, alt, shift: key.is_ascii_uppercase(), key: key.to_ascii_lowercase() }
    }

    /// The chord as a keyboard labels it — `Alt+N`, `Ctrl+Alt+N`: modifiers in Ctrl, Alt, Shift
    /// order, the key in capitals.
    pub(crate) fn label(&self) -> String {
        let modifiers = [(self.ctrl, "Ctrl+"), (self.alt, "Alt+"), (self.shift, "Shift+")];
        let mut label: String = modifiers.iter().filter(|(held, _)| *held).map(|(_, name)| *name).collect();
        label.push(self.key.to_ascii_uppercase());
        label
    }

    /// The chord as fish 4's `bind` names it — `alt-n`, `ctrl-alt-n` — with Shift spelled the way
    /// fish lists it back: as the uppercase letter.
    pub(crate) fn fish_key(&self) -> String {
        let modifiers = [(self.ctrl, "ctrl-"), (self.alt, "alt-")];
        let mut name: String = modifiers.iter().filter(|(held, _)| *held).map(|(_, name)| *name).collect();
        name.push(if self.shift { self.key.to_ascii_uppercase() } else { self.key });
        name
    }
}

/// The helper the fish file binds its keys through, and erases once they are bound.
pub(crate) const FISH_BIND: &str = "__bashrs_bind";

/// Binds `sequence` to `command` in fish, through [`fish_bind_helper`].
pub(crate) fn fish_bind(sequence: &str, command: &str) -> String {
    format!("{FISH_BIND} {} {}", Chord::parse(sequence).fish_key(), Shell::Fish.quote(command))
}

/// The definition of [`FISH_BIND`]: a key bound to run its command as if typed — the command line
/// becomes the command and executes, so it lands in history as typing it would, the counterpart of
/// readline's macro, which types the command and Enter. The line is replaced rather than appended
/// to, so half-typed text never merges into the command. Bound in fish's default mode, which is
/// also vi's normal mode, and in vi's insert mode, so the keys work wherever someone types.
///
/// Fish binds three of these keys itself — Alt+H to a man page, Alt+W to `whatis`, Alt+L to listing
/// the current token — as preset bindings, which a user binding like these outranks.
pub(crate) fn fish_bind_helper() -> String {
    format!(
        "#   Each key runs its command as if typed — in fish's default mode, and in vi's insert mode too.\n\
         #   They replace fish's own bindings of the same keys (Alt+H man page, Alt+W whatis, Alt+L list token).\n\
         function {FISH_BIND} -a key cmd\n    \
         for mode in default insert\n        \
         bind -M $mode $key \"commandline -r \"(string escape -- $cmd)\"; commandline -f execute\"\n    \
         end\n\
         end\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chord spelling the comments use: modifiers in Ctrl, Alt, Shift order, key as the
    /// keyboard labels it, an uppercase letter in the sequence meaning Shift.
    #[test]
    fn hotkeys_read_as_a_keyboard_labels_them() {
        let label = |sequence| Chord::parse(sequence).label();
        assert_eq!(label(r"\en"), "Alt+N");
        assert_eq!(label(r"\e\C-n"), "Ctrl+Alt+N", "Ctrl before Alt");
        assert_eq!(label(r"\C-y"), "Ctrl+Y");
        assert_eq!(label(r"\C-\eY"), "Ctrl+Alt+Shift+Y", "Shift last, from the capital");
        assert_eq!(label(r"\eq"), "Alt+Q");
    }

    /// Fish 4 names keys this way and lists them back the same; readline's `\C-n` is the one
    /// spelling it rejects outright, which is why the bindings are translated rather than reused.
    #[test]
    fn hotkeys_read_as_fish_names_them() {
        let fish = |sequence| Chord::parse(sequence).fish_key();
        assert_eq!(fish(r"\en"), "alt-n");
        assert_eq!(fish(r"\e\C-n"), "ctrl-alt-n");
        assert_eq!(fish(r"\C-\eY"), "ctrl-alt-Y", "Shift as the uppercase letter");
        assert_eq!(fish(r"\el"), "alt-l");
    }

    /// A bare command is written bare, anything else as one quoted word — the helper escapes it
    /// again for the bound script, at bind time.
    #[test]
    fn a_fish_bind_hands_the_helper_its_key_and_command() {
        assert_eq!(fish_bind(r"\eq", "brs_compile"), "__bashrs_bind alt-q brs_compile");
        assert_eq!(fish_bind(r"\el", "cinnamon --replace & disown"), "__bashrs_bind alt-l 'cinnamon --replace & disown'");
        let helper = fish_bind_helper();
        assert!(helper.contains(&format!("function {FISH_BIND} -a key cmd\n")), "{helper}");
        assert!(helper.contains("for mode in default insert"), "vi's insert mode too: {helper}");
        assert!(helper.contains(r#"bind -M $mode $key "commandline -r "(string escape -- $cmd)"; commandline -f execute""#), "{helper}");
    }
}
