//! What's on the screen: the windows the desktop lists, each with its title and owning process —
//! and the few things done to a window from outside: asking it to close, stepping its opacity.
//!
//! Two sources, and [`list`] takes the session's: in an X11 session the X server's list, and in a
//! Wayland one KWin's — KDE Plasma's compositor, which manages Wayland and X11 windows alike —
//! through sequencer's KWin scripts (`sequencer::input::kde`). Xwayland sees only its own X11
//! windows, so KWin is the one way to a native Wayland window; a Wayland session without KWin
//! falls back to the X server, whose XWayland windows are then all there is to see. Each window
//! remembers who listed it ([`WindowId`]), and a close request goes back the same way.
//!
//! The X server is asked in-process over `x11rb` — already in the dependency tree for
//! sequencer's X11 backends — rather than shelling to `wmctrl`/`xdotool`, which may not be
//! installed and whose output would need parsing back apart. Who wants which window is the
//! caller's question.
//!
//! The usual X11 caveats apply and are reported rather than papered over: it needs a display (on
//! Wayland only XWayland windows are visible) and a window manager that maintains
//! `_NET_CLIENT_LIST` (every mainstream one does).
//!
//! **The PID comes from the X server, not the window.** The obvious source, `_NET_WM_PID`, is
//! self-reported by the application — and an app in a PID namespace (Flatpak, Snap) reports its
//! pid *inside* the sandbox, where the browser is proudly process 2. Matching that against the
//! host's `/proc` finds `kthreadd`. The X-Resource extension instead asks the server who owns
//! the window's connection, and the server answers with the pid it accepted the socket from —
//! the real one, whatever namespace the client lives in. `_NET_WM_PID` remains only as the
//! fallback for a server without the extension. KWin's list keeps the same guarantee: it asks the
//! X server the same way for an X11 window's owner, and a Wayland window's is whoever holds its
//! Wayland connection.

use sequencer::input::kde::KWinError;
use x11rb::connection::Connection;
use x11rb::protocol::res::{self, ClientIdMask, ClientIdSpec};
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

/// One mapped window that names its owner.
pub(crate) struct Window {
    /// The handle a close request is addressed to.
    pub(crate) id: WindowId,
    /// The owning process.
    pub(crate) pid: u32,
    /// Its title: `_NET_WM_NAME` (UTF-8), or the older `WM_NAME` when that's all there is — or
    /// for KWin's list, the title as the app set it. For a browser this carries the ACTIVE tab's
    /// title — background tabs are not windows.
    pub(crate) title: String,
}

/// A window as whoever listed it knows it — and so who a close request goes back to.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum WindowId {
    /// An X window id: closed over the X connection.
    X11(u32),
    /// KWin's id for the window, Wayland or X11 alike: closed through KWin.
    KWin(String),
}

/// What [`list`] found — and, where KWin should have answered but could not, why the windows are
/// XWayland's alone.
pub(crate) struct Listing {
    pub(crate) windows: Vec<Window>,
    /// Why KWin was passed over: in a Wayland session it runs in, but it would not list.
    pub(crate) passed_over: Option<String>,
}

/// Every window the desktop lists, with the PID and title of each: KWin's list in a Wayland
/// session KWin runs, the X server's otherwise. `Err` is a human-readable reason there's nothing
/// to enumerate (no display, no EWMH window manager).
pub(crate) fn list() -> Result<Listing, String> {
    let mut passed_over = None;
    if !x11_session(&|name| std::env::var(name).ok()) {
        match kwin_list() {
            Ok(windows) => return Ok(Listing { windows, passed_over }),
            // No KWin to ask — another compositor, or no session bus: the X server is all there is.
            Err(KWinError::NotRunning | KWinError::Bus(_)) => {}
            Err(why) => passed_over = Some(why.to_string()),
        }
    }
    match x11_list() {
        Ok(windows) => Ok(Listing { windows, passed_over }),
        Err(why) => Err(match passed_over {
            Some(kwin) => format!("KWin couldn't list its windows ({kwin}), and {why}"),
            None => why,
        }),
    }
}

