//! Keybinds emitted as `bind` lines in the sourcefiles by the [`crate::cli`] generator.
//! Unconditional key → shell-function mappings live in [`bindings`]; the conditional ALT+L
//! "restart the desktop environment" keybind — which depends on which DE is running — is built by
//! [`desktop_restart`].
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
        // Picking new chords, learned the hard way (each confirmed through a pty / `bind -p`):
        // CTRL+SHIFT+<key> is byte-identical to CTRL+<key> — a terminal cannot encode Shift
        // beside a control character — and CTRL+Q (0x11) is XON, eaten by the tty's flow
        // control before readline ever sees it. ALT+SHIFT+<key> arrives intact but collides
        // with the ALT+SHIFT keyboard-layout switch common on desktops. ALT+F/B/D/N/L are
        // readline's own (word motion and friends); ALT+Z/W/H/Q/V were free.
    ]
}

/// Desktop environments / window managers we can reset, as `(process to detect, reset
/// command, extra detection guard)`. Most X11 WMs take `<wm> --replace & disown` (resets the
/// visuals without dropping the session); tiling and Wayland compositors use their own
/// in-place restart or config reload. A Wayland compositor can't restart without dropping its
/// clients, so the best it can do is reload config (Sway / Hyprland / labwc); the rest
/// (GNOME / COSMIC / niri / Wayfire / …) expose no session-preserving reset and are
/// left out. The guard is an extra shell test AND-ed into detection — GNOME's `--replace`
/// only works on X11 (on Wayland it kills the session), so it's gated to X11 sessions.
///
/// KDE Plasma on Wayland is the same kind of compositor (`kwin_wayland`), but its desktop shell
/// — panels, wallpaper, widgets — is a separate process, restartable in place as a systemd user
/// unit. It is listed LAST: `plasmashell` runs in every Plasma session, so anywhere earlier it
/// would answer before `kwin_x11` on X11, or before a tiling WM someone runs Plasma under; last,
/// it answers exactly where no listed window manager does.
const DESKTOPS: &[(&str, &str, &str)] = &[
    ("cinnamon", "cinnamon --replace & disown", ""),   // Cinnamon (X11)
    ("gnome-shell", "gnome-shell --replace & disown", r#"[ "$XDG_SESSION_TYPE" = x11 ]"#), // GNOME (X11 only)
    ("kwin_x11", "kwin_x11 --replace & disown", ""),   // KDE Plasma (X11; Wayland runs kwin_wayland)
    ("xfwm4", "xfwm4 --replace & disown", ""),         // XFCE
    ("marco", "marco --replace & disown", ""),         // MATE
    ("budgie-wm", "budgie-wm --replace & disown", ""), // Budgie
    ("gala", "gala --replace & disown", ""),           // Pantheon
    ("deepin-wm", "deepin-wm --replace & disown", ""), // Deepin
    ("i3", "i3-msg restart", ""),                      // i3
    ("qtile", "qtile cmd-obj -o cmd -f restart", ""),  // Qtile
    ("sway", "swaymsg reload", ""),                    // Sway (Wayland: reloads config)
    ("Hyprland", "hyprctl reload", ""),                // Hyprland (Wayland: reloads config)
    ("labwc", "labwc --reconfigure", ""),              // labwc (Wayland: reloads config)
    ("openbox", "openbox --replace & disown", ""),     // Openbox (LXQt / LXDE default WM)
    // KDE Plasma's desktop shell (Wayland: kwin_wayland itself can't restart) — LAST, see above.
    ("plasmashell", "systemctl --user restart plasma-plasmashell.service", ""),
];

/// The key [`desktop_restart`] binds: ALT+L.
const DESKTOP_KEY: &str = r"\el";

/// The ALT+L keybind (`DESKTOP_KEY`): at source time, reset whichever [`DESKTOPS`] entry is
/// running — first match wins, nothing bound if none match. We can't know the user's DE
/// when generating the file, so detection is deferred to the shell that sources it. Bash's
/// lines are indented to sit inside the generator's `if [ -n "$BASH_VERSION" ]` block; fish's
/// stand at the top level, as `fish_indent` lays an `if` chain out.
///
/// One `pgrep` asks for every listed process at once, and the chain then reads its answer in
/// table order — so the first listed desktop that runs still wins, whatever order the processes
/// started in. A `pgrep` per row cost a process per row on every shell start, all of them where
/// no row matches (about 9 ms in bash, 7 in fish), against one for the whole table.
pub fn desktop_restart(shell: Shell) -> String {
    match shell {
        Shell::Bash => bash_desktop_restart(),
        Shell::Fish => fish_desktop_restart(),
    }
}

/// Every [`DESKTOPS`] process as one `pgrep -x` pattern. The names are plain words (a test holds
/// them to that), so each means only itself — here, in bash's glob and in fish's `contains`.
fn desktop_pattern() -> String {
    DESKTOPS.iter().map(|(process, _, _)| *process).collect::<Vec<_>>().join("|")
}

/// `pgrep -l -d ' '` answers `PID NAME PID NAME …` on one line, so with a space at each end, a
/// running process is exactly a ` NAME ` inside it. The variable is the chain's own and is unset
/// after it: sourcing leaves nothing behind.
fn bash_desktop_restart() -> String {
    let lines: Vec<String> = DESKTOPS
        .iter()
        .enumerate()
        .map(|(i, (process, command, guard))| {
            let keyword = if i == 0 { "if" } else { "elif" };
            let guard = if guard.is_empty() { String::new() } else { format!(" && {guard}") };
            format!("{keyword} [[ $__bashrs_desktops == *\" {process} \"* ]]{guard}; then bind '\"{DESKTOP_KEY}\": \"{command}\\n\"'")
        })
        .collect();
    let mut out = String::from(
        "    # ALT+L : restart / reload the running desktop environment (resets visuals, keeps your session)\n",
    );
    out += &format!("    __bashrs_desktops=\" $(pgrep -x -l -d ' ' '{}') \"\n", desktop_pattern());
    // The `then`s aligned into a column (the shared aligner), so the eye can walk the chain of
    // detection conditions without re-finding where each one ends.
    for line in crate::support::align::align_columns(lines, &["then "]) {
        out += &format!("    {line}\n");
    }
    out += "    fi\n";
    out += "    unset __bashrs_desktops\n";
    out
}

/// Fish's `if` / `else if` chain: the same detections, each binding through [`fish_bind`], a line
/// each with the binds aligned into a column, as bash's `then`s are. Fish's `[` and `&&` read the
/// guards as bash writes them, and `&` then `disown` as two commands, as bash does, so the table
/// serves both shells unchanged. `pgrep -l` answers a `PID NAME` line per process, cut to the
/// names; `set -l` keeps the list to the sourcing of this file.
fn fish_desktop_restart() -> String {
    let lines: Vec<String> = DESKTOPS
        .iter()
        .enumerate()
        .map(|(i, (process, command, guard))| {
            let keyword = if i == 0 { "if" } else { "else if" };
            let guard = if guard.is_empty() { String::new() } else { format!(" && {guard}") };
            format!("{keyword} contains {process} $desktops{guard}; {}", fish_bind(DESKTOP_KEY, command))
        })
        .collect();
    let mut out = String::from("# ALT+L : restart / reload the running desktop environment (resets visuals, keeps your session)\n");
    out += &format!("set -l desktops (pgrep -x -l '{}' | string split -f2 ' ')\n", desktop_pattern());
    for line in crate::support::align::align_columns(lines, &[FISH_BIND]) {
        out += &format!("{line}\n");
    }
    out += "end\n";
    out
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

    /// Plasma's shell is the table's last word in both shells: it runs in every Plasma session, so
    /// placed anywhere earlier it would answer before `kwin_x11` on X11, or before a tiling WM.
    #[test]
    fn the_plasma_shell_answers_only_where_no_listed_window_manager_runs() {
        assert_eq!(DESKTOPS.last().map(|(process, _, _)| *process), Some("plasmashell"));
        let bash = desktop_restart(Shell::Bash);
        let last = bash.lines().rfind(|line| line.contains("then bind")).expect("bash chain");
        assert!(last.contains(r#"*" plasmashell "*"#) && last.contains("systemctl --user restart plasma-plasmashell.service"), "{last}");
        let fish = desktop_restart(Shell::Fish);
        let last = fish.lines().rfind(|line| line.contains("contains ")).expect("fish chain");
        assert!(last.starts_with("else if contains plasmashell $desktops;"), "{last}");
        assert!(last.ends_with("__bashrs_bind alt-l 'systemctl --user restart plasma-plasmashell.service'"), "{last}");
    }

    /// One pattern serves `pgrep`'s regex, bash's glob and fish's `contains`, so every name must
    /// mean only itself in all three: a `.` or `*` would match other processes too.
    #[test]
    fn desktop_names_are_plain_words() {
        for (process, _, _) in DESKTOPS {
            assert!(
                !process.is_empty() && process.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-')),
                "`{process}` is not a plain process name"
            );
        }
    }

    /// The same table in fish: one `pgrep` for all of it, then a first-match chain over the names
    /// it found, guards carried through — a line each, the binds in one column.
    #[test]
    fn fish_desktop_restart_is_the_same_first_match_chain() {
        let s = desktop_restart(Shell::Fish);
        assert!(s.starts_with("# ALT+L : restart / reload"), "{s}");
        assert_eq!(s.matches("pgrep").count(), 1, "one pgrep for the whole table: {s}");
        assert!(s.contains(&format!("set -l desktops (pgrep -x -l '{}' | string split -f2 ' ')\n", desktop_pattern())), "{s}");
        let chain: Vec<&str> = s.lines().filter(|line| line.contains(" contains ") || line.starts_with("if ")).collect();
        assert!(chain[0].starts_with("if contains cinnamon $desktops;"), "{s}");
        assert!(chain[0].ends_with("__bashrs_bind alt-l 'cinnamon --replace & disown'"), "{s}");
        assert!(s.contains(r#"else if contains gnome-shell $desktops && [ "$XDG_SESSION_TYPE" = x11 ];"#), "X11 gate kept: {s}");
        assert_eq!(chain.iter().filter(|line| line.starts_with("if ")).count(), 1, "single `if`: {s}");
        assert_eq!(chain.iter().filter(|line| line.starts_with("else if ")).count(), DESKTOPS.len() - 1, "the rest `else if`");
        let binds: std::collections::BTreeSet<usize> = chain.iter().filter_map(|line| line.find(FISH_BIND)).collect();
        assert_eq!(binds.len(), 1, "the binds are not aligned: {s}");
        assert!(s.ends_with("end\n"));
    }

    #[test]
    fn desktop_restart_is_a_first_match_alt_l_chain() {
        let s = desktop_restart(Shell::Bash);
        assert!(s.contains("\\el"), "should bind ALT+L (\\el): {s}");
        // One `pgrep` asks for every listed process; the chain reads its one-line answer.
        assert_eq!(s.matches("pgrep").count(), 1, "one pgrep for the whole table: {s}");
        assert!(s.contains(&format!("__bashrs_desktops=\" $(pgrep -x -l -d ' ' '{}') \"", desktop_pattern())), "{s}");
        // The conditions and their binds, spacing-agnostic: the gap before `then` belongs to
        // the aligner, asserted separately below.
        assert!(s.contains(r#"if [[ $__bashrs_desktops == *" cinnamon "* ]];"#));
        assert!(s.contains("cinnamon --replace & disown"));
        // GNOME's --replace is gated to X11 — on Wayland it would kill the session
        assert!(s.contains(r#"[[ $__bashrs_desktops == *" gnome-shell "* ]] && [ "$XDG_SESSION_TYPE" = x11 ];"#));
        // a tiling WM with its own (non-`--replace`) restart is carried through verbatim
        assert!(s.contains(r#"elif [[ $__bashrs_desktops == *" i3 "* ]];"#) && s.contains("i3-msg restart"));
        // a Wayland compositor reload keeps clients
        assert!(s.contains(r#"elif [[ $__bashrs_desktops == *" Hyprland "* ]];"#) && s.contains("hyprctl reload"));
        assert!(s.trim_end().ends_with("fi\n    unset __bashrs_desktops"), "the chain's variable goes with it: {s}");
        assert_eq!(s.matches("    if ").count(), 1, "single `if`, the rest `elif`");
        // Every detection line's `then` sits in ONE column — the detection chain reads as a
        // table, condition left, action right. Equality across all rows means the shared
        // aligner really ran here.
        let then_columns: std::collections::BTreeSet<usize> = s
            .lines()
            .filter(|line| line.contains("__bashrs_desktops =="))
            .map(|line| line.find("then bind").expect("every detection line binds"))
            .collect();
        assert_eq!(then_columns.len(), 1, "the `then`s are not aligned: {s}");
    }
}
