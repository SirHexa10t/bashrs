//! Commands backed by my other repos, imported as dependencies. Each is exposed
//! under the external tool's own name (`table`) or a task-named family wrapping its
//! subcommands (`backup_*` over filesync) — all `#[unprefixed]`, and each reuses the
//! project's own argument structs, so the args are defined once, upstream.

#[bashrs_macros::category(command = ComfyReposCommand, prefix = "comfy_")]
mod commands {
    use crate::support::comfy_repos::table_fancy_options;
    use clap::Args;
    use std::io::{self, BufWriter, Write};

    /// Align whitespace-delimited columns into a neat table (table_formatter)
    #[unprefixed]
    pub fn table(args: table_formatter::Args) {
        if let Err(err) = table_formatter::run_with(args) {
            eprintln!("table: {err}");
        }
    }

    /// `table` in its framed, terminal-width form: `-j " | " --split-lines --space-rows '-'
    /// --emit-frame` — pipe-joined columns in a border, records ruled apart, wide rows wrapped to
    /// fit the window (table_formatter)
    #[unprefixed]
    pub fn table_fancy(args: TableFancyArgs) {
        if let Err(err) = _table_fancy(&args.input) {
            eprintln!("table_fancy: {err}");
        }
    }

    /// Read `input`, format it with the shared [`table_fancy_options`] preset, and print it. The
    /// pinned flags are *not* exposed as arguments (unlike the `backup_*` pattern,
    /// `table_formatter::Args` keeps its fields private), so the preset — not a mutated arg set —
    /// is what this command and `doc_render` share.
    fn _table_fancy(input: &str) -> io::Result<()> {
        let lines = table_formatter::read_lines(input)?;
        let table = table_formatter::format_table(&lines, &table_fancy_options())
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
        // One locked, buffered writer for the whole table — mirroring table_formatter's own
        // printer, where a per-line `println!` would take the lock and syscall for every line.
        let mut out = BufWriter::new(io::stdout().lock());
        for line in &table {
            writeln!(out, "{line}")?;
        }
        out.flush()
    }

    /// What `table_fancy` takes: `table`'s positional input, verbatim — the rest of its shape is
    /// the pinned preset.
    #[derive(Args)]
    pub struct TableFancyArgs {
        /// Input file path / data (or use stdin if not provided)
        #[arg(default_value = "-")]
        input: String,
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
