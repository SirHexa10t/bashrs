pub mod categories;
pub mod cli;
pub mod shell_conf;
pub mod support;

use crate::categories::lookup::GgElevatedRescan;

/// The binary's internal self-invocations: the `sudo` re-execs that `#[elevated]` routines spawn to
/// run part of themselves as root. Each routine's `try_handle` — which claims the run iff its marker
/// leads `argv` — is listed here; [`run`] offers the process to them before the user CLI parses.
/// Adding an `#[elevated]` routine adds one entry; nothing else changes.
const REEXEC_HANDLERS: &[fn() -> bool] = &[GgElevatedRescan::try_handle];

/// Binary entry point. An internal re-exec handles itself and stops here; every other invocation is
/// a real command, parsed and dispatched normally.
pub fn run() {
    if REEXEC_HANDLERS.iter().any(|handle| handle()) {
        return;
    }
    cli::parse().command.run();
}
