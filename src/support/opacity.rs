//! The terminal's opacity, stepped up or down: by the terminal itself where it can — only the
//! background fades, so the text stays crisp — else by the window manager, which fades the whole
//! window, text and all; else not at all, with every reason said. `BACKENDS`, in that order of
//! preference, decides: the first that applies here and succeeds does the step.
//!
//! A terminal answers for itself only when this shell runs directly in it. Inside a multiplexer
//! (tmux, screen, zellij, herdr) its variables are whatever they were when the multiplexer
//! started — possibly another window, possibly another terminal — and its control channel is the
//! multiplexer's; there the window manager acts instead, on the focused window, which is the
//! terminal hosting the multiplexer.

use std::process::Command;

use crate::support::windows;

/// What fades.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Level {
    /// The terminal's background only; the text stays crisp.
    Background,
    /// The whole window, text and all.
    Window,
}

/// The variables a backend reads to tell whether it applies — the process environment in
/// earnest, a fixed set in tests.
type Vars<'a> = dyn Fn(&str) -> Option<String> + 'a;

/// One way to step the opacity.
struct Backend {
    /// Who does it, as the user knows it.
    name: &'static str,
    level: Level,
    /// Whether it can act here: not at all (`None`), yes, or — this is its terminal, but out of
    /// reach — no, and why.
    here: fn(&Vars) -> Option<Result<(), String>>,
    /// Step by `delta` (± a fraction), answering with the opacity now when it can tell.
    step: fn(f64) -> Result<Option<f64>, String>,
}

/// Every way there is, most preferred first: the terminals that fade only their background, then
/// the window managers, the X11 property every X11 compositor reads coming last.
const BACKENDS: &[Backend] = &[
    Backend { name: "kitty", level: Level::Background, here: in_kitty, step: kitty_step },
    Backend { name: "Alacritty", level: Level::Background, here: in_alacritty, step: alacritty_step },
    Backend { name: "KWin", level: Level::Window, here: under_kwin, step: kwin_step },
    Backend { name: "sway", level: Level::Window, here: under_sway, step: sway_step },
    Backend { name: "Hyprland", level: Level::Window, here: under_hyprland, step: hyprland_step },
    Backend { name: "X11 compositor", level: Level::Window, here: under_x11, step: x11_step },
];

/// The multiplexers, each by the variable it sets in the shells it runs.
const MULTIPLEXERS: &[(&str, &str)] = &[("TMUX", "tmux"), ("STY", "screen"), ("ZELLIJ", "zellij"), ("HERDR_PANE_ID", "herdr")];

/// The lowest a background goes: no floor — a fully clear background still shows the text.
const BACKGROUND_FLOOR: f64 = 0.0;

/// The lowest a whole window goes where this code sets it (Hyprland, X11): where KWin's own
/// opacity actions stop (its `useractions.cpp` never goes below 0.05), as a window any fainter is
/// all but impossible to find again. sway's relative step keeps its own floor, which is 0.
const WINDOW_FLOOR: f64 = 0.05;

/// How far one of KWin's opacity actions moves a window (its `useractions.cpp`: ±0.05 a press).
const KWIN_STEP: f64 = 0.05;

/// What a step did.
#[derive(Debug, PartialEq)]
pub(crate) struct Stepped {
    /// Who did it.
    pub(crate) by: &'static str,
    pub(crate) level: Level,
    /// The opacity now, when the backend could tell.
    pub(crate) now: Option<f64>,
    /// The ways tried before it, each with why it did not do the step.
    pub(crate) passed_over: Vec<(&'static str, String)>,
}

/// Step the terminal's opacity by `delta` (± a fraction) the first way that works here. `Err`
/// holds every way that applied and why it could not; empty when none applied at all.
pub(crate) fn step(delta: f64) -> Result<Stepped, Vec<(&'static str, String)>> {
    step_with(BACKENDS, &|name| std::env::var(name).ok(), delta)
}

/// The names of the ways at `level`, in preference order — what the commands name when nothing
/// applied.
pub(crate) fn names(level: Level) -> Vec<&'static str> {
    BACKENDS.iter().filter(|backend| backend.level == level).map(|backend| backend.name).collect()
}