/// An X11 session: said so outright, or a display with no Wayland one beside it — Xwayland's
/// display is no X11 session. `vars` reads the environment: the process's in earnest, a fixed set
/// in tests.
pub(crate) fn x11_session(vars: &dyn Fn(&str) -> Option<String>) -> bool {
    vars("XDG_SESSION_TYPE").as_deref() == Some("x11")
        || (vars("DISPLAY").is_some() && vars("WAYLAND_DISPLAY").is_none())
}

/// Ask each window to close, the way the titlebar's ✕ does — through whoever listed it. The
/// application decides what that means: it may raise an "unsaved changes" dialog, and KWin ends
/// an X11 app outright when its window takes no close request.
pub(crate) fn close(ids: &[WindowId]) -> Result<(), String> {
    let (x11, kwin) = by_source(ids);
    if !x11.is_empty() {
        x11_close(&x11)?;
    }
    if !kwin.is_empty() {
        sequencer::input::kde::close(&kwin).map_err(|err| err.to_string())?;
    }
    Ok(())
}

/// Which of `ids` are still on screen. Errors (display gone mid-check) read as "none left" —
/// the caller only uses this to decide whether to keep waiting.
pub(crate) fn still_open(ids: &[WindowId]) -> Vec<WindowId> {
    let (x11, kwin) = by_source(ids);
    let mut open = std::collections::BTreeSet::new();
    if !x11.is_empty() {
        open.extend(x11_list().into_iter().flatten().map(|window| window.id));
    }
    if !kwin.is_empty() {
        open.extend(kwin_list().into_iter().flatten().map(|window| window.id));
    }
    ids.iter().filter(|id| open.contains(*id)).cloned().collect()
}

/// `ids` split by who listed them: the X11 window ids, then KWin's.
fn by_source(ids: &[WindowId]) -> (Vec<u32>, Vec<String>) {
    let mut x11 = Vec::new();
    let mut kwin = Vec::new();
    for id in ids {
        match id {
            WindowId::X11(id) => x11.push(*id),
            WindowId::KWin(id) => kwin.push(id.clone()),
        }
    }
    (x11, kwin)
}

/// The one rule for a window from either source: it must name an owner, to be matched to a
/// process, and carry a title, to be matched to a request. Missing either, it silently isn't a
/// candidate.
fn named(id: WindowId, pid: Option<u32>, title: Option<String>) -> Option<Window> {
    Some(Window { id, pid: pid?, title: title.filter(|title| !title.is_empty())? })
}

// --- KWin --------------------------------------------------------------------------------------

/// KWin's windows — every kind, its own special ones (desktop, panels, notifications) left out.
fn kwin_list() -> Result<Vec<Window>, KWinError> {
    Ok(sequencer::input::kde::windows()?
        .into_iter()
        .filter_map(|window| named(WindowId::KWin(window.id), window.pid, Some(window.title)))
        .collect())
}

// --- X11 ---------------------------------------------------------------------------------------

/// Every window the X11 window manager lists, with the PID and title of each. `Err` is a
/// human-readable reason there's nothing to enumerate (no display, no EWMH window manager).
fn x11_list() -> Result<Vec<Window>, String> {
    let (conn, screen) = x11rb::connect(None)
        .map_err(|_| "no X11 display — on Wayland, only XWayland windows would be visible anyway".to_string())?;
    let root = conn.setup().roots[screen].root;
    let atom = |name: &str| -> Result<u32, String> {
        conn.intern_atom(false, name.as_bytes())
            .map_err(|err| format!("X connection failed: {err}"))?
            .reply()
            .map(|reply| reply.atom)
            .map_err(|err| format!("X connection failed: {err}"))
    };
    let client_list = atom("_NET_CLIENT_LIST")?;
    let net_wm_pid = atom("_NET_WM_PID")?;
    let net_wm_name = atom("_NET_WM_NAME")?;
    let utf8_string = atom("UTF8_STRING")?;

    let listed = conn
        .get_property(false, root, client_list, AtomEnum::WINDOW, 0, u32::MAX)
        .map_err(|err| format!("could not read the window list: {err}"))?
        .reply()
        .map_err(|err| format!("could not read the window list: {err}"))?;
    let ids: Vec<u32> = listed.value32().map(Iterator::collect).unwrap_or_default();
    if ids.is_empty() {
        return Err("the window manager exposes no client list (_NET_CLIENT_LIST) — cannot enumerate windows".into());
    }

    let mut windows = Vec::new();
    for id in ids {
        // A window without a PID can't be matched to a process; without a title it can't be
        // matched to a request. Either way it silently isn't a candidate ([`named`]) — and one
        // without an owner isn't asked for its title.
        let pid = _server_pid(&conn, id).or_else(|| _cardinal(&conn, id, net_wm_pid));
        let title = pid.and_then(|_| {
            _text(&conn, id, net_wm_name, utf8_string)
                .or_else(|| _text(&conn, id, AtomEnum::WM_NAME.into(), AtomEnum::ANY.into()))
        });
        windows.extend(named(WindowId::X11(id), pid, title));
    }
    Ok(windows)
}

