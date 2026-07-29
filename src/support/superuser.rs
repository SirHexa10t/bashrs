//! One anchor for privilege escalation — the elevation command and the helpers around it.
//! Everything that runs something as root goes through here, so switching tools (e.g. to `doas`)
//! is a one-line change to [`CMD`] instead of a hunt for every literal `sudo`.
//!
//! Two callers, only one of them visible to a search: `packages_*` names this module directly, and
//! every `#[bashrs_macros::elevated]` routine calls [`command`]/[`revoke`] from its generated
//! `reexec`. The macro reaches them through [`crate::elevation`] — the crate-root alias that exists
//! so this module's own path isn't baked into the macro crate.

use std::process::Command;

/// The privilege-escalation command — the single source of truth for the keyword. Swap this (and,
/// if the new tool spells it differently, [`revoke`]) to change the escalation tool everywhere.
pub(crate) const CMD: &str = "sudo";

/// A fresh [`Command`] that runs its (later-added) program as the superuser — i.e. `sudo …`.
pub(crate) fn command() -> Command {
    Command::new(CMD)
}

/// Drop any cached elevation (`sudo -k`), so it can't carry over to a later command.
pub(crate) fn revoke() {
    let _ = command().arg("-k").status();
}