/// [`step`] over the given `backends` and variables.
fn step_with(backends: &[Backend], vars: &Vars, delta: f64) -> Result<Stepped, Vec<(&'static str, String)>> {
    let mut passed_over = Vec::new();
    for backend in backends {
        match (backend.here)(vars) {
            None => {}
            Some(Err(why)) => passed_over.push((backend.name, why)),
            Some(Ok(())) => match (backend.step)(delta) {
                Ok(now) => return Ok(Stepped { by: backend.name, level: backend.level, now, passed_over }),
                Err(why) => passed_over.push((backend.name, why)),
            },
        }
    }
    Err(passed_over)
}

/// The opacity one step from `now`: on the whole-percent grid, so repeated steps never gather
/// float residue, and within `floor..=1`.
fn stepped(now: f64, delta: f64, floor: f64) -> f64 {
    ((now + delta) * 100.0).round().clamp(floor * 100.0, 100.0) / 100.0
}

/// The multiplexer this shell runs in, if any.
fn multiplexed(vars: &Vars) -> Option<&'static str> {
    MULTIPLEXERS.iter().find(|(var, _)| vars(var).is_some()).map(|(_, name)| *name)
}

/// For a terminal this shell is in: reachable, unless a multiplexer stands between them.
fn reachable(vars: &Vars, terminal: &str) -> Result<(), String> {
    match multiplexed(vars) {
        Some(multiplexer) => Err(format!("inside {multiplexer}, {terminal}'s own controls are out of reach")),
        None => Ok(()),
    }
}

/// Run `argv`, answering with its stdout — or why not: the program isn't installed (`None`), or
/// it failed, in its own first words (`Some`).
fn run(argv: &[&str]) -> Result<String, Option<String>> {
    let out = match Command::new(argv[0]).args(&argv[1..]).output() {
        Ok(out) => out,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Err(None),
        Err(err) => return Err(Some(format!("could not run {}: {err}", argv[0]))),
    };
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let said = String::from_utf8_lossy(&out.stderr);
    let first = said.lines().map(str::trim).find(|line| !line.is_empty());
    Err(Some(first.map_or_else(|| format!("{} failed", argv[0]), |line| line.trim_start_matches("Error: ").to_string())))
}

/// [`run`], with a missing program said as such.
fn run_named(argv: &[&str]) -> Result<String, String> {
    run(argv).map_err(|why| why.unwrap_or_else(|| format!("{} is not installed", argv[0])))
}

// --- kitty -------------------------------------------------------------------------------------

fn in_kitty(vars: &Vars) -> Option<Result<(), String>> {
    vars("KITTY_WINDOW_ID")?;
    Some(reachable(vars, "kitty"))
}

/// Read the background opacity of the OS window this shell is in, then set the next one — over
/// kitty's remote control, which reports back what kitty.conf must say (`allow_remote_control`,
/// `dynamic_background_opacity`) when it is off. Not the relative `set_background_opacity`
/// action: with the dynamic opacity off, that one only pops up an error inside kitty and
/// reports success to the caller.
fn kitty_step(delta: f64) -> Result<Option<f64>, String> {
    let id = std::env::var("KITTY_WINDOW_ID").map_err(|_| "KITTY_WINDOW_ID is not set".to_string())?;
    let now = kitty_opacity(&kitty(&["ls", "--self"])?).ok_or("kitty's listing names no background opacity")?;
    let next = stepped(now, delta, BACKGROUND_FLOOR);
    kitty(&["set-background-opacity", "--match", &format!("id:{id}"), &format!("{next:.2}")])?;
    Ok(Some(next))
}

/// `kitten @ <args>` — or the older `kitty @`, where there is no `kitten`.
fn kitty(args: &[&str]) -> Result<String, String> {
    let with = |program: &'static str| [&[program, "@"][..], args].concat();
    match run(&with("kitten")) {
        Err(None) => run_named(&with("kitty")),
        ran => ran.map_err(|why| why.unwrap_or_default()),
    }
}

/// The background opacity in `kitten @ ls --self`'s listing: that of its one OS window.
fn kitty_opacity(listing: &str) -> Option<f64> {
    let windows: serde_json::Value = serde_json::from_str(listing).ok()?;
    windows.get(0)?.get("background_opacity")?.as_f64()
}

// --- Alacritty ---------------------------------------------------------------------------------

fn in_alacritty(vars: &Vars) -> Option<Result<(), String>> {
    vars("ALACRITTY_WINDOW_ID")?;
    Some(reachable(vars, "Alacritty"))
}

