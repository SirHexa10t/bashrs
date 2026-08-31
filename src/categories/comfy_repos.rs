//! Commands backed by my other repos, imported as dependencies. Each is exposed under the
//! external tool's own name (`table`), a task-named family wrapping its subcommands (`backup_*`
//! over filesync), or the verb it has always had here (`dl`, over the `vidl` crate) — all
//! `#[unprefixed]`, and each reuses the upstream argument structs where it can, so the args are
//! defined once, upstream.
//!
//! `dl` carries a little more of its own weight than the others: injecting bashrs's pinned tool
//! paths, pinning where imported cookies are kept, and the supported-sites page stay on this side
//! of the boundary. Acquiring the cookies themselves does not — that moved into `vidl`, which owns
//! the store it reads.

#[bashrs_macros::category(command = ComfyReposCommand, prefix = "comfy_")]
mod commands {
    use crate::support::args::NoArgs;
    use crate::support::comfy_repos::table_fancy_options;
    use crate::support::doc_render;
    use clap::Args;
    use std::io::{self, BufWriter, Write};
    use std::path::{Path, PathBuf};

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

    /// Hold or toggle a key (default F9) to click — or to repeat a keyboard key — at a chosen
    /// rate, wherever the mouse already is; F8 quits. Works on X11, Wayland and the console
    /// alike; on X11 it needs no privileges at all. If device access is needed and missing,
    /// the error walks you through the fix (sequencer clicker)
    #[unprefixed]
    #[alias("autoclick")]
    pub fn clicker(args: sequencer::ClickerArgs) {
        _sequencer(sequencer::Command::Clicker(args));
    }

    /// Measure what this machine can really deliver: writes key presses as fast as asked
    /// (or flat out, with no `--cps`) while reading its own virtual device back, printing the
    /// live rate as it goes. The gap between written and delivered is the honest ceiling —
    /// a rate counted from the sending loop alone would never show it (sequencer bench)
    #[unprefixed]
    pub fn clicker_benchmark(args: sequencer::BenchArgs) {
        _sequencer(sequencer::Command::Bench(args));
    }

    /// How *this* shell spells sequencer's `doctor` subcommand.
    ///
    /// Unlike the wrapper comments in `sourcefile.sh` — which are each command's own clap `about`,
    /// read straight from the doc comment above it — this cannot be derived. Upstream prints
    /// "To stop being asked, `…` prints a one-time setup" from inside its own crate, and has no
    /// way to know the subcommand it calls `doctor` is reached here as [`autokey_doctor`]. So the
    /// name is passed in, and a test asserts it still resolves to a real command.
    pub(crate) const DOCTOR_COMMAND: &str = "autokey_doctor";

    /// Check everything the synthetic-input commands need — the uinput module loaded,
    /// /dev/uinput writable, /dev/input readable — and print the exact commands to fix whatever
    /// is missing. One check for the whole family: `clicker` and the `autokey_*` profile
    /// commands open the same devices. Exit 0 means ready (sequencer doctor)
    #[unprefixed]
    pub fn autokey_doctor(args: sequencer::DoctorArgs) {
        _sequencer(sequencer::Command::Doctor(args));
    }

    /// `~/.bashrs/user-data/sequencer` — the profile store: where this shell keeps binds
    /// profiles. Deliberately nothing more than a well-known directory — sequencer takes real
    /// paths only, no command resolves bare names against this behind the user's back, and TAB
    /// completion doesn't offer its contents (they aren't in the PWD; the full-path listing is
    /// `autokey_list_bashrs_profiles`'s job). It is what a bare `autokey_apply` passes and what
    /// that listing walks; the same division as [`_cookie_store_dir`] — where bashrs keeps its
    /// data is bashrs's own business. `install-shell` creates it.
    fn _profile_store_dir() -> PathBuf {
        crate::conf::user_data_dir().join("sequencer")
    }

