//! What's on the screen: the X11 window list, each with its title and owning process.
//!
//! In-process over `x11rb` — already in the dependency tree for sequencer's X11 backends — rather
//! than shelling to `wmctrl`/`xdotool`, which may not be installed and whose output would need
//! parsing back apart. This module only *enumerates*; who wants which window is the caller's
//! question.
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
//! fallback for a server without the extension.

use x11rb::connection::Connection;
use x11rb::protocol::res::{self, ClientIdMask, ClientIdSpec};
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

/// One mapped window that names its owner.
pub(crate) struct Window {
    /// The X window id — the handle a close request is addressed to.
    pub(crate) id: u32,
    /// The owning process.
    pub(crate) pid: u32,
    /// Its title: `_NET_WM_NAME` (UTF-8), or the older `WM_NAME` when that's all there is. For a
    /// browser this carries the ACTIVE tab's title — background tabs are not windows.
    pub(crate) title: String,
}

/// Every window the window manager lists, with the PID and title of each. `Err` is a
/// human-readable reason there's nothing to enumerate (no display, no EWMH window manager).
pub(crate) fn list() -> Result<Vec<Window>, String> {
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
        // matched to a request. Either way it silently isn't a candidate.
        let Some(pid) = _server_pid(&conn, id).or_else(|| _cardinal(&conn, id, net_wm_pid)) else {
            continue;
        };
        let title = _text(&conn, id, net_wm_name, utf8_string)
            .or_else(|| _text(&conn, id, AtomEnum::WM_NAME.into(), AtomEnum::ANY.into()));
        let Some(title) = title.filter(|title| !title.is_empty()) else { continue };
        windows.push(Window { id, pid, title });
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
pub(crate) fn close(ids: &[u32]) -> Result<(), String> {
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

/// Which of `ids` are still on screen. Errors (display gone mid-check) read as "none left" —
/// the caller only uses this to decide whether to keep waiting.
pub(crate) fn still_open(ids: &[u32]) -> Vec<u32> {
    match list() {
        Ok(all) => {
            let open: std::collections::BTreeSet<u32> = all.into_iter().map(|w| w.id).collect();
            ids.iter().copied().filter(|id| open.contains(id)).collect()
        }
        Err(_) => Vec::new(),
    }
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
