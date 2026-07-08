pub mod categories;
pub mod cli;
pub mod shell_conf;
pub mod support;

/// Binary entry point. `gg`'s permission-denied recovery re-execs the binary under `sudo` to
/// re-scan the unreadable paths as root; that root process lands here first — recognised by its
/// `argv` marker and handled before clap (it's an internal re-run, not a user command), via the
/// `#[elevated]`-generated [`GgElevatedRescan`](crate::categories::lookup::GgElevatedRescan). Any
/// ordinary invocation falls through to the parsed-and-dispatched CLI.
pub fn run() {
    if crate::categories::lookup::GgElevatedRescan::try_handle() {
        return;
    }
    <cli::Cli as clap::Parser>::parse().command.run();
}