    /// Check binds files for problems without applying them: nothing is activated, no profile
    /// state is touched, and the files are left exactly as they are. `autokey_reformat` is the
    /// same check with tidying turned on (sequencer profile-check)
    #[unprefixed]
    pub fn autokey_check(args: sequencer::ProfileCheckArgs) {
        _sequencer(sequencer::Command::ProfileCheck(args));
    }

    /// `autokey_check` that also rewrites: every file that parses is tidied in place with all its
    /// comments kept, and one that doesn't is reported and left alone. Still activates nothing —
    /// the rewrite is the only thing it changes (sequencer profile-check --format)
    #[unprefixed]
    pub fn autokey_reformat(mut args: sequencer::ProfileCheckArgs) {
        // Always tidy what parses: a plain bool, so a caller passing it anyway is a no-op
        // (hidden from this command's help/completion — see `cli::HIDDEN_PINNED`).
        args.format = true;
        _sequencer(sequencer::Command::ProfileCheck(args));
    }

    /// Apply binds profiles: remap keys and run sequences until stopped. Each FILE is linked into
    /// the active set (naming a directory takes every `.toml` directly inside it, in name order);
    /// naming nothing applies the profile store, ~/.bashrs/user-data/sequencer. The first
    /// invocation becomes the manager and later ones add to it (sequencer profile-apply)
    #[unprefixed]
    pub fn autokey_apply(mut args: sequencer::ProfileApplyArgs) {
        // Upstream requires at least one FILE; this shell relaxes that (`cli::OPTIONAL_ARGS`)
        // and answers the bare invocation itself: the whole store, as an ordinary directory
        // argument through upstream's own expansion — which also supplies the error when the
        // store is empty. Explicit arguments pass through untouched, real paths like anywhere.
        if args.files.is_empty() {
            args.files = vec![_profile_store_dir()];
        }
        _sequencer(sequencer::Command::ProfileApply(args));
    }