/// Read this window's opacity back (`alacritty msg get-config`, Alacritty 0.16 and newer), then
/// set the next one for it alone — both address the window in `$ALACRITTY_WINDOW_ID` by default.
fn alacritty_step(delta: f64) -> Result<Option<f64>, String> {
    let config = run_named(&["alacritty", "msg", "get-config"])
        .map_err(|why| format!("{why} — reading the opacity back takes Alacritty 0.16 or newer"))?;
    let next = stepped(alacritty_opacity(&config), delta, BACKGROUND_FLOOR);
    run_named(&["alacritty", "msg", "config", &format!("window.opacity={next:.2}")])?;
    Ok(Some(next))
}

/// `window.opacity` in Alacritty's runtime config (JSON) — opaque where it says nothing.
fn alacritty_opacity(config: &str) -> f64 {
    serde_json::from_str::<serde_json::Value>(config)
        .ok()
        .and_then(|config| config.get("window")?.get("opacity")?.as_f64())
        .unwrap_or(1.0)
}

// --- KWin --------------------------------------------------------------------------------------

/// KDE's session says so in `XDG_CURRENT_DESKTOP`, one of whose `:`-separated names is `KDE`.
fn under_kwin(vars: &Vars) -> Option<Result<(), String>> {
    vars("XDG_CURRENT_DESKTOP")?.split(':').any(|desktop| desktop.eq_ignore_ascii_case("KDE")).then_some(Ok(()))
}

/// Press KWin's own "Increase/Decrease Opacity" actions — 5% each — as often as the step needs,
/// over D-Bus. KWin acts on the focused window.
fn kwin_step(delta: f64) -> Result<Option<f64>, String> {
    let action = if delta > 0.0 { "Increase Opacity" } else { "Decrease Opacity" };
    for _ in 0..kwin_presses(delta) {
        kwin_invoke(action)?;
    }
    Ok(None)
}

/// How many of KWin's 5% presses make a step of `delta` — one at least.
fn kwin_presses(delta: f64) -> usize {
    ((delta.abs() / KWIN_STEP).round() as usize).max(1)
}

/// The D-Bus calls that press one of KWin's actions, in the order tried: `busctl` (systemd's)
/// first, `dbus-send` (the reference D-Bus tools) where there is no `busctl`.
fn kwin_calls(action: &str) -> [Vec<String>; 2] {
    let own = |words: &[&str]| words.iter().map(|word| word.to_string()).collect::<Vec<_>>();
    let service = ["org.kde.kglobalaccel", "/component/kwin"];
    [
        own(&["busctl", "--user", "call", service[0], service[1], "org.kde.kglobalaccel.Component", "invokeShortcut", "s", action]),
        own(&[
            "dbus-send",
            "--session",
            "--type=method_call",
            &format!("--dest={}", service[0]),
            service[1],
            "org.kde.kglobalaccel.Component.invokeShortcut",
            &format!("string:{action}"),
        ]),
    ]
}

fn kwin_invoke(action: &str) -> Result<(), String> {
    for call in kwin_calls(action) {
        let argv: Vec<&str> = call.iter().map(String::as_str).collect();
        match run(&argv) {
            Ok(_) => return Ok(()),
            Err(None) => continue,
            Err(Some(why)) => return Err(why),
        }
    }
    Err("neither busctl nor dbus-send is installed, so KWin can't be asked".into())
}

// --- sway --------------------------------------------------------------------------------------

fn under_sway(vars: &Vars) -> Option<Result<(), String>> {
    vars("SWAYSOCK").map(|_| Ok(()))
}

/// sway's own relative step, on the focused window.
fn sway_step(delta: f64) -> Result<Option<f64>, String> {
    run_named(&sway_argv(delta).iter().map(String::as_str).collect::<Vec<_>>())?;
    Ok(None)
}

fn sway_argv(delta: f64) -> Vec<String> {
    let direction = if delta > 0.0 { "plus" } else { "minus" };
    ["swaymsg", "opacity", direction, &format!("{:.2}", delta.abs())].map(str::to_string).to_vec()
}

// --- Hyprland ----------------------------------------------------------------------------------

fn under_hyprland(vars: &Vars) -> Option<Result<(), String>> {
    vars("HYPRLAND_INSTANCE_SIGNATURE").map(|_| Ok(()))
}

