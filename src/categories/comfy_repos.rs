//! Commands backed by my other repos, imported as dependencies. Each is exposed under the
//! external tool's own name (`table`), a task-named family wrapping its subcommands (`backup_*`
//! over filesync), a task name of its own (`symlink_devs`, over devmounts' `apply`), or the verb
//! it has always had here (`dl`, over the `vidl` crate, with `dl_fix` over its `repair`) — each
//! named outright with `#[name]`, being an alias of a repo rather than a command of a bashrs
//! category (whose prefix `#[unprefixed]` drops), and each reusing the upstream argument structs
//! where it can, so the args are defined once, upstream.
//!
//! `dl` and `dl_fix` carry a little more of their own weight than the others: injecting bashrs's
//! pinned tool paths, pinning where imported cookies are kept, and `dl`'s supported-sites page
//! stay on this side of the boundary. Acquiring the cookies themselves does not — that moved into
//! `vidl`, which owns the store it reads.

#[bashrs_macros::category(command = ComfyReposCommand, prefix = "comfy_")]
mod commands {
    use crate::conf::Place;
    use crate::support::args::NoArgs;
    use crate::support::comfy_repos::table_fancy_options;
    use crate::support::doc_render;
    use clap::Args;
    use std::io::{self, BufWriter, Write};
    use std::path::{Path, PathBuf};

    /// Align whitespace-delimited columns into a neat table — or paste an HTML <table>
    /// (a saved wiki/docs page) and it's translated into one (table_formatter)
    #[name("table")]
    pub fn table(args: table_formatter::Args) {
        if let Err(err) = table_formatter::run_with(args) {
            eprintln!("table: {err}");
        }
    }

    /// `table` in its framed, terminal-width form: `-j " | " --split-lines --space-rows '-'
    /// --emit-frame` — pipe-joined columns in a border, records ruled apart, wide rows wrapped to
    /// fit the window (table_formatter)
    #[name("table_fancy")]
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
    #[name("backup_diff")]
    pub fn backup_diff(args: filesync::cli::DiffArgs) {
        _filesync(filesync::Command::Diff(args));
    }

    /// Make DEST mirror SOURCE: copy new/changed, rename moves, delete extras; resumable (filesync sync)
    #[name("backup_sync")]
    pub fn backup_sync(args: filesync::cli::SyncArgs) {
        _filesync(filesync::Command::Sync(args));
    }

    /// Verify a mirror by content: `backup_diff` comparing every file's bytes (blake3), so silent
    /// corruption can't hide behind a matching size+mtime (filesync diff --eager-checksum)
    #[name("backup_find_bitrot")]
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
    #[name("clicker")]
    #[alias("autoclick")]
    pub fn clicker(args: sequencer::ClickerArgs) {
        _sequencer(sequencer::Command::Clicker(args));
    }

    /// Measure what this machine can really deliver: writes key presses as fast as asked
    /// (or flat out, with no `--cps`) while reading its own virtual device back, printing the
    /// live rate as it goes. The gap between written and delivered is the honest ceiling —
    /// a rate counted from the sending loop alone would never show it (sequencer bench)
    #[name("clicker_benchmark")]
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
    #[name("autokey_doctor")]
    pub fn autokey_doctor(args: sequencer::DoctorArgs) {
        _sequencer(sequencer::Command::Doctor(args));
    }

    /// The profile store ([`Place::SequencerProfiles`]): where this shell keeps binds
    /// profiles. Deliberately nothing more than a well-known directory — sequencer takes real
    /// paths only, no command resolves bare names against this behind the user's back, and TAB
    /// completion doesn't offer its contents (they aren't in the PWD; the full-path listing is
    /// `autokey_list_bashrs_profiles`'s job). It is what a bare `autokey_apply` passes and what
    /// that listing walks; the same division as [`_cookie_store_dir`] — where bashrs keeps its
    /// data is bashrs's own business. `install-shell` creates it.
    fn _profile_store_dir() -> PathBuf {
        Place::SequencerProfiles.path()
    }