    /// List the profile store (~/.bashrs/user-data/sequencer): every binds profile in it,
    /// RECURSIVELY, and every directory — `autokey_apply` expands a directory non-recursively
    /// on purpose, so each directory is its own applyable unit and earns its own line (a
    /// trailing `/`). Full paths, made to be copied straight onto an `autokey_apply` line:
    /// the apply commands take real paths only, so the listing hands out exactly those.
    /// Applied entries are marked, plus anything applied from outside the store, with the
    /// path it came from. `_bashrs_` because the store is this shell's: sequencer applies
    /// whatever files it is given and has no drawer of its own
    #[unprefixed]
    pub fn autokey_list_bashrs_profiles(_args: NoArgs) {
        let store = _profile_store_dir();
        // Applied links resolve to CANONICAL targets (sequencer's scan), so stored files are
        // matched by canonical path — two same-named profiles in different folders can't
        // vouch for each other the way name-matching would let them.
        let applied = sequencer::applied_profiles();
        let is_applied = |path: &Path| {
            std::fs::canonicalize(path)
                .is_ok_and(|canonical| applied.iter().any(|(_, target)| *target == canonical))
        };
        let mut lines: Vec<String> = Vec::new();
        let mut matched: Vec<PathBuf> = Vec::new(); // store files found applied, canonical
        let mut pending = vec![store.clone()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                let shown = path.display().to_string();
                if path.is_dir() {
                    lines.push(format!("{shown}/"));
                    pending.push(path);
                } else if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("toml")) {
                    match is_applied(&path) {
                        true => {
                            lines.push(format!("{shown}  [applied]"));
                            matched.extend(std::fs::canonicalize(&path));
                        }
                        false => lines.push(shown),
                    }
                }
            }
        }
        if lines.is_empty() {
            println!(
                "no profiles in {} — drop binds .toml files there (see `autokey_check`)",
                store.display()
            );
        }
        lines.sort(); // paths sort a directory's line right above its contents
        for line in &lines {
            println!("{line}");
        }
        // Applied out of somewhere else entirely — still part of "what is live right now".
        for (name, target) in &applied {
            if !matched.contains(target) {
                println!("{name}  [applied from {}]", target.display());
            }
        }
    }

    /// Remove profiles from the active set by name (`gaming` or `gaming.toml`); naming none opens
    /// an interactive picker listing what is currently applied (sequencer profile-unapply)
    #[unprefixed]
    pub fn autokey_unapply(args: sequencer::ProfileUnapplyArgs) {
        _sequencer(sequencer::Command::ProfileUnapply(args));
    }

    /// Print the name of each key as it is pressed, once per press — how to learn the key names a
    /// binds profile expects. Reads the input devices, so it is exact; `-n` reads the terminal
    /// instead, needing no privileges but seeing only keys that type something
    /// (sequencer detect-key)
    #[unprefixed]
    pub fn autokey_detect(args: sequencer::DetectKeyArgs) {
        _sequencer(sequencer::Command::DetectKey(args));
    }

    /// Run one sequencer invocation — the funnel for the `sequencer`-backed commands, each
    /// wrapping one subcommand (flag completion per command; sequencer never runs bare), the
    /// same shape as [`_filesync`]. Logging must be initialized first, exactly as the upstream
    /// binary's `main` does — the sent-rate report, the chosen backend, and drop warnings all
    /// ride tracing, so skipping it would run the clicker mute.
    ///
    /// Entered through `run_with_sudo_prompt` — upstream's session mode — though on an X11
    /// session it never asks: clicks go through XTEST and hotkeys through key grabs, so no
    /// input device is opened and there is nothing for sudo to unlock. Elsewhere (Wayland, a
    /// console) an unprivileged run without the udev/group setup explains itself, asks once,
    /// re-execs `bashrs <command> …` verbatim under sudo, and the elevated process sheds root
    /// the moment the devices are open. It is handed [`DOCTOR_COMMAND`] so the advice it prints
    /// names something this shell can actually run.
    fn _sequencer(command: sequencer::Command) {
        let global = command.global();
        let _ = sequencer::init_logging(global.verbose, global.quiet);
        let code = sequencer::run_with_sudo_prompt(&command.into(), DOCTOR_COMMAND);
        if code != 0 {
            std::process::exit(code.into());
        }
    }

    /// Run one filesync invocation — every `backup_*` command funnels here, each wrapping one
    /// subcommand (which gives each its own flag completion; filesync never runs bare). Entered
    /// through `run_with_sudo_prompt`, not plain `run`, so `backup_*` starts exactly like the
    /// standalone binary: an interactive unprivileged run asks for sudo once, up front (the
    /// re-exec re-runs `bashrs backup_… <args>` verbatim, so the round trip lands back here as
    /// root and proceeds); `--unelevated`, already-root, and tty-less runs are never prompted.
    /// filesync reports its own errors; the exit code is surfaced so `backup_sync … && next`
    /// composes.
    fn _filesync(command: filesync::Command) {
        let code = filesync::run_with_sudo_prompt(filesync::Cli { command });
        if code != 0 {
            std::process::exit(code.into());
        }
    }

    // --- dl ------------------------------------------------------------------

    /// `~/.bashrs/user-data/browser_cookies` — the base holding one subdir per imported site.
    /// `dl` pins this: the store's *shape* and every operation on it belong to `vidl`, but where
    /// bashrs keeps its data is bashrs's own business. So `vidl`'s two-value
    /// `--cookies-extract-for-domain <TARGET> <OUTPUT-DIR>` is narrowed here to the target alone
    /// (`crate::cli::NARROWED_ARGS`) and this supplies the rest.
    fn _cookie_store_dir() -> PathBuf {
        crate::conf::user_data_dir().join("browser_cookies")
    }

    /// The bundled copy of `name`, when bashrs actually bundles it. `tools::resolve` answers with
    /// the bare name when there's no bundle, which is the signal to let the tool be found on PATH
    /// instead — so `None` here means "no pinned copy; use the normal lookup".
    fn _bundled(name: &str) -> Option<PathBuf> {
        let resolved = crate::tools::resolve(name);
        (resolved != *name).then(|| PathBuf::from(resolved))
    }

    /// Download a video with the bundled yt-dlp. A YouTube URL (`youtube.com`, `youtu.be`, …)
    /// takes the full path — per-video subtitle selection, the playlist unplayable-report,
    /// channel tabs sorted into folders; any other site yt-dlp supports takes a generic path —
    /// one video downloaded flat into `--into`, the same quality knobs and failure ledger, minus
    /// the folder trees (a generic page gives no channel/playlist structure to build them from).
    /// `-c` lists what "any other site" tends to cover. The machinery lives in
    /// the [`vidl`] crate; this stays the thin argument shell.
    #[name("dl")]
    pub fn dl(args: DlArgs) {
        let DlArgs { mut vidl, compatibility_help } = args;
        if compatibility_help {
            print!("{}", _compatibility_help());
            return;
        }
        if vidl.url.is_none() && !vidl.taglist && vidl.cookies_extract_for_domain.is_none() {
            eprintln!("dl: a URL is required (or -t, -c, or --cookies-extract-for-domain)");
            std::process::exit(2);
        }
        // Hand vidl the copies bashrs bundles and version-pins. Without this it would find
        // whatever is on PATH — which is the right default for a standalone user, and exactly
        // what the pinning exists to avoid here. The python matters twice over: it runs the
        // cookie filter's sqlite work, so an extract never depends on a system interpreter.
        vidl::tools::install(vidl::tools::Tools {
            ytdlp: _bundled("yt-dlp").map(PathBuf::into_os_string),
            python: _bundled("python3").map(PathBuf::into_os_string),
            ffmpeg_dir: _bundled("ffmpeg").and_then(|bin| bin.parent().map(Path::to_path_buf)),
            js_runtime: _bundled("deno"),
        });
        // Imported cookies live under bashrs's data dir, not vidl's per-user default — the one
        // download option bashrs decides for itself. Set unconditionally, not just on an import: a
        // store is READ on every run, and a root known only while importing would be found once
        // and then lost.
        vidl.cookie_root = Some(_cookie_store_dir());
        let code = vidl::run(vidl);
        if code != 0 {
            std::process::exit(code);
        }
    }

    #[derive(Args)]
    pub struct DlArgs {
        #[command(flatten)]
        pub vidl: vidl::Args,
        /// Print the kinds of sites yt-dlp tends to support (like --help, but for site coverage)
        #[arg(short = 'c', long)]
        pub compatibility_help: bool,
    }

    /// The `dl -c` site-coverage listing — a Markdown doc embedded at compile time (long enough
    /// to earn its own file, and natural to author/read as Markdown). Rendered for the terminal
    /// by [`doc_render::render_doc`]; edit the `.md` to revise it.
    const COMPATIBILITY: &str = include_str!("templates/yt-dlp_compatibility.md");

    /// What `dl -c` prints: [`COMPATIBILITY`] rendered to ANSI-coloured text by
    /// [`doc_render::render_doc`] — markers stripped, headings/emphasis/code/lists/quotes coloured
    /// via the shared theme, prose passed through line-for-line, and the pipe tables re-laid out
    /// `table_fancy`-style to fit the terminal.
    fn _compatibility_help() -> String {
        doc_render::render_doc(COMPATIBILITY)
    }

    #[cfg(test)]
    mod autokey_tests {
        use super::*;

        /// The Rust half of the store-location pin; the shell half (the completer's
        /// `$HOME/.bashrs/user-data/sequencer` glob) is pinned in `cli::sourcefile`'s tests.
        /// Either moving without the other turns exactly one of the two red.
        #[test]
        fn the_profile_store_sits_in_user_data() {
            assert_eq!(_profile_store_dir(), crate::conf::user_data_dir().join("sequencer"));
        }
    }

    #[cfg(test)]
    mod dl_tests {
        use super::*;

        #[test]
        fn compatibility_doc_renders_within_the_shared_invariants() {
            // The rendering guarantees (colour; prose 1:1; framed, window-bounded tables; no
            // leaked markers) are pinned once in `doc_render::assert_render_invariants` — here
            // the REAL doc, emoji, CJK and long rows included, is run through them.
            doc_render::assert_render_invariants(COMPATIBILITY);
        }

    }
}
