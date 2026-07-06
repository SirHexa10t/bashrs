//! Readline keybinds emitted as `bind` lines in `sourcefile.sh` by the [`crate::cli`]
//! generator. Unconditional key → shell-function mappings live in [`bindings`]; the
//! conditional ALT+L "restart the desktop environment" keybind — which depends on which
//! DE is running — is built by [`desktop_restart`].

/// `(readline key sequence, shell function to run)` pairs.
///
/// `\en` is ALT+N (ESC-n). The function is run as if typed at the prompt, so the
/// generator appends the Enter (`\n`) that executes it.
pub fn bindings() -> &'static [(&'static str, &'static str)] {
    &[
        (r"\en", "session_new"),       // ALT+N → start a fresh shell session
        (r"\eh", "bashrs_sourcefile"), // ALT+H → run bashrs_sourcefile
        (r"\eq", "bashrs_compile"),    // ALT+Q → run bashrs_compile
    ]
}

/// Desktop environments / window managers we can reset, as `(process to detect, command
/// to reset it)`. Most X11 WMs take `<wm> --replace & disown` (resets the visuals without
/// dropping the session); tiling WMs use their own in-place restart/reload. Pure-Wayland
/// compositors that can't be replaced live (e.g. COSMIC) are intentionally left out.
const DESKTOPS: &[(&str, &str)] = &[
    ("cinnamon", "cinnamon --replace & disown"),       // Cinnamon
    ("gnome-shell", "gnome-shell --replace & disown"), // GNOME
    ("kwin_x11", "kwin_x11 --replace & disown"),       // KDE Plasma (X11)
    ("xfwm4", "xfwm4 --replace & disown"),             // XFCE
    ("marco", "marco --replace & disown"),             // MATE
    ("budgie-wm", "budgie-wm --replace & disown"),     // Budgie
    ("gala", "gala --replace & disown"),               // Pantheon
    ("deepin-wm", "deepin-wm --replace & disown"),     // Deepin
    ("i3", "i3-msg restart"),                          // i3
    ("qtile", "qtile cmd-obj -o cmd -f restart"),      // Qtile
    ("sway", "swaymsg reload"),                        // Sway (Wayland: reloads config)
    ("openbox", "openbox --replace & disown"),         // Openbox (LXQt / LXDE default WM)
];

/// The ALT+L keybind (`\el`): at source time, reset whichever [`DESKTOPS`] entry is
/// running — first match wins, nothing bound if none match. We can't know the user's DE
/// when generating the file, so detection is deferred to the shell that sources it. Lines
/// are indented to sit inside the generator's `if [ -n "$BASH_VERSION" ]` block.
pub fn desktop_restart() -> String {
    let mut out = String::from(
        "    # ALT+L : restart the running desktop environment (resets visuals, keeps your session)\n",
    );
    for (i, (process, command)) in DESKTOPS.iter().enumerate() {
        let keyword = if i == 0 { "if" } else { "elif" };
        out += &format!("    {keyword} pgrep -x {process} >/dev/null; then bind '\"\\el\": \"{command}\\n\"'\n");
    }
    out += "    fi\n";
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_restart_is_a_first_match_alt_l_chain() {
        let s = desktop_restart();
        assert!(s.contains("\\el"), "should bind ALT+L (\\el): {s}");
        assert!(s.contains("if pgrep -x cinnamon >/dev/null; then bind"));
        assert!(s.contains("cinnamon --replace & disown"));
        assert!(s.contains("elif pgrep -x gnome-shell >/dev/null; then bind"));
        // a tiling WM with its own (non-`--replace`) restart is carried through verbatim
        assert!(s.contains("elif pgrep -x i3 >/dev/null; then bind") && s.contains("i3-msg restart"));
        assert!(s.trim_end().ends_with("fi"));
        assert_eq!(s.matches("    if ").count(), 1, "single `if`, the rest `elif`");
    }
}
