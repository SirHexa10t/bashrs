pub mod categories;
pub mod cli;
pub mod internal_cli;
pub mod shell_conf;
pub mod support;

/// Binary entry point. `gg` re-execs itself under `sudo` to re-scan permission-denied paths; that
/// root process lands here first, so it's handled before clap (it's an internal re-run, not a
/// command) and then we return. Otherwise the CLI is parsed and dispatched normally.
pub fn run() {
    if internal_cli::handle() {
        return;
    }
    <cli::Cli as clap::Parser>::parse().command.run();
}
