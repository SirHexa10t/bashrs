//! Keybinds emitted as `bind` lines in the sourcefiles by the [`crate::cli`] generator: key →
//! shell-function mappings, all in [`bindings`] — ALT+L's desktop restart too, a command
//! (`proc_restart_desktop`) that tells which desktop runs when it is pressed, so the sourcefiles
//! bind it like any other key. Each runs its function one of two ways ([`Run`]): as if typed, or
//! in place, beside the line being typed.
//!
//! The keys are written once, in readline's notation, and `Chord` reads that notation for
//! everything else: the keyboard label the comments show, and the key name fish's `bind` takes.
//! `fish_bind` is fish's counterpart of readline's macro — the bound command runs as if typed, on
//! a line of its own, through a function the fish file keeps for its keys (`fish_as_typed_helper`)
//! — and, for a key run in place, of bash's `bind -x`, through a helper the fish file defines for
//! its keybinds and erases after them (`fish_bind_helper`).

use crate::support::shell_dialect::Shell;

/// `(readline key sequence, shell function to run, how it runs)` rows.
///
/// `\en` is ALT+N (ESC-n). A key run [`Run::AsTyped`] runs its function as if typed at the
/// prompt, so the generator has it type the function's name and the Enter (`\n`) executing it —
/// on a line of its own, the line being typed set aside till the next prompt.
pub fn bindings() -> &'static [(&'static str, &'static str, Run)] {
    &[
        (r"\en", "shell_new", Run::AsTyped),         // ALT+N → start a fresh shell session
        (r"\e\C-n", "shell_bare", Run::AsTyped),     // CTRL+ALT+N → fresh session WITHOUT bashrs (one-shot; any new shell arms again)
        (r"\eh", "brs_sourcefile", Run::AsTyped), // ALT+H → run brs_sourcefile
        (r"\ew", "brs_configure", Run::AsTyped),  // ALT+W → the settings form
        (r"\eq", "brs_compile", Run::AsTyped),    // ALT+Q → run brs_compile
        (r"\el", "proc_restart_desktop", Run::AsTyped), // ALT+L → restart / reload the running desktop environment
        (r"\e=", "term_opacity_up", Run::InPlace),   // ALT+= → a step more opaque: the '+' key, its Shift left out
        (r"\e+", "term_opacity_up", Run::InPlace),   // ALT++ → the same, from the numpad or a layout whose '+' takes no Shift
        (r"\e-", "term_opacity_down", Run::InPlace), // ALT+- → a step more transparent
        // Picking new chords, learned the hard way (each confirmed through a pty / `bind -p`):
        // CTRL+SHIFT+<key> is byte-identical to CTRL+<key> — a terminal cannot encode Shift
        // beside a control character — and CTRL+Q (0x11) is XON, eaten by the tty's flow
        // control before readline ever sees it. ALT+SHIFT+<key> arrives intact but collides
        // with the ALT+SHIFT keyboard-layout switch common on desktops. ALT+F/B/D/N/L are
        // readline's own (word motion and friends) — ALT+N and ALT+L given over to bashrs on
        // purpose; ALT+Z/W/H/Q/V were free.
        // ALT+- and ALT+= are readline's digit-argument and possible-completions, and ALT+- is
        // zsh's neg-argument — given over to bashrs too. '+' takes Shift on US-style layouts, so
        // ALT+= is bound beside ALT++; fish spells '-' `minus` (`alt--` it rejects).
    ]
}

/// How a key runs its function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Run {
    /// As if typed, on a line of its own: the line being typed is set aside, the function lands on
    /// the command line and runs, into history as typing it would put it, and the line comes back
    /// at the next prompt, cursor and all — half-typed text neither merges into the command nor is
    /// lost. Readline's macro between two `bind -x` helpers setting the line aside and giving it
    /// back, a zsh widget around its `push-line`, fish's `commandline` and a one-shot handler of
    /// its next prompt. The function is a bare name, as one run in place is: zsh names the key's
    /// widget after it.
    AsTyped,
    /// Beside the line being typed, which stays as it was, the function's output shown with it —
    /// for a key pressed in runs, each press leaving no history entry: bash's `bind -x`, a zsh
    /// widget showing it under the line (`zle -M`), fish's `__fish_echo`. The function is a bare
    /// name — zsh names the key's widget after it — that reads nothing from the terminal: zsh
    /// captures its output to show it.
    InPlace,
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
    /// fish lists it back: as the uppercase letter; and `-`, which joins a modifier to its key in
    /// fish's notation, by its name: `alt-minus`, as `alt--` fish rejects.
    pub(crate) fn fish_key(&self) -> String {
        let modifiers = [(self.ctrl, "ctrl-"), (self.alt, "alt-")];
        let mut name: String = modifiers.iter().filter(|(held, _)| *held).map(|(_, name)| *name).collect();
        match self.key {
            '-' => name.push_str("minus"),
            key => name.push(if self.shift { key.to_ascii_uppercase() } else { key }),
        }
        name
    }
}

/// The helper the fish file binds its keys through, and erases once they are bound.
pub(crate) const FISH_BIND: &str = "__bashrs_bind";

/// The function a key run [`Run::AsTyped`] calls in fish, with its command — kept, unlike
/// [`FISH_BIND`], for the keys to call when pressed ([`fish_as_typed_helper`]).
const FISH_AS_TYPED: &str = "__bashrs_as_typed";

/// The word after a [`FISH_BIND`] call's command that has its key run the command in place.
const FISH_IN_PLACE: &str = "in-place";