    /// Check binds files for problems without applying them: nothing is activated, no profile
    /// state is touched, and the files are left exactly as they are. `autokey_reformat` is the
    /// same check with tidying turned on (sequencer profile-check)
    #[name("autokey_check")]
    pub fn autokey_check(args: sequencer::ProfileCheckArgs) {
        _sequencer(sequencer::Command::ProfileCheck(args));
    }

    /// `autokey_check` that also rewrites: every file that parses is tidied in place with all its
    /// comments kept, and one that doesn't is reported and left alone. Still activates nothing —
    /// the rewrite is the only thing it changes (sequencer profile-check --format)
    #[name("autokey_reformat")]
    pub fn autokey_reformat(mut args: sequencer::ProfileCheckArgs) {
        // Always tidy what parses: a plain bool, so a caller passing it anyway is a no-op
        // (hidden from this command's help/completion — see `cli::HIDDEN_PINNED`).
        args.format = true;
        _sequencer(sequencer::Command::ProfileCheck(args));
    }

    /// Apply binds profiles: remap keys and run sequences until stopped. Each FILE is linked into
    /// the active set (naming a directory takes every `.toml` directly inside it, in name order);
    /// naming nothing applies the profile store, ~/.bashrs/user-custom/sequencer. The first
    /// invocation becomes the manager and later ones add to it (sequencer profile-apply)
    #[name("autokey_apply")]
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

    /// List the profile store (~/.bashrs/user-custom/sequencer): every binds profile in it,
    /// RECURSIVELY, and every directory — `autokey_apply` expands a directory non-recursively
    /// on purpose, so each directory is its own applyable unit and earns its own line (a
    /// trailing `/`). Full paths, made to be copied straight onto an `autokey_apply` line:
    /// the apply commands take real paths only, so the listing hands out exactly those.
    /// Applied entries are marked, plus anything applied from outside the store, with the
    /// path it came from. `_bashrs_` because the store is this shell's: sequencer applies
    /// whatever files it is given and has no drawer of its own
    #[name("autokey_list_bashrs_profiles")]
    pub fn autokey_list_bashrs_profiles(_args: NoArgs) {
        for line in _profile_listing(&_profile_store_dir(), &sequencer::applied_profiles()) {
            println!("{line}");
        }
    }