/// Read the focused window's opacity (`hyprctl getprop`), then set the next one — Hyprland's
/// `setprop`, which answers `ok` when it took.
fn hyprland_step(delta: f64) -> Result<Option<f64>, String> {
    let said = run_named(&["hyprctl", "getprop", "activewindow", "opacity"])?;
    let now = hyprland_opacity(&said).ok_or_else(|| format!("this Hyprland doesn't report a window's opacity ({})", said.trim()))?;
    let next = stepped(now, delta, WINDOW_FLOOR);
    let answer = run_named(&["hyprctl", "dispatch", "setprop", "activewindow", "opacity", &format!("{next:.2}")])?;
    if answer.trim() != "ok" {
        return Err(format!("Hyprland answered: {}", answer.trim()));
    }
    Ok(Some(next))
}

/// The opacity in `hyprctl getprop … opacity`'s answer: its first word, a number.
fn hyprland_opacity(said: &str) -> Option<f64> {
    said.split_whitespace().next()?.parse().ok()
}

// --- X11 ---------------------------------------------------------------------------------------

/// An X11 session: said so outright, or a display with no Wayland one beside it.
fn under_x11(vars: &Vars) -> Option<Result<(), String>> {
    let x11 = vars("XDG_SESSION_TYPE").as_deref() == Some("x11")
        || (vars("DISPLAY").is_some() && vars("WAYLAND_DISPLAY").is_none());
    x11.then_some(Ok(()))
}