/// Binds `sequence` to `command` in fish, run as `run` says, through [`fish_bind_helper`].
pub(crate) fn fish_bind(sequence: &str, command: &str, run: Run) -> String {
    let bind = format!("{FISH_BIND} {} {}", Chord::parse(sequence).fish_key(), Shell::Fish.quote(command));
    match run {
        Run::AsTyped => bind,
        Run::InPlace => format!("{bind} {FISH_IN_PLACE}"),
    }
}

/// The definition of [`FISH_BIND`]: a key bound to run its command as if typed, through
/// [`FISH_AS_TYPED`] — the counterpart of readline's macro, which types the command and Enter on a
/// line of its own. Bound in fish's default mode, which is also vi's normal mode, and in vi's
/// insert mode, so the keys work wherever someone types.
///
/// A key marked [`FISH_IN_PLACE`] runs its command through `__fish_echo` instead — fish's own, the
/// one its Alt+L and Alt+W presets print through: the output goes below the line, and the prompt
/// is drawn again beneath it, the line as it was.
///
/// Fish binds three of these keys itself — Alt+H to a man page, Alt+W to `whatis`, Alt+L to listing
/// the current token — as preset bindings, which a user binding like these outranks.
pub(crate) fn fish_bind_helper() -> String {
    format!(
        "#   Each key runs its command as if typed, on a line of its own — the line being typed set aside till the next prompt —\n\
         #   or, marked {FISH_IN_PLACE}, beside the line being typed, which stays as it was; in fish's default mode, and in vi's insert mode too.\n\
         #   They replace fish's own bindings of the same keys (Alt+H man page, Alt+W whatis, Alt+L list token).\n\
         function {FISH_BIND} -a key cmd how\n    \
         set -l script \"{FISH_AS_TYPED} \"(string escape -- $cmd)\n    \
         if test \"$how\" = {FISH_IN_PLACE}\n        \
         set script \"__fish_echo \"(string escape -- $cmd)\n    \
         end\n    \
         for mode in default insert\n        \
         bind -M $mode $key $script\n    \
         end\n\
         end\n"
    )
}

/// The definition of [`FISH_AS_TYPED`], for the fish file to keep while any key runs as typed:
/// the line being typed — text and cursor — is set aside, the command takes its place and
/// executes, landing in history as typing it would, and a handler of the next prompt, gone once it
/// has run, gives the line back — the stack of lines zsh keeps for this, fish has not. Nothing
/// where no key runs as typed.
pub(crate) fn fish_as_typed_helper(bindings: &[(&str, &str, Run)]) -> String {
    if !bindings.iter().any(|(_, _, run)| *run == Run::AsTyped) {
        return String::new();
    }
    format!(
        "#   kept for the keys typing a command, which set the line being typed aside and give it back at the next prompt\n\
         function {FISH_AS_TYPED} -a cmd\n    \
         set -g __bashrs_line (commandline | string collect)\n    \
         set -g __bashrs_point (commandline -C)\n    \
         commandline -r -- $cmd\n    \
         commandline -f execute\n    \
         function __bashrs_line_restore --on-event fish_prompt\n        \
         functions -e __bashrs_line_restore\n        \
         commandline -r -- \"$__bashrs_line\"\n        \
         commandline -C -- $__bashrs_point\n        \
         set -e __bashrs_line __bashrs_point\n    \
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
        assert_eq!(label(r"\e-"), "Alt+-", "a key that is no letter, as it stands");
        assert_eq!(label(r"\e+"), "Alt++", "no Shift read into a sign, however it was typed");
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
        assert_eq!(fish(r"\e-"), "alt-minus", "fish's own joiner, by its name");
        assert_eq!(fish(r"\e="), "alt-=");
        assert_eq!(fish(r"\e+"), "alt-+");
    }

    /// A bare command is written bare, anything else as one quoted word — the helper escapes it
    /// again for the bound script, at bind time — and a key run in place says so after it.
    #[test]
    fn a_fish_bind_hands_the_helper_its_key_and_command() {
        assert_eq!(fish_bind(r"\eq", "brs_compile", Run::AsTyped), "__bashrs_bind alt-q brs_compile");
        assert_eq!(
            fish_bind(r"\el", "cinnamon --replace & disown", Run::AsTyped),
            "__bashrs_bind alt-l 'cinnamon --replace & disown'"
        );
        assert_eq!(fish_bind(r"\e-", "term_opacity_down", Run::InPlace), "__bashrs_bind alt-minus term_opacity_down in-place");
        let helper = fish_bind_helper();
        assert!(helper.contains(&format!("function {FISH_BIND} -a key cmd how\n")), "{helper}");
        assert!(helper.contains(r#"set -l script "__bashrs_as_typed "(string escape -- $cmd)"#), "{helper}");
        assert!(
            helper.contains("if test \"$how\" = in-place\n        set script \"__fish_echo \"(string escape -- $cmd)\n"),
            "in place, through fish's own __fish_echo: {helper}"
        );
        assert!(helper.contains("for mode in default insert\n        bind -M $mode $key $script\n"), "vi's insert mode too: {helper}");
    }

    /// Every key binds a bare function name, run one way whichever keys run it: zsh names a key's
    /// widget after its command — one widget per command — and a name with a space in it is a
    /// syntax error there, in bash too, which parses zsh's branch of the block it shares with zsh,
    /// so every key of both shells would go.
    #[test]
    fn every_key_binds_a_bare_name_run_one_way() {
        for (key, command, run) in bindings() {
            assert!(
                !command.is_empty() && command.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "{key} runs `{command}`, which is no bare name"
            );
            for (other, same, how) in bindings() {
                assert!(same != command || how == run, "{key} and {other} run `{command}` two ways, which one widget cannot");
            }
        }
    }
}