    /// The listing's lines exactly as printed: every profile and directory in `store`, then
    /// whatever `applied` holds from outside it, the remarks lined up in one column. Reads
    /// `store` and nothing else, so a test can pose both halves.
    fn _profile_listing(store: &Path, applied: &[(String, PathBuf)]) -> Vec<String> {
        // Applied links resolve to CANONICAL targets (sequencer's scan), so stored files are
        // matched by canonical path — two same-named profiles in different folders can't
        // vouch for each other the way name-matching would let them.
        let is_applied = |path: &Path| {
            std::fs::canonicalize(path)
                .is_ok_and(|canonical| applied.iter().any(|(_, target)| *target == canonical))
        };
        let mut rows: Vec<(String, Option<String>)> = Vec::new();
        let mut matched: Vec<PathBuf> = Vec::new(); // store files found applied, canonical
        let mut pending = vec![store.to_path_buf()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                let shown = path.display().to_string();
                if path.is_dir() {
                    rows.push((format!("{shown}/"), None));
                    pending.push(path);
                } else if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("toml")) {
                    match is_applied(&path) {
                        true => {
                            rows.push((shown, Some("[applied]".to_string())));
                            matched.extend(std::fs::canonicalize(&path));
                        }
                        false => rows.push((shown, None)),
                    }
                }
            }
        }
        let mut listing = Vec::new();
        if rows.is_empty() {
            listing.push(format!(
                "no profiles in {} — drop binds .toml files there (see `autokey_check`)",
                store.display()
            ));
        }
        rows.sort(); // paths sort a directory's line right above its contents
        // Applied out of somewhere else entirely — still part of "what is live right now".
        for (name, target) in applied {
            if !matched.contains(target) {
                rows.push((name.clone(), Some(format!("[applied from {}]", target.display()))));
            }
        }
        listing.extend(_aligned(&rows));
        listing
    }

    /// `rows` as one table: the paths in the first column, the remarks lined up in the second,
    /// and a row with no remark ending at its path — no padding after it, no filler.
    ///
    /// Divided by NUL, the one character a path can never hold. table_formatter reads its input
    /// by splitting lines, and a whitespace divider — the default included — splits on any run
    /// of two spaces or a tab, so a profile under `gaming  old/` would print re-spaced. These
    /// paths are there to be copied onto an `autokey_apply` line; printing one wrong is the one
    /// thing this listing cannot do.
    fn _aligned(rows: &[(String, Option<String>)]) -> Vec<String> {
        const DIVIDER: &str = " \0 ";
        let lines: Vec<String> = rows
            .iter()
            .map(|(path, remark)| match remark {
                Some(remark) => format!("{path}{DIVIDER}{remark}"),
                None => path.clone(),
            })
            .collect();
        let opts = table_formatter::FormatOptions {
            divide_by: DIVIDER.to_string(),
            trim_trailing: true,
            ..Default::default()
        };
        // Cannot fail with these options (a test pins it); if it ever did, this is the old
        // two-space shape rather than NULs on the terminal.
        table_formatter::format_table(&lines, &opts)
            .unwrap_or_else(|_| lines.iter().map(|line| line.replace(DIVIDER, "  ")).collect())
    }

    /// Remove profiles from the active set by name (`gaming` or `gaming.toml`); naming none opens
    /// an interactive picker listing what is currently applied (sequencer profile-unapply)
    #[name("autokey_unapply")]
    pub fn autokey_unapply(args: sequencer::ProfileUnapplyArgs) {
        _sequencer(sequencer::Command::ProfileUnapply(args));
    }

    /// Print the name of each key as it is pressed, once per press — how to learn the key names a
    /// binds profile expects. Reads the input devices, so it is exact; `-n` reads the terminal
    /// instead, needing no privileges but seeing only keys that type something
    /// (sequencer detect-key)
    #[name("autokey_detect")]
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

    /// [`Place::BrowserCookies`] — the base holding one subdir per imported site.
    /// `dl` pins this: the store's *shape* and every operation on it belong to `vidl`, but where
    /// bashrs keeps its data is bashrs's own business. So `vidl`'s two-value
    /// `--cookies-extract-for-domain <TARGET> <OUTPUT-DIR>` is narrowed here to the target alone
    /// (`crate::cli::NARROWED_ARGS`) and this supplies the rest.
    fn _cookie_store_dir() -> PathBuf {
        Place::BrowserCookies.path()
    }

    /// Hand vidl the copies bashrs bundles and version-pins. Without this it would find whatever
    /// is on PATH — which is the right default for a standalone user, and exactly what the
    /// pinning exists to avoid here (a tool with no bundled copy is left to that normal lookup).
    /// The python matters twice over: it runs the cookie filter's sqlite work, so an extract
    /// never depends on a system interpreter.
    fn _install_vidl_tools() {
        use crate::tools::bundled;
        vidl::tools::install(vidl::tools::Tools {
            ytdlp: bundled("yt-dlp").map(PathBuf::into_os_string),
            python: bundled("python3").map(PathBuf::into_os_string),
            ffmpeg_dir: bundled("ffmpeg").and_then(|bin| bin.parent().map(Path::to_path_buf)),
            js_runtime: bundled("deno"),
        });
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
        if vidl.url.is_none() && !vidl.taglist && vidl.global.cookies_extract_for_domain.is_none() {
            eprintln!("dl: a URL is required (or -t, -c, or --cookies-extract-for-domain)");
            std::process::exit(2);
        }
        _install_vidl_tools();
        // Imported cookies live under bashrs's data dir, not vidl's per-user default — the one
        // download option bashrs decides for itself. Set unconditionally, not just on an import: a
        // store is READ on every run, and a root known only while importing would be found once
        // and then lost.
        vidl.global.cookie_root = Some(_cookie_store_dir());
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

    /// Add to the YouTube videos downloaded here (or under PATH), collection folders included — or
    /// to the one video file PATH names — the data each lacks: its thumbnail, subtitle tracks and
    /// their auto-generated marks, metadata tags and chapters, as a download of it would hold them
    /// now. A video is known by the id in its file name, or else by the page its metadata names;
    /// nothing is downloaded again, and nothing a file holds is replaced. `--check` only says what
    /// each lacks (vidl repair)
    #[name("dl_fix")]
    pub fn dl_fix(mut args: vidl::RepairArgs) {
        _install_vidl_tools();
        // As for `dl`: imported cookies live under bashrs's data dir, so a store imported there is
        // the one a repair asks YouTube with.
        args.global.cookie_root = Some(_cookie_store_dir());
        let code = vidl::run_repair(args);
        if code != 0 {
            std::process::exit(code);
        }
    }

    // --- symlink_devs --------------------------------------------------------

    /// Link each device a config lists into place wherever the OS has mounted it, mounting what
    /// isn't: every device gets a root link that follows it, and short links into it go
    /// through that, so a device that moves or is swapped only has its root link re-pointed. With
    /// no CONFIG, the one config in ~/.bashrs/user-custom/devmounts is used (devmounts apply)
    #[name("symlink_devs")]
    pub fn symlink_devs(mut args: devmounts::cli::RunArgs) {
        // Upstream requires a CONFIG; this shell relaxes that (`cli::OPTIONAL_ARGS`) and answers the
        // bare invocation with the store's one config. With none to give, devmounts says the
        // CONFIG is missing, in its own words, and the note after adds how to go without one.
        let found = args.config.is_none().then(|| _devmounts_store_config(&Place::DevmountsConfigs.path()));
        if let Some(StoreConfig::One(config)) = &found {
            args.config = Some(config.clone());
        }
        let code = devmounts::cli::Command::Apply(args).run();
        if let Some(note) = found.and_then(|found| _store_note(&found, code)) {
            eprintln!("{note}");
        }
        if code != 0 {
            std::process::exit(code.into());
        }
    }

    /// What the devmounts store ([`Place::DevmountsConfigs`]) holds for a bare `symlink_devs`.
    #[derive(Debug, PartialEq)]
    enum StoreConfig {
        /// Exactly one config: the one to use.
        One(PathBuf),
        /// No store at all.
        Missing,
        /// A store that can't be listed, and why.
        Unreadable(String),
        /// A store with no config in it.
        Empty,
        /// Several configs, in name order: which one is the user's to say.
        Several(Vec<PathBuf>),
    }

    /// The configs in `store`: its `.toml` files — directly in it, a symlink to one counting as
    /// one (a config kept with the user's dotfiles, linked in).
    fn _devmounts_store_config(store: &Path) -> StoreConfig {
        let entries = match std::fs::read_dir(store) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return StoreConfig::Missing,
            Err(err) => return StoreConfig::Unreadable(err.to_string()),
        };
        let is_config = |path: &PathBuf| {
            path.is_file() && path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("toml"))
        };
        let mut configs: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).filter(is_config).collect();
        configs.sort();
        match configs.len() {
            0 => StoreConfig::Empty,
            1 => StoreConfig::One(configs.remove(0)),
            _ => StoreConfig::Several(configs),
        }
    }

    /// What a bare `symlink_devs` adds to devmounts' own words, given what the store held and how
    /// the run ended: with no single config to take, how to leave CONFIG out, and what the store
    /// holds instead; with one that couldn't be run, which file it was — devmounts' parse errors
    /// don't name it, and nobody named it on the command line.
    fn _store_note(found: &StoreConfig, code: u8) -> Option<String> {
        let store = Place::DevmountsConfigs.shown();
        let held = match found {
            StoreConfig::One(config) => {
                let could_not_run = code == devmounts::cli::Exit::CouldNotRun.code();
                return could_not_run
                    .then(|| format!("symlink_devs: that CONFIG was {}, the one in {store}", config.display()));
            }
            StoreConfig::Missing => "which doesn't exist yet".to_owned(),
            StoreConfig::Unreadable(why) => format!("which can't be read ({why})"),
            StoreConfig::Empty => "which holds none now".to_owned(),
            StoreConfig::Several(configs) => {
                let names: Vec<String> =
                    configs.iter().filter_map(|config| config.file_name()).map(|name| name.to_string_lossy().into_owned()).collect();
                format!("which holds {} now: {}", configs.len(), names.join(", "))
            }
        };
        Some(format!("symlink_devs: to leave CONFIG out, keep exactly one config (a .toml) in {store}, {held}"))
    }

    #[cfg(test)]
    mod autokey_tests {
        use super::*;

        /// A scratch store for one test, gone afterwards.
        fn scratch(tag: &str) -> PathBuf {
            let dir = std::env::temp_dir().join(format!("bashrs_profiles_{tag}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }

        /// One line split into its path and its remark, padding dropped — the CONTENT of the
        /// listing, so the layout can be asserted on its own.
        fn content(line: &str) -> (String, Option<String>) {
            match line.find("[applied") {
                Some(at) => (line[..at].trim_end().to_string(), Some(line[at..].to_string())),
                None => (line.to_string(), None),
            }
        }

        /// Every profile and directory, recursively and in path order; applied ones marked,
        /// matched by canonical path; anything applied from elsewhere listed last with where it
        /// came from; a non-profile file left out.
        #[test]
        fn the_listing_names_every_profile_and_marks_the_applied_ones() {
            let store = scratch("content");
            std::fs::create_dir_all(store.join("sub/deep")).unwrap();
            for file in ["a.toml", "b.toml", "notes.txt", "sub/c.toml"] {
                std::fs::write(store.join(file), "").unwrap();
            }
            let canonical = |file: &str| store.join(file).canonicalize().unwrap();
            let applied = vec![
                ("a.toml".to_string(), canonical("a.toml")),
                ("c.toml".to_string(), canonical("sub/c.toml")),
                ("stray.toml".to_string(), PathBuf::from("/elsewhere/stray.toml")),
            ];
            let at = |rest: &str| format!("{}/{rest}", store.display());
            let marked = |text: &str| Some(text.to_string());
            let listing: Vec<(String, Option<String>)> =
                _profile_listing(&store, &applied).iter().map(|line| content(line)).collect();
            assert_eq!(
                listing,
                [
                    (at("a.toml"), marked("[applied]")),
                    (at("b.toml"), None),
                    (at("sub/"), None),
                    (at("sub/c.toml"), marked("[applied]")),
                    (at("sub/deep/"), None),
                    ("stray.toml".to_string(), marked("[applied from /elsewhere/stray.toml]")),
                ]
            );
            let _ = std::fs::remove_dir_all(&store);
        }

        /// The layout: every remark starts in the same column, a line without one ends at its path,
        /// and a path holding a double space and a tab prints exactly as it is — the case the NUL
        /// divider exists for, since a whitespace divider would split it.
        #[test]
        fn the_remarks_line_up_and_every_path_prints_exactly() {
            let store = scratch("layout");
            std::fs::create_dir_all(store.join("gaming  old")).unwrap();
            for file in ["a.toml", "a-much-longer-profile-name.toml", "gaming  old/x\ty.toml"] {
                std::fs::write(store.join(file), "").unwrap();
            }
            let canonical = |file: &str| store.join(file).canonicalize().unwrap();
            let applied = vec![
                ("a.toml".to_string(), canonical("a.toml")),
                ("x\ty.toml".to_string(), canonical("gaming  old/x\ty.toml")),
                ("stray.toml".to_string(), PathBuf::from("/elsewhere/stray.toml")),
            ];
            let listing = _profile_listing(&store, &applied);
            let columns: Vec<usize> =
                listing.iter().filter_map(|line| line.find("[applied")).collect();
            assert_eq!(columns.len(), 3, "three remarks: {listing:#?}");
            assert!(columns.windows(2).all(|pair| pair[0] == pair[1]), "one column: {listing:#?}");
            let odd = format!("{}/gaming  old/x\ty.toml", store.display());
            assert!(listing.iter().any(|line| line.starts_with(&format!("{odd}  "))), "{listing:#?}");
            let bare = format!("{}/a-much-longer-profile-name.toml", store.display());
            assert!(listing.contains(&bare), "no remark, nothing after the path: {listing:#?}");
            assert!(listing.iter().all(|line| !line.contains('\0')), "the divider never shows");
            let _ = std::fs::remove_dir_all(&store);
        }

        /// An empty or missing store says so — and still lists what is applied from elsewhere.
        #[test]
        fn an_empty_store_says_so_and_still_lists_what_is_applied_elsewhere() {
            let store = scratch("empty").join("never-made");
            let none = _profile_listing(&store, &[]);
            assert_eq!(none.len(), 1);
            assert!(none[0].starts_with(&format!("no profiles in {}", store.display())), "{none:?}");
            let stray = vec![("stray.toml".to_string(), PathBuf::from("/elsewhere/stray.toml"))];
            let with = _profile_listing(&store, &stray);
            assert_eq!(with[1], "stray.toml  [applied from /elsewhere/stray.toml]", "{with:?}");
        }
    }

    #[cfg(test)]
    mod symlink_devs_tests {
        use super::*;

        /// A scratch store for one test, gone afterwards.
        fn scratch(tag: &str) -> PathBuf {
            let dir = std::env::temp_dir().join(format!("bashrs_devmounts_{tag}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }

        /// A bare run takes the store's config only when it holds exactly one, and tells the other
        /// cases apart. A config is a `.toml` file directly in the store — or a symlink to one —
        /// never a directory named like one, nor anything deeper.
        #[test]
        fn the_store_gives_its_config_only_when_it_holds_exactly_one() {
            let store = scratch("store");
            assert_eq!(_devmounts_store_config(&store.join("never-made")), StoreConfig::Missing);
            assert_eq!(_devmounts_store_config(&store), StoreConfig::Empty);
            std::fs::create_dir_all(store.join("old.toml")).unwrap();
            std::fs::create_dir_all(store.join("sub")).unwrap();
            std::fs::write(store.join("sub/deep.toml"), "").unwrap();
            std::fs::write(store.join("notes.txt"), "").unwrap();
            assert_eq!(_devmounts_store_config(&store), StoreConfig::Empty, "none of those is a config here");

            std::os::unix::fs::symlink(store.join("sub/deep.toml"), store.join("linked.TOML")).unwrap();
            assert_eq!(_devmounts_store_config(&store), StoreConfig::One(store.join("linked.TOML")));
            std::fs::write(store.join("home.toml"), "").unwrap();
            let both = vec![store.join("home.toml"), store.join("linked.TOML")];
            assert_eq!(_devmounts_store_config(&store), StoreConfig::Several(both));
            let _ = std::fs::remove_dir_all(&store);
        }

        /// A store that can't be listed says why, rather than passing for an empty one.
        #[test]
        fn an_unreadable_store_is_told_from_an_empty_one() {
            if crate::support::superuser::is_root() {
                return; // root lists any directory
            }
            use std::os::unix::fs::PermissionsExt;
            let store = scratch("unreadable");
            std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o000)).unwrap();
            let found = _devmounts_store_config(&store);
            std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(matches!(found, StoreConfig::Unreadable(_)), "{found:?}");
            let _ = std::fs::remove_dir_all(&store);
        }

        /// What follows devmounts' own words: how to leave CONFIG out and what the store holds
        /// instead — or, when the store's one config couldn't be run, which file that was; and
        /// nothing when it ran.
        #[test]
        fn the_note_says_what_the_store_holds() {
            let store = Place::DevmountsConfigs.shown();
            let leave_out = format!("symlink_devs: to leave CONFIG out, keep exactly one config (a .toml) in {store}, ");
            let note = |found: StoreConfig| _store_note(&found, 2).unwrap();
            assert_eq!(note(StoreConfig::Missing), format!("{leave_out}which doesn't exist yet"));
            assert_eq!(note(StoreConfig::Empty), format!("{leave_out}which holds none now"));
            assert_eq!(note(StoreConfig::Unreadable("Permission denied".into())), format!("{leave_out}which can't be read (Permission denied)"));
            let several = StoreConfig::Several(vec!["/s/home.toml".into(), "/s/work.toml".into()]);
            assert_eq!(note(several), format!("{leave_out}which holds 2 now: home.toml, work.toml"));

            let one = || StoreConfig::One("/s/home.toml".into());
            assert_eq!(_store_note(&one(), 2), Some(format!("symlink_devs: that CONFIG was /s/home.toml, the one in {store}")));
            for ran in [0, 1] {
                assert_eq!(_store_note(&one(), ran), None, "exit {ran}: the report says it all");
            }
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
