//! bashrs — a Rust-based bashrc: the subcommands of this binary become the user's shell functions
//! via `bashrs generate` (run by `COMPILE.SH`, which writes `~/.bashrs/sourcefile.sh`).
//!
//! The crate is layered; **imports must only point left**:
//!
//! ```text
//! support  <     conf      <     tools     <    drivers    <  categories, cli
//!            (configrs.toml,  (fetch reads   (what commands
//!             exports, keys,   the config;    the tools: yt-dlp,
//!             greeting)        tool shims)    the python env,
//!                                             the companion repos)
//! ```
//!
//! Higher layers never leak downward. Instead, each subsystem *contributes* its shell surface as
//! strings — categories their command wrappers, [`tools`] its PATH line and companion aliases,
//! [`categories::session`] its raw-shell commands — and [`cli`] assembles them all into the
//! sourcefile. `tests/layering.rs` enforces the arrow directions.

pub mod categories;
pub mod cli;
pub mod conf;
pub mod drivers;
pub mod support;
pub mod tools;

use crate::categories::lookup::GgElevatedRescan;

/// The one in-crate path the `#[bashrs_macros::elevated]` macro names by string: its expansion
/// emits `crate::elevation::command()` / `::revoke()`. That edge crosses a crate boundary as text,
/// so no compiler sees it until the macro expands — this alias is the single declared anchor for
/// it. **Moving [`support::superuser`] means re-pointing this line, and nothing else**; without it,
/// the module's own path would be part of the macro's contract and a rename would fail inside a
/// generated block, pointing at the wrong file.
pub(crate) use support::superuser as elevation;

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