/// The pid the X server itself holds for the client owning `window` — X-Resource's
/// `LocalClientPID`, which is namespace-proof: it is whoever the server accepted the connection
/// from, not whatever the client claims about itself. `None` when the server lacks the extension
/// (the caller falls back to `_NET_WM_PID`) or the window is gone.
fn _server_pid(conn: &impl Connection, window: u32) -> Option<u32> {
    let spec = ClientIdSpec { client: window, mask: ClientIdMask::LOCAL_CLIENT_PID };
    let reply = res::query_client_ids(conn, &[spec]).ok()?.reply().ok()?;
    reply
        .ids
        .into_iter()
        .find(|id| u32::from(id.spec.mask) & u32::from(ClientIdMask::LOCAL_CLIENT_PID) != 0)
        .and_then(|id| id.value.first().copied())
        .filter(|pid| *pid > 0)
}

/// Ask each window to close, the way the titlebar's ✕ does. The application decides what that
/// means (it may raise an "unsaved changes" dialog), and the rest of its windows are untouched:
/// this is the one per-window verb X has, which is exactly why it exists here — a signal can
/// only ever address the whole process.
///
/// Delivered DIRECTLY to the owning client as ICCCM `WM_DELETE_WINDOW`, not routed through the
/// window manager. The first cut here sent EWMH `_NET_CLOSE_WINDOW` to the root instead — a
/// request TO the WM to do the closing — and a real-world WM simply ignored it (several are
/// strict about the timestamp/source words, others don't honor it at all), leaving nothing
/// closed and nothing to show why. The direct message removes that middleman; the EWMH route
/// remains only for the rare window that doesn't advertise `WM_DELETE_WINDOW` in its
/// `WM_PROTOCOLS`, where the WM's own (usually harsher) close is all there is.
///
/// Every send is checked, so a bad window id or a dead display comes back as an error rather
/// than as a silently-ignored request.
fn x11_close(ids: &[u32]) -> Result<(), String> {
    use x11rb::protocol::xproto::{ClientMessageEvent, EventMask};
    let (conn, screen) = x11rb::connect(None).map_err(|_| "no X11 display".to_string())?;
    let root = conn.setup().roots[screen].root;
    let atom = |name: &[u8]| -> Result<u32, String> {
        conn.intern_atom(false, name)
            .map_err(|err| format!("X connection failed: {err}"))?
            .reply()
            .map(|reply| reply.atom)
            .map_err(|err| format!("X connection failed: {err}"))
    };
    let wm_protocols = atom(b"WM_PROTOCOLS")?;
    let wm_delete = atom(b"WM_DELETE_WINDOW")?;
    let net_close = atom(b"_NET_CLOSE_WINDOW")?;
    for &id in ids {
        let sent = if _accepts(&conn, id, wm_protocols, wm_delete) {
            // ICCCM: straight to the client. Mask NO_EVENT delivers to the window's owner.
            let event = ClientMessageEvent::new(32, id, wm_protocols, [wm_delete, 0, 0, 0, 0]);
            conn.send_event(false, id, EventMask::NO_EVENT, event)
        } else {
            // EWMH via the WM: data is (timestamp, source); source 2 = direct user action.
            let event = ClientMessageEvent::new(32, id, net_close, [0, 2, 0, 0, 0]);
            conn.send_event(
                false,
                root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                event,
            )
        };
        sent.map_err(|err| format!("could not send the close request: {err}"))?
            .check()
            .map_err(|err| format!("the close request was rejected: {err}"))?;
    }
    conn.flush().map_err(|err| format!("could not send the close request: {err}"))?;
    Ok(())
}

