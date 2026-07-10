//! Commands backed by my other repos, imported as dependencies. Each is exposed
//! under the external tool's own name (`table`) or a task-named family wrapping its
//! subcommands (`backup_*` over filesync) — all `#[unprefixed]`, and each reuses the
//! project's own argument structs, so the args are defined once, upstream.

#[bashrs_macros::category(command = ComfyReposCommand, prefix = "comfy_")]
mod commands {
    /// Align whitespace-delimited columns into a neat table (table_formatter)
    #[unprefixed]
    pub fn table(args: table_formatter::Args) {
        if let Err(err) = table_formatter::run_with(args) {
            eprintln!("table: {err}");
        }
    }

    /// Report what a backup sync would do (new / changed / moved / deleted); changes nothing (filesync diff)
    #[unprefixed]
    pub fn backup_diff(args: filesync::cli::DiffArgs) {
        _filesync(filesync::Command::Diff(args));
    }

    /// Make DEST mirror SOURCE: copy new/changed, rename moves, delete extras; resumable (filesync sync)
    #[unprefixed]
    pub fn backup_sync(args: filesync::cli::SyncArgs) {
        _filesync(filesync::Command::Sync(args));
    }

    /// Verify a mirror by content: `backup_diff` comparing every file's bytes (blake3), so silent
    /// corruption can't hide behind a matching size+mtime (filesync diff --eager-checksum)
    #[unprefixed]
    pub fn backup_find_bitrot(mut args: filesync::cli::DiffArgs) {
        // Force the content comparison; a plain bool, so a caller passing it anyway is a no-op
        // (hidden from this command's help/completion — see `cli::HIDDEN_PINNED`).
        args.common.eager_checksum = true;
        _filesync(filesync::Command::Diff(args));
    }

    /// Run one filesync invocation — every `backup_*` command funnels here, each wrapping one
    /// subcommand (which gives each its own flag completion; filesync never runs bare). `run`
    /// reports its own errors; the exit code is surfaced so `backup_sync … && next` composes.
    fn _filesync(command: filesync::Command) {
        let code = filesync::run(filesync::Cli { command });
        if code != 0 {
            std::process::exit(code.into());
        }
    }
}