fn x11_step(delta: f64) -> Result<Option<f64>, String> {
    windows::step_opacity(|now| stepped(now, delta, WINDOW_FLOOR)).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars<'a>(set: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| set.iter().find(|(var, _)| *var == name).map(|(_, value)| value.to_string())
    }

    /// A stand-in backend: applies (or not) and steps (or fails) as told.
    fn backend(name: &'static str, level: Level, here: fn(&Vars) -> Option<Result<(), String>>, step: fn(f64) -> Result<Option<f64>, String>) -> Backend {
        Backend { name, level, here, step }
    }

    /// The first way that applies and works does the step; a way that applies but fails — or
    /// can't be reached — is passed over with its reason, which the result carries; ways that
    /// don't apply say nothing; and when nothing works, every reason comes back.
    #[test]
    fn the_first_way_that_works_steps_and_the_ones_before_it_say_why_not() {
        let absent = |_: &Vars| None;
        let present = |_: &Vars| Some(Ok(()));
        let unreachable = |_: &Vars| Some(Err("out of reach".to_string()));
        let fails = |_: f64| Err("refused".to_string());
        let works = |delta: f64| Ok(Some(0.5 + delta));
        let none: &Vars = &|_| None;

        let ways = [
            backend("a", Level::Background, absent, works),
            backend("b", Level::Background, unreachable, works),
            backend("c", Level::Background, present, fails),
            backend("d", Level::Window, present, works),
            backend("e", Level::Window, present, works),
        ];
        let stepped = step_with(&ways, none, 0.05).expect("d works");
        assert_eq!((stepped.by, stepped.level, stepped.now), ("d", Level::Window, Some(0.55)));
        assert_eq!(stepped.passed_over, [("b", "out of reach".to_string()), ("c", "refused".to_string())]);

        let failing = [backend("a", Level::Background, absent, works), backend("c", Level::Window, present, fails)];
        assert_eq!(step_with(&failing, none, 0.05), Err(vec![("c", "refused".to_string())]));
        assert_eq!(step_with(&[backend("a", Level::Window, absent, works)], none, 0.05), Err(Vec::new()), "nothing applied");
    }

    /// A terminal applies when its variable is set — and is out of reach, with the reason, when
    /// a multiplexer stands between; the window managers apply by their own variables, inside a
    /// multiplexer or not.
    #[test]
    fn each_way_applies_by_its_own_variables() {
        assert_eq!(in_kitty(&vars(&[("KITTY_WINDOW_ID", "3")])), Some(Ok(())));
        assert_eq!(in_kitty(&vars(&[])), None);
        for (var, multiplexer) in MULTIPLEXERS {
            let inside = in_kitty(&vars(&[("KITTY_WINDOW_ID", "3"), (var, "1")]));
            assert_eq!(inside, Some(Err(format!("inside {multiplexer}, kitty's own controls are out of reach"))));
            assert_eq!(under_sway(&vars(&[("SWAYSOCK", "/run/sway"), (var, "1")])), Some(Ok(())), "the window manager still reaches");
        }
        assert_eq!(in_alacritty(&vars(&[("ALACRITTY_WINDOW_ID", "9")])), Some(Ok(())));
        assert_eq!(in_alacritty(&vars(&[("ALACRITTY_WINDOW_ID", "9"), ("TMUX", "/tmp/t")])).map(|here| here.is_err()), Some(true));

        assert_eq!(under_kwin(&vars(&[("XDG_CURRENT_DESKTOP", "KDE")])), Some(Ok(())));
        assert_eq!(under_kwin(&vars(&[("XDG_CURRENT_DESKTOP", "kde")])), Some(Ok(())), "the case varies");
        assert_eq!(under_kwin(&vars(&[("XDG_CURRENT_DESKTOP", "X-Cinnamon")])), None);
        assert_eq!(under_kwin(&vars(&[("XDG_CURRENT_DESKTOP", "ubuntu:GNOME")])), None);
        assert_eq!(under_hyprland(&vars(&[("HYPRLAND_INSTANCE_SIGNATURE", "abc")])), Some(Ok(())));
        assert_eq!(under_sway(&vars(&[])), None);

        assert_eq!(under_x11(&vars(&[("XDG_SESSION_TYPE", "x11")])), Some(Ok(())));
        assert_eq!(under_x11(&vars(&[("DISPLAY", ":0")])), Some(Ok(())), "a display, no Wayland");
        assert_eq!(under_x11(&vars(&[("DISPLAY", ":0"), ("WAYLAND_DISPLAY", "wayland-0")])), None, "XWayland is no X11 session");
        assert_eq!(under_x11(&vars(&[("XDG_SESSION_TYPE", "wayland")])), None);
    }

    /// A step lands on the whole-percent grid — no float residue however many steps it takes —
    /// and never leaves `floor..=1`.
    #[test]
    fn a_step_lands_on_whole_percents_within_its_floor() {
        assert_eq!(stepped(0.9, 0.05, 0.0), 0.95);
        assert_eq!(stepped(0.85, -0.05, 0.0), 0.8);
        let mut opacity = 1.0;
        for _ in 0..7 {
            opacity = stepped(opacity, -0.1, 0.0);
        }
        assert_eq!(opacity, 0.3, "seven steps, no residue");
        assert_eq!(stepped(0.98, 0.05, 0.0), 1.0, "never past opaque");
        assert_eq!(stepped(0.07, -0.05, WINDOW_FLOOR), WINDOW_FLOOR, "never below the floor");
        assert_eq!(stepped(0.03, -0.05, BACKGROUND_FLOOR), 0.0);
    }

    /// The answers the backends read back, as their tools give them.
    #[test]
    fn each_tools_answer_yields_the_opacity() {
        let listing = r#"[{"id": 1, "background_opacity": 0.8, "tabs": [{"windows": [{"id": 3}]}]}]"#;
        assert_eq!(kitty_opacity(listing), Some(0.8));
        assert_eq!(kitty_opacity("[]"), None);
        assert_eq!(alacritty_opacity(r#"{"window": {"opacity": 0.9, "decorations": "Full"}}"#), 0.9);
        assert_eq!(alacritty_opacity(r#"{"window": {}}"#), 1.0, "unsaid is opaque");
        assert_eq!(hyprland_opacity("0.85\n"), Some(0.85));
        assert_eq!(hyprland_opacity("1"), Some(1.0));
        assert_eq!(hyprland_opacity("prop not found"), None);
    }

    /// How each window manager is asked: KWin by as many 5% presses as the step needs, over
    /// busctl first and dbus-send second; sway by its own relative step.
    #[test]
    fn each_window_manager_is_asked_in_its_own_terms() {
        assert_eq!([0.05, 0.2, 0.01, -0.1].map(kwin_presses), [1, 4, 1, 2]);
        let [busctl, dbus_send] = kwin_calls("Increase Opacity");
        assert_eq!(
            busctl,
            ["busctl", "--user", "call", "org.kde.kglobalaccel", "/component/kwin", "org.kde.kglobalaccel.Component", "invokeShortcut", "s", "Increase Opacity"]
        );
        assert_eq!(dbus_send[0], "dbus-send");
        assert!(dbus_send.contains(&"string:Increase Opacity".to_string()), "{dbus_send:?}");
        assert_eq!(sway_argv(0.05), ["swaymsg", "opacity", "plus", "0.05"]);
        assert_eq!(sway_argv(-0.2), ["swaymsg", "opacity", "minus", "0.20"]);
    }

    /// The names the commands give when nothing applied come from the table itself.
    #[test]
    fn the_ways_are_named_by_level_in_preference_order() {
        assert_eq!(names(Level::Background), ["kitty", "Alacritty"]);
        assert_eq!(names(Level::Window), ["KWin", "sway", "Hyprland", "X11 compositor"]);
    }
}