/// Whether `window` lists `protocol` in its `WM_PROTOCOLS` — i.e. has asked to be TOLD about
/// this kind of event rather than have it done to it.
fn _accepts(conn: &impl Connection, window: u32, wm_protocols: u32, protocol: u32) -> bool {
    conn.get_property(false, window, wm_protocols, AtomEnum::ATOM, 0, u32::MAX)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .and_then(|reply| Some(reply.value32()?.collect::<Vec<_>>()))
        .is_some_and(|protocols| protocols.contains(&protocol))
}

/// Move the focused window's opacity to `next(now)` — the caller's stepping rule, handed the
/// opacity it has now — and say where it ended up. EWMH's `_NET_WM_WINDOW_OPACITY` carries it:
/// what X11 compositors (picom, xfwm4, Mutter, KWin) draw a window's opacity from; a window
/// without the property is opaque.
///
/// The FOCUSED window: the terminal the command was typed into, as every window-level opacity
/// control acts on (`_NET_ACTIVE_WINDOW`, the client window itself, which is where compositors
/// read the property). And only under a compositor — one owns the `_NET_WM_CM_S<screen>`
/// selection — since without one the property changes nothing on screen: that is an error, not
/// a silent success.
pub(crate) fn step_opacity(next: impl FnOnce(f64) -> f64) -> Result<f64, String> {
    use x11rb::protocol::xproto::PropMode;
    use x11rb::wrapper::ConnectionExt as _;
    let (conn, screen) = x11rb::connect(None).map_err(|_| "no X11 display".to_string())?;
    let root = conn.setup().roots[screen].root;
    let atom = |name: &str| -> Result<u32, String> {
        conn.intern_atom(false, name.as_bytes())
            .map_err(|err| format!("X connection failed: {err}"))?
            .reply()
            .map(|reply| reply.atom)
            .map_err(|err| format!("X connection failed: {err}"))
    };
    let compositor = conn
        .get_selection_owner(atom(&format!("_NET_WM_CM_S{screen}"))?)
        .map_err(|err| format!("X connection failed: {err}"))?
        .reply()
        .map_err(|err| format!("X connection failed: {err}"))?
        .owner;
    if compositor == x11rb::NONE {
        return Err("no compositor is running, so a window's opacity would not show".into());
    }
    let window = _window_property(&conn, root, atom("_NET_ACTIVE_WINDOW")?)
        .filter(|&window| window != x11rb::NONE)
        .ok_or("the window manager names no focused window (_NET_ACTIVE_WINDOW)")?;
    let opacity = atom("_NET_WM_WINDOW_OPACITY")?;
    let next = next(_cardinal(&conn, window, opacity).map_or(1.0, opacity_of));
    conn.change_property32(PropMode::REPLACE, window, opacity, AtomEnum::CARDINAL, &[cardinal_of(next)])
        .map_err(|err| format!("could not set the window's opacity: {err}"))?
        .check()
        .map_err(|err| format!("the window's opacity was refused: {err}"))?;
    conn.flush().map_err(|err| format!("could not set the window's opacity: {err}"))?;
    Ok(next)
}

/// `_NET_WM_WINDOW_OPACITY` as a fraction: the property spans all of `u32`, 0 clear and
/// `u32::MAX` opaque.
fn opacity_of(cardinal: u32) -> f64 {
    f64::from(cardinal) / f64::from(u32::MAX)
}

/// [`opacity_of`]'s inverse, for a fraction in `0..=1`.
fn cardinal_of(opacity: f64) -> u32 {
    (opacity.clamp(0.0, 1.0) * f64::from(u32::MAX)).round() as u32
}

