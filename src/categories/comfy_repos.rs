//! Commands backed by my other repos, imported as dependencies. Each is exposed under the
//! external tool's own name (`table`), a task-named family wrapping its subcommands (`backup_*`
//! over filesync), or the verb it has always had here (`dl`, over the `vidl` crate) — all
//! `#[unprefixed]`, and each reuses the upstream argument structs where it can, so the args are
//! defined once, upstream.
//!
//! `dl` carries more of its own weight than the others: `vidl` downloads videos and nothing else,
//! so acquiring and vetting browser cookies, injecting bashrs's pinned tool paths, and the
//! supported-sites page all stay on this side of the boundary.

#[bashrs_macros::category(command = ComfyReposCommand, prefix = "comfy_")]
mod commands {
    use crate::drivers::cookie_store;
    use crate::support::browsers;
    use crate::support::comfy_repos::table_fancy_options;
    use crate::support::doc_render;
    use crate::support::doc_style::{self, _header};
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
    /// the first invocation becomes the manager and later ones add to it (sequencer profile-apply)
    #[unprefixed]
    pub fn autokey_apply(args: sequencer::ProfileApplyArgs) {
        _sequencer(sequencer::Command::ProfileApply(args));
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

    /// `~/.bashrs/user-data/browser_cookies` — the base holding one subdir per imported site
    /// (`<key>/store/` + `<key>/browser.spec`). `--cookie-import` writes a site's subdir; each
    /// `dl <url>` run reads the subdir matching the URL's site. The dir NAME belongs to
    /// [`browsers`] (the store's on-disk shape is its concern); only the join onto the
    /// user-data root happens here.
    fn _cookie_store_dir() -> PathBuf {
        crate::conf::user_data_dir().join(browsers::ROOT_SUBDIR)
    }

    /// The bundled copy of `name`, when bashrs actually bundles it. `tools::resolve` answers with
    /// the bare name when there's no bundle, which is the signal to let the tool be found on PATH
    /// instead — so `None` here means "no pinned copy; use the normal lookup".
    fn _bundled(name: &str) -> Option<PathBuf> {
        let resolved = crate::tools::resolve(name);
        (resolved != *name).then(|| PathBuf::from(resolved))
    }

    /// The `--cookie-import <target>` action: resolve the target site, scan for browser cookie
    /// stores, let the user pick one, and copy *only that site's* cookies into a per-site dir
    /// under bashrs's data (which a running browser can't lock). Later `dl` runs on that site
    /// use it automatically. `target` is a site keyword, a domain, or a URL. Returns whether a
    /// usable store was imported.
    fn _import_cookies(target: &str) -> bool {
        // Reject a target we can't map to cookie domains (a bare word like `tiktok2`) up front —
        // otherwise we'd build an empty store and report a misleading "no cookies found".
        if !browsers::is_importable_target(target) {
            eprintln!(
                "dl: '{target}' isn't a site I can target — pass a keyword ({}), a domain (example.com), or a video URL",
                browsers::known_site_names()
            );
            return false;
        }
        let site = browsers::resolve_target(target);
        let home = crate::conf::home();
        let stores = browsers::cookie_stores(&home);
        if stores.is_empty() {
            if browsers::any_browser_installed(&home) {
                eprintln!("dl: found browsers but no cookie stores yet — browse (and sign in) once, then retry");
            } else {
                eprintln!("dl: no browser cookie stores found on this system");
            }
            return false;
        }
        let Some(store) = _pick(&stores, &site.label) else { return false };
        let site_dir = _cookie_store_dir().join(&site.key);
        let store_dir = match browsers::reset_site(&site_dir) {
            Ok(dir) => dir,
            Err(err) => {
                eprintln!("dl: could not prepare the {} cookie store: {err}", site.label);
                return false;
            }
        };
        // Cherry-pick: only the target site's cookies are copied — never the whole DB.
        let Some(matched) = cookie_store::filter_cookie_db(store, &store_dir, &site.domains) else {
            let _ = browsers::forget(&site_dir);
            eprintln!("dl: could not filter the cookie store (is the bundled python available?)");
            return false;
        };
        if matched == 0 {
            let _ = browsers::forget(&site_dir);
            eprintln!(
                "dl: no {} cookies in {} — sign in to {} in that browser/profile, then re-import",
                site.label, store.label, site.label
            );
            return false;
        }
        if let Err(err) = browsers::write_spec(&site_dir, store.browser) {
            let _ = browsers::forget(&site_dir);
            eprintln!("dl: could not record the {} cookie store: {err}", site.label);
            return false;
        }
        println!("imported {matched} {} cookie(s) from {}", site.label, store.label);
        if site.key == "youtube" {
            println!("note: YouTube rotates cookies on open tabs — if auth later fails, re-export via a private window (sign in, open only youtube.com/robots.txt, then close it) and re-import");
        }
        _report_cookie_check(&site, store.browser, matched, &site_dir)
    }

    /// Warn (in red) when the imported store a download is about to use holds only expired cookies,
    /// so a gated fetch that's about to fail for a stale session says so up front — with the
    /// re-import fix — rather than surfacing as a bare auth error. Silent when the store is fresh or
    /// the check can't run.
    fn _warn_if_cookies_expired(site_dir: &Path, site: &browsers::SiteTarget) {
        let Some((db, kind)) = browsers::imported_db(site_dir) else { return };
        if cookie_store::cookies_expired(&db, kind) == Some(true) {
            eprintln!(
                "{}",
                doc_style::problematic(&format!(
                    "dl: the imported {} cookies have expired — re-import with `dl --cookie-import {}`",
                    site.label, site.key
                ))
            );
        }
    }

    /// Read the freshly imported (already domain-filtered) store back through yt-dlp to confirm
    /// the cookies actually *decrypt* — a keyring-locked Chromium store filters fine but reads
    /// back empty — and report the concrete result, catching that failure now instead of on
    /// every later run. Returns whether a usable store remains. When the read-back can't run
    /// (yt-dlp not bundled yet), keeps the store on the provisional count rather than overclaiming.
    fn _report_cookie_check(site: &browsers::SiteTarget, browser: &str, matched: usize, site_dir: &Path) -> bool {
        // The WAL caveat holds whenever cookies came through: a copy captures only what the
        // browser already flushed to the DB, so a sign-in from moments ago may not be in it yet.
        let wal_note = || println!("note: if a fresh sign-in isn't recognized, fully quit the browser and re-import");
        let Some(spec) = browsers::imported_spec(site_dir) else { return true };
        match cookie_store::readable_cookie_count(&spec, site_dir) {
            Some(n) if n > 0 => {
                println!("validated — {n} {} cookie(s) readable; dl runs on {} will use them", site.label, site.label);
                wal_note();
                true
            }
            Some(_) => {
                // Filtered rows but none decrypt → useless; drop it so future runs don't keep
                // re-attempting a doomed read (a locked Chromium keyring can even prompt).
                let _ = browsers::forget(site_dir);
                if browser == "firefox" {
                    eprintln!("dl: {matched} {} cookie(s) imported but none could be read — the profile DB may be unreadable; re-import", site.label);
                } else {
                    eprintln!("dl: {matched} {} cookie(s) imported but none decrypted — Chromium needs the desktop keyring unlocked; a Firefox store is more reliable. Import discarded.", site.label);
                }
                false
            }
            None => {
                println!("(couldn't verify readability — is yt-dlp installed? the {matched} imported cookie(s) will be tried on dl runs)");
                wal_note();
                true
            }
        }
    }

    /// Prompt for one of `stores` to import `site_label`'s cookies from (auto-selecting a lone
    /// candidate).
    fn _pick<'a>(stores: &'a [browsers::CookieStore], site_label: &str) -> Option<&'a browsers::CookieStore> {
        if let [only] = stores {
            println!("one cookie store found — importing {site_label} cookies from {}", only.label);
            return Some(only);
        }
        println!("{}", _header(&format!("Import {site_label} cookies from:")));
        for (i, store) in stores.iter().enumerate() {
            println!("  {}) {}", i + 1, store.label);
        }
        print!("choice [1-{}]: ", stores.len());
        use std::io::Write;
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).ok()?;
        match line.trim().parse::<usize>().ok().filter(|n| (1..=stores.len()).contains(n)) {
            Some(n) => Some(&stores[n - 1]),
            None => {
                eprintln!("dl: not a listed number");
                None
            }
        }
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
        let DlArgs { mut vidl, no_cookies, cookie_import, compatibility_help } = args;
        if compatibility_help {
            print!("{}", _compatibility_help());
            return;
        }
        if let Some(target) = &cookie_import {
            let imported = _import_cookies(target);
            // `--cookie-import <target>` (no URL) is a setup step: import and stop. With a URL,
            // fall through and download, now able to use what was just imported.
            if vidl.url.is_none() {
                std::process::exit(i32::from(!imported));
            }
        }
        if vidl.url.is_none() && !vidl.taglist {
            eprintln!("dl: a URL is required (or -t, -c, or --cookie-import)");
            std::process::exit(2);
        }
        // Hand vidl the copies bashrs bundles and version-pins. Without this it would find
        // whatever is on PATH — which is the right default for a standalone user, and exactly
        // what the pinning exists to avoid here.
        vidl::tools::install(vidl::tools::Tools {
            ytdlp: _bundled("yt-dlp").map(PathBuf::into_os_string),
            python: _bundled("python3").map(PathBuf::into_os_string),
            ffmpeg_dir: _bundled("ffmpeg").and_then(|bin| bin.parent().map(Path::to_path_buf)),
            js_runtime: _bundled("deno"),
        });
        // Auto-select the per-site store matching this URL's host — a prior `--cookie-import` for
        // this site is the standing default; an explicit `--cookies` file wins, and `--no-cookies`
        // opts out of stored cookies entirely (download anonymously). This is the one download
        // option bashrs decides for itself; every other flag reaches vidl as the user typed it.
        let site = browsers::resolve_target(vidl.url.as_deref().unwrap_or_default());
        let site_dir = _cookie_store_dir().join(&site.key);
        let asked_for_cookies = vidl.cookies.is_some() || vidl.cookies_from_browser.is_some();
        let imported = (!no_cookies && !asked_for_cookies)
            .then(|| browsers::imported_spec(&site_dir))
            .flatten();
        if imported.is_some() {
            _warn_if_cookies_expired(&site_dir, &site);
            vidl.cookies_from_browser = imported;
        }
        let code = vidl::run(vidl);
        if code != 0 {
            std::process::exit(code);
        }
    }

    #[derive(Args)]
    pub struct DlArgs {
        #[command(flatten)]
        pub vidl: vidl::Args,
        /// Ignore cookies stored by a prior `--cookie-import` for this site and download
        /// anonymously — useful when a site bot-walls a signed-in session (e.g. TikTok) and
        /// sending your real cookies risks getting that session flagged
        #[arg(long, conflicts_with_all = ["cookies", "cookies_from_browser", "cookie_import"])]
        pub no_cookies: bool,
        /// Import a browser's cookies for one site, so later dl runs on it authenticate. TARGET
        /// is a site keyword (youtube, tiktok, facebook, instagram, twitter, reddit, vimeo,
        /// twitch, niconico, bilibili, patreon, nebula, bbc), a domain (example.com), or a URL.
        /// Only that site's cookies are copied —
        /// never your whole cookie DB. Scans installed browsers (native/Flatpak/Snap/Nix) and
        /// copies from your pick, so a running browser can't lock it. Runs standalone, or before
        /// a download when a URL is also given
        #[arg(long, value_name = "TARGET", conflicts_with = "cookies")]
        pub cookie_import: Option<String>,
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
