//! One anchor for privilege escalation — the elevation command and the helpers around it.
//! Everything that runs something as root goes through here, so switching tools (e.g. to `doas`)
//! is a one-line change to [`CMD`] instead of a hunt for every literal `sudo`.
//!
//! Two callers, only one of them visible to a search: `packages_*` names this module directly, and
//! every `#[bashrs_macros::elevated]` routine calls [`is_root`]/[`ticket_exists`]/[`command`]/
//! [`revoke_ours`] from its generated `reexec`. The macro reaches them through
//! [`crate::elevation`] — the crate-root alias that exists so this module's own path isn't baked
//! into the macro crate.
//!
//! Revocation is guarded, not reflexive: [`ticket_exists`] is probed *before* elevating, and
//! [`revoke_ours`] clears the cached credential only when that probe said there was none — a
//! ticket the user held before we started is part of *their* sudo workflow (`sudo a; bashrs …;
//! sudo b`), and killing it would make their next command re-prompt over a run that never even
//! asked them for a password.

use std::process::Command;

/// The privilege-escalation command — the single source of truth for the keyword. Swap this (and,
/// if the new tool spells it differently, [`revoke`]) to change the escalation tool everywhere.
pub(crate) const CMD: &str = "sudo";

/// A fresh [`Command`] that runs its (later-added) program as the superuser — i.e. `sudo …`.
pub(crate) fn command() -> Command {
    Command::new(CMD)
}

/// Whether this process already runs as the superuser. Inside a root shell (`bashrsudo`,
/// `sudo -s`) elevation is a fact, not a request: no re-exec, no prompt, no ticket to manage.
pub(crate) fn is_root() -> bool {
    // SAFETY: geteuid reads a process credential; it has no failure modes and touches no memory.
    unsafe { libc::geteuid() == 0 }
}

/// Whether the user already holds a live elevation ticket on this terminal (`sudo -n true`).
/// Probe it BEFORE elevating — afterwards there is a ticket either way, and no way to tell
/// whose it is.
pub(crate) fn ticket_exists() -> bool {
    command()
        .args(["-n", "true"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Drop the cached elevation this run earned (`sudo -k`) — unless [`ticket_exists`] said the
/// user already held one, which is theirs to keep.
pub(crate) fn revoke_ours(had_ticket: bool) {
    if !had_ticket {
        let _ = command().arg("-k").status();
    }
}