/// A window's single WINDOW-typed property (`_NET_ACTIVE_WINDOW` on the root), if it has one.
fn _window_property(conn: &impl Connection, window: u32, property: u32) -> Option<u32> {
    conn.get_property(false, window, property, AtomEnum::WINDOW, 0, 1)
        .ok()?
        .reply()
        .ok()?
        .value32()?
        .next()
}

/// A window's single CARDINAL property, if it has one.
fn _cardinal(conn: &impl Connection, window: u32, property: u32) -> Option<u32> {
    conn.get_property(false, window, property, AtomEnum::CARDINAL, 0, 1)
        .ok()?
        .reply()
        .ok()?
        .value32()?
        .next()
}

/// A window's text property, read leniently: titles are for eyes, so a stray non-UTF-8 byte
/// degrades to the replacement character rather than discarding the window.
fn _text(conn: &impl Connection, window: u32, property: u32, kind: u32) -> Option<String> {
    let reply = conn.get_property(false, window, property, kind, 0, u32::MAX).ok()?.reply().ok()?;
    (reply.format == 8 && !reply.value.is_empty())
        .then(|| String::from_utf8_lossy(&reply.value).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars<'a>(set: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| set.iter().find(|(var, _)| *var == name).map(|(_, value)| value.to_string())
    }

    /// The session decides the source: an X11 session is said so outright, or is a display with
    /// no Wayland one beside it — Xwayland's display is no X11 session, so KWin is asked there.
    #[test]
    fn an_x11_session_is_said_so_or_a_display_without_wayland() {
        assert!(x11_session(&vars(&[("XDG_SESSION_TYPE", "x11")])));
        assert!(x11_session(&vars(&[("DISPLAY", ":0")])), "a display, no Wayland");
        assert!(!x11_session(&vars(&[("DISPLAY", ":0"), ("WAYLAND_DISPLAY", "wayland-0")])), "XWayland is no X11 session");
        assert!(!x11_session(&vars(&[("XDG_SESSION_TYPE", "wayland")])));
        assert!(!x11_session(&vars(&[])), "no display at all");
    }

    /// One rule for both sources: a window is a candidate only with an owner and a title.
    #[test]
    fn a_window_needs_an_owner_and_a_title_to_be_a_candidate() {
        let kept = named(WindowId::KWin("{1}".into()), Some(42), Some("Untitled — Kate".into())).expect("both");
        assert_eq!((kept.id, kept.pid, kept.title.as_str()), (WindowId::KWin("{1}".into()), 42, "Untitled — Kate"));
        assert!(named(WindowId::X11(7), None, Some("xeyes".into())).is_none(), "no owner");
        assert!(named(WindowId::X11(7), Some(42), Some(String::new())).is_none(), "an empty title");
        assert!(named(WindowId::X11(7), Some(42), None).is_none(), "no title");
    }

    /// A close request goes back to whoever listed the window, each side in the caller's order.
    #[test]
    fn ids_split_by_who_listed_them() {
        let ids = [WindowId::KWin("{2}".into()), WindowId::X11(9), WindowId::KWin("{1}".into()), WindowId::X11(3)];
        assert_eq!(by_source(&ids), (vec![9, 3], vec!["{2}".to_string(), "{1}".to_string()]));
        assert_eq!(by_source(&[]), (vec![], vec![]));
    }

    /// The property's scale end to end: its two extremes are opaque and clear, a fraction comes
    /// back within the property's resolution, and out of range is clamped rather than wrapped.
    #[test]
    fn window_opacity_spans_the_whole_cardinal_range() {
        assert_eq!(cardinal_of(1.0), u32::MAX);
        assert_eq!(cardinal_of(0.0), 0);
        assert_eq!(opacity_of(u32::MAX), 1.0);
        assert_eq!(opacity_of(0), 0.0);
        for opacity in [0.05, 0.5, 0.85] {
            assert!((opacity_of(cardinal_of(opacity)) - opacity).abs() < 1e-9, "{opacity}");
        }
        assert_eq!(cardinal_of(1.5), u32::MAX);
        assert_eq!(cardinal_of(-0.5), 0);
    }
}
