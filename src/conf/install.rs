//! The install itself — the tail end of COMPILE.sh, moved into one testable implementation:
//! guard and create `~/.bashrs`, install the running binary into it (self-copy), write the
//! generated sourcefiles, then wire each rc file the user has to source them (idempotent-by-marker,
//! so re-runs find the block and skip) — and fish, through a hook in its conf.d. Run via the hidden
//! `install-shell` command, COMPILE.sh's last step — invoked from the freshly built
//! `target/release/bashrs`, which is what makes the self-copy the installation.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The sourcefile path as written INTO rc files — deliberately UNEXPANDED: the literal `$HOME`
/// expands when the rc file is *sourced*, so the wired line stays correct across machines and
/// users sharing one rc (dotfile repos), where an expanded home would freeze this machine's
/// path in. (COMPILE.sh's old `SRC_PATH`, renamed for what it is.)
const UNEXPANDED_SOURCEFILE_PATH: &str = "$HOME/.bashrs/sourcefile.sh";

/// The fish sourcefile's path as fish's hook sources it — unexpanded for the same reason.
const UNEXPANDED_FISH_SOURCEFILE_PATH: &str = "$HOME/.bashrs/sourcefile.fish";

/// Markers bracketing the wired block — re-runs find the opener and skip instead of appending
/// a duplicate.
const BLOCK_START: &str = "# >>> bashrs >>>";
const BLOCK_END: &str = "# <<< bashrs <<<";

/// What [`wire_rc`] did to one rc file.
#[derive(Debug, PartialEq)]
enum Wired {
    /// The rc file doesn't exist — only files the user already has are touched.
    Absent,
    /// The block is already in place from an earlier run — left untouched.
    AlreadyPresent,
    Added,
}

/// SIDE EFFECTS — the whole install: guard and create `~/.bashrs`, bring its user directories
/// to their current names (`migrate_user_dirs`), copy the running binary in,
/// write the generated sourcefiles (`bash` to `~/.bashrs/sourcefile.sh`, `fish` to
/// `~/.bashrs/sourcefile.fish`), make bashrs's completions win for the names in `fish_completed`
/// that fish completes too (`shadow_fish_completions`), then wire `.bashrc`, `.zshrc` and fish's
/// conf.d. Any failure prints and exits non-zero, aborting COMPILE.sh like its old `die` did.
pub fn install_shell(bash: &str, fish: &str, fish_completed: &[String]) {
    let home = super::bashrs_home();
    let die = |msg: String| -> ! {
        eprintln!("ERROR: {msg}");
        std::process::exit(1);
    };
    if let Err(msg) = ensure_home(&home) {
        die(msg);
    }
    for line in migrate_user_dirs(&home) {
        match line {
            Ok(moved) => println!("{moved}"),
            Err(warning) => eprintln!("WARNING: {warning}"),
        }
    }
    if let Err(msg) = ensure_profile_store(&home) {
        die(msg);
    }
    match install_binary(&home) {
        // Through a symlink, both ends are named: the file written is not where the user looks.
        Ok(Some(bin)) => match home.join("bashrs") {
            link if link == bin => println!("Installed {}", bin.display()),
            link => println!("Installed {} → {}", link.display(), bin.display()),
        },
        Ok(None) => {} // already running as the installed copy — nothing to move
        Err(msg) => die(msg),
    }
    let (sourcefile, fish_sourcefile) = (super::sourcefile(), super::fish_sourcefile());
    for (file, content) in [(&sourcefile, bash), (&fish_sourcefile, fish)] {
        if let Err(err) = std::fs::write(file, content) {
            die(format!("could not write {}: {err}", file.display()));
        }
        println!("Generated {}", file.display());
    }
    let colliding = colliding(fish_completed, &fish_complete_path(), &fish_built_in_completions());
    match shadow_fish_completions(&super::fish_completions_dir(), &colliding) {
        Ok(()) if colliding.is_empty() => {}
        Ok(()) => println!("Fish ships completions for {}; bashrs's own replace them", colliding.join(", ")),
        Err(msg) => die(msg),
    }

    // The user's own home now, not `~/.bashrs` — a different directory, so a distinct name.
    let user_home = super::home();
    // Whether one target is wired, reporting what was found; a failure ends the install.
    let wired = |target: &Path, outcome: Result<Wired, String>| match outcome {
        Ok(Wired::Absent) => false,
        Ok(Wired::AlreadyPresent) => {
            println!("Source block already present in {} — leaving it untouched.", target.display());
            true
        }
        Ok(Wired::Added) => {
            println!("Added source block to {}", target.display());
            true
        }
        Err(msg) => die(msg),
    };
    let mut any_present = false;
    for rc in [user_home.join(".bashrc"), user_home.join(".zshrc")] {
        any_present |= wired(&rc, wire_rc(&rc));
    }
    let fish_config = fish_config_dir(std::env::var_os("XDG_CONFIG_HOME").as_deref(), &user_home);
    let fish_wired = wired(&fish_hook(&fish_config), wire_fish(&fish_config));
    if !(any_present || fish_wired) {
        println!("Note: neither ~/.bashrc nor ~/.zshrc exists, and fish has no configuration directory; nothing was wired up.");
    }
    println!();
    // A `brs_compile` (the everyday recompile) reloads the shell for you the instant this
    // returns, so telling the user to open one themselves is wrong — it already happens. Only a
    // run with no reload coming (a first-time `./COMPILE.sh`, or the script run by hand) needs
    // the manual instruction. Which one this is is read off the process tree, where the fact
    // already lives — see [`reload_follows`].
    let fish_sourcefile = fish_wired.then_some(fish_sourcefile.as_path());
    println!("{}", activation_line(reload_follows(), &sourcefile, fish_sourcefile));
}

/// The install's closing line. When a shell reload will follow (the `brs_compile` flow),
/// say so — the prompt is about to reset under the user, and asking them to act would contradict
/// what's happening. Otherwise (a first-time `./COMPILE.sh`), name the one step that activates it
/// — and fish's own, when fish was wired (`fish_sourcefile`).
///
/// The backticks around the command are display only: this string leaves through `println!`, not
/// through a shell, so nothing here is ever evaluated — unlike the sourcefile's stale-config nag,
/// where a backtick once meant command substitution at every prompt.
fn activation_line(reload_follows: bool, sourcefile: &Path, fish_sourcefile: Option<&Path>) -> String {
    if reload_follows {
        "Done — reloading your shell.".to_string()
    } else {
        let fish = fish_sourcefile.map(|fish| format!("  (in fish:  `source \"{}\"`)", fish.display()));
        format!("Done. Open a new shell, or run:  `. \"{}\"`{}", sourcefile.display(), fish.unwrap_or_default())
    }
}

/// Whether the shell will reload on its own once this install returns — true exactly when a
/// `brs_compile` invocation sits in this process's ancestry (its generated wrapper catches the
/// reload exit code and runs `shell_new`). Read straight off `/proc`'s parent chain rather than
/// signalled through an env var: the process tree already carries the fact, and a second channel
/// for it could only drift or leak. The chain here is short — install-shell ← bash(COMPILE.sh) ←
/// bashrs(brs_compile) ← the user's shell — but the walk is bounded anyway.
fn reload_follows() -> bool {
    ancestor_argvs().into_iter().any(|argv| is_brs_compile(&argv))
}

/// The argv of each ancestor process, nearest first, walked via `/proc/<pid>/stat`'s ppid field
/// (parsed past the LAST `)` — the comm before it may itself contain parentheses) up to init.
/// Bounded, and any unreadable link ends the walk — a truncated answer degrades to the manual
/// closing line, never to a wrong claim of a reload.
fn ancestor_argvs() -> Vec<Vec<String>> {
    let mut argvs = Vec::new();
    let mut pid = std::process::id();
    for _ in 0..16 {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { break };
        let Some(after_comm) = stat.rsplit(')').next() else { break };
        let Some(ppid) = after_comm.split_whitespace().nth(1).and_then(|f| f.parse::<u32>().ok())
        else {
            break;
        };
        if ppid <= 1 {
            break;
        }
        if let Ok(cmdline) = std::fs::read(format!("/proc/{ppid}/cmdline")) {
            let argv: Vec<String> = cmdline
                .split(|byte| *byte == 0)
                .filter(|part| !part.is_empty())
                .map(|part| String::from_utf8_lossy(part).into_owned())
                .collect();
            argvs.push(argv);
        }
        pid = ppid;
    }
    argvs
}

/// Whether one process's argv is a `brs_compile` run — the bashrs binary (by file name, since
/// the wrapper invokes it by absolute path) told to compile. Both halves matter: `brs_compile`
/// alone would also match some unrelated program handed that word as data.
fn is_brs_compile(argv: &[String]) -> bool {
    let binary_is_bashrs = argv
        .first()
        .map(Path::new)
        .and_then(Path::file_name)
        .is_some_and(|name| name == "bashrs");
    binary_is_bashrs && argv.get(1).is_some_and(|arg| arg == "brs_compile")
}

/// Guard and create the install dir. `~/.bashrs` itself is preserved as-is — it may be a
/// symlink (e.g. into a dotfiles repo), and writing *through* a live one is the point; only a
/// symlink whose target is missing is refused (creating alongside it would shadow the intent).
fn ensure_home(home: &Path) -> Result<(), String> {
    if home.is_symlink() && !home.is_dir() {
        return Err(format!(
            "{} is a symlink to a missing target; fix or remove it, then re-run.",
            home.display()
        ));
    }
    std::fs::create_dir_all(home).map_err(|err| format!("could not create {}: {err}", home.display()))
}

/// Create the binds-profile store, `<home>/user-custom/sequencer` (the `<home>` form of
/// [`super::user_custom_dir`], kept parameterised like everything here so tests install into a
/// temp home). Nothing else creates it — profiles are hand-written — and it must exist before
/// the first one does: it's what makes the store discoverable, and what a bare `autokey_apply`
/// passes. The `user-appdata` dirs are NOT made here; their owners build them on first write
/// (`dl`'s cookie store, the vendor list).
fn ensure_profile_store(home: &Path) -> Result<(), String> {
    let store = home.join("user-custom").join("sequencer");
    std::fs::create_dir_all(&store)
        .map_err(|err| format!("could not create {}: {err}", store.display()))
}

/// The user directories' earlier names under `<home>`, each with the name it has now, in the
/// order the moves must happen: `user-data` held the apps' data and the user's own writing
/// alike, so it became `user-appdata` as a whole — and then the binds-profile store, the
/// user's own, moved on to `user-custom`. A later rename is one more row.
const USER_DIR_MOVES: &[(&str, &str)] = &[("user-data", "user-appdata"), ("user-appdata/sequencer", "user-custom/sequencer")];

/// Bring the user directories under `home` to their current names ([`USER_DIR_MOVES`]) — once:
/// a move happens while the old name exists and the new one doesn't. A rename, so a symlinked
/// directory moves as the link it is, its target untouched. An empty directory on either side
/// is no obstacle — nothing to merge — so it is simply removed; but when both hold something,
/// nothing is merged behind the user's back: that is a warning naming both. Never fatal — the
/// install goes on either way. `Ok` lines say what moved; `Err` lines are the warnings.
fn migrate_user_dirs(home: &Path) -> Vec<Result<String, String>> {
    let mut said = Vec::new();
    for (old, new) in USER_DIR_MOVES {
        let (from, to) = (home.join(old), home.join(new));
        if from.symlink_metadata().is_err() {
            continue; // nothing under the old name: moved already, or never there
        }
        if to.symlink_metadata().is_ok() {
            if is_empty_dir(&from) {
                let _ = std::fs::remove_dir(&from);
                continue;
            }
            if !is_empty_dir(&to) || std::fs::remove_dir(&to).is_err() {
                said.push(Err(format!(
                    "both {} and {} exist — merge the first into the second by hand, then delete it",
                    from.display(),
                    to.display()
                )));
                continue;
            }
        }
        let moved = to.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::rename(&from, &to));
        said.push(match moved {
            Ok(()) => Ok(format!("Moved {} to {}", from.display(), to.display())),
            Err(err) => Err(format!(
                "could not move {} to {}: {err} — move it by hand: mv {} {}",
                from.display(),
                to.display(),
                from.display(),
                to.display()
            )),
        });
    }
    said
}

/// Whether `path` is a real directory with nothing in it — never a symlink, whose target is not
/// this install's to judge, let alone remove.
fn is_empty_dir(path: &Path) -> bool {
    path.symlink_metadata().is_ok_and(|meta| meta.is_dir())
        && std::fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_none())
}

/// Install the running executable as `<home>/bashrs` — the self-copy that IS the installation
/// (COMPILE.sh runs this command from the freshly built `target/release/bashrs`). The old copy
/// is unlinked first, like `install(1)` did: overwriting a *running* executable in place fails
/// with ETXTBSY (the `brs_compile` flow keeps the old binary alive while this runs), while
/// unlinking leaves that process its inode and frees the name. `None` when already running AS
/// the installed copy — copying a file onto itself would truncate it.
///
/// A symlinked `<home>/bashrs` is replaced at the file it points to, and the link survives — see
/// [`super::replacement_target`]. Unlink-first is exactly the kind of replacement that would
/// otherwise cut the link.
fn install_binary(home: &Path) -> Result<Option<PathBuf>, String> {
    let source = std::env::current_exe().map_err(|err| format!("could not locate the running binary: {err}"))?;
    let dest = super::replacement_target(&home.join("bashrs"));
    if dest.exists()
        && matches!((source.canonicalize(), dest.canonicalize()), (Ok(a), Ok(b)) if a == b)
    {
        return Ok(None);
    }
    let _ = std::fs::remove_file(&dest);
    std::fs::copy(&source, &dest)
        .map_err(|err| format!("could not install the binary to {}: {err}", dest.display()))?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))
        .map_err(|err| format!("could not mark {} executable: {err}", dest.display()))?;
    Ok(Some(dest))
}

/// Wire one rc file to source the bashrs sourcefile — append the marker-bracketed block once.
/// Only a file that already exists is touched; an existing path must then be a regular,
/// appendable file (a directory or a read-only rc is an error worth stopping the install for,
/// not a silent skip).
fn wire_rc(rc: &Path) -> Result<Wired, String> {
    if !rc.exists() {
        return Ok(Wired::Absent);
    }
    append_block(rc, &format!("[ -r \"{path}\" ] && . \"{path}\"", path = UNEXPANDED_SOURCEFILE_PATH))
}

/// Fish's configuration directory, by fish's own rule: `$XDG_CONFIG_HOME/fish`, else
/// `~/.config/fish`. A relative `XDG_CONFIG_HOME` is ignored, as the XDG spec says to: fish would
/// read it against wherever it happens to start, which no installer can know.
fn fish_config_dir(xdg_config_home: Option<&std::ffi::OsStr>, home: &Path) -> PathBuf {
    match xdg_config_home.map(Path::new).filter(|dir| dir.is_absolute()) {
        Some(dir) => dir.join("fish"),
        None => home.join(".config").join("fish"),
    }
}

/// The hook fish loads bashrs through: a file of bashrs's own in fish's conf.d.
fn fish_hook(fish_config: &Path) -> PathBuf {
    fish_config.join("conf.d").join("bashrs.fish")
}

/// Wire fish to its sourcefile through [`fish_hook`] — the place fish reads snippets a program
/// installs, before `config.fish`, so the user's own config is never edited. Only where fish's
/// configuration directory exists, which is fish having been run: the counterpart of touching only
/// the rc files a user has. The hook is bashrs's own file, so it is created when absent, and
/// otherwise held to the same marker rule as an rc file.
fn wire_fish(fish_config: &Path) -> Result<Wired, String> {
    if !fish_config.is_dir() {
        return Ok(Wired::Absent);
    }
    let hook = fish_hook(fish_config);
    let line = format!("test -r \"{path}\"; and source \"{path}\"", path = UNEXPANDED_FISH_SOURCEFILE_PATH);
    if hook.exists() {
        return append_block(&hook, &line);
    }
    let conf_d = fish_config.join("conf.d");
    std::fs::create_dir_all(&conf_d).map_err(|err| format!("could not create {}: {err}", conf_d.display()))?;
    std::fs::write(&hook, format!("{BLOCK_START}\n{line}\n{BLOCK_END}\n"))
        .map_err(|err| format!("could not write {}: {err}", hook.display()))?;
    Ok(Wired::Added)
}

/// Where the installed fish looks for completion files — its `$fish_complete_path`, asked of fish
/// itself, since fish's own startup builds the list (and the user's config may add to it). Asked
/// non-interactively, as any fish script would, so bashrs's own hook returns before touching it.
/// Empty when fish is not installed.
fn fish_complete_path() -> Vec<PathBuf> {
    if !crate::support::exec::on_path("fish") {
        return Vec::new();
    }
    let listing = crate::support::exec::capture_without_input("fish", ["-c", "string join \\n -- $fish_complete_path"]);
    listing.unwrap_or_default().lines().filter(|line| !line.is_empty()).map(PathBuf::from).collect()
}

/// The completions fish carries inside its own binary, by command name, as `status list-files
/// completions` lists them. Fish 4.9 keeps its own completions there, and no longer has their
/// directory on `$fish_complete_path` at all, so a search of the path alone finds none of them —
/// yet fish still loads them at a name's first TAB, unless a file of the same name on the path comes
/// first. An older fish has no such subcommand (its error is discarded) and keeps its completions
/// as files on the path, which [`fish_complete_path`] covers; for it, this is empty.
fn fish_built_in_completions() -> BTreeSet<String> {
    if !crate::support::exec::on_path("fish") {
        return BTreeSet::new();
    }
    let listing = crate::support::exec::capture_without_input("fish", ["-c", "status list-files completions 2>/dev/null"]);
    built_in_names(&listing.unwrap_or_default())
}

/// The command names in a `status list-files completions` listing: its `completions/NAME.fish`
/// lines.
fn built_in_names(listing: &str) -> BTreeSet<String> {
    listing
        .lines()
        .filter_map(|line| line.strip_prefix("completions/")?.strip_suffix(".fish"))
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

/// The names among `completed` that fish completes too — with a file somewhere on `complete_path`,
/// or one `built_in` to fish: the names whose completions fish would load at the first TAB and
/// merge with bashrs's.
fn colliding<'a>(completed: &'a [String], complete_path: &[PathBuf], built_in: &BTreeSet<String>) -> Vec<&'a str> {
    let ships = |name: &str| {
        built_in.contains(name) || complete_path.iter().any(|dir| dir.join(format!("{name}.fish")).is_file())
    };
    completed.iter().map(String::as_str).filter(|name| ships(name)).collect()
}

/// Make bashrs's completions win for each name in `colliding`, through a file in `dir` — which the
/// fish sourcefile puts first on `$fish_complete_path`. Fish loads the first `<name>.fish` it finds
/// on that path at the name's first TAB — and turns to a completion built into it (fish 4.9 keeps
/// its own there) only where the path has none — so this one is loaded in place of fish's own, and
/// none of fish's code runs; it defines nothing, as bashrs's completions are in the sourcefile
/// already. A file for a
/// name that no longer collides is removed, and `dir` with it once none is left. Only `*.fish`
/// files are touched — anything else in `dir` is not bashrs's to remove.
fn shadow_fish_completions(dir: &Path, colliding: &[&str]) -> Result<(), String> {
    if !colliding.is_empty() {
        std::fs::create_dir_all(dir).map_err(|err| format!("could not create {}: {err}", dir.display()))?;
    }
    for name in colliding {
        let file = dir.join(format!("{name}.fish"));
        let text = format!(
            "# Generated by bashrs's install — do not edit.\n\
             # bashrs completes `{name}` itself, in ~/.bashrs/sourcefile.fish. Fish has completions of its\n\
             # own for the name; found first, this file is loaded in their place, so they never join bashrs's.\n"
        );
        std::fs::write(&file, text).map_err(|err| format!("could not write {}: {err}", file.display()))?;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(()) };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let stale = path.extension().is_some_and(|ext| ext == "fish")
            && path.file_stem().and_then(|stem| stem.to_str()).is_some_and(|stem| !colliding.contains(&stem));
        if stale {
            std::fs::remove_file(&path).map_err(|err| format!("could not remove {}: {err}", path.display()))?;
        }
    }
    let _ = std::fs::remove_dir(dir); // only succeeds once empty
    Ok(())
}

/// Append the marker-bracketed block holding `line` to an existing `file`, once. The file must be
/// a regular, appendable file — a directory or a read-only file is an error worth stopping the
/// install for, not a silent skip.
fn append_block(file: &Path, line: &str) -> Result<Wired, String> {
    if !file.is_file() {
        return Err(format!("{} exists but is not a regular file.", file.display()));
    }
    let current = std::fs::read(file).map_err(|err| format!("could not read {}: {err}", file.display()))?;
    if String::from_utf8_lossy(&current).contains(BLOCK_START) {
        return Ok(Wired::AlreadyPresent);
    }
    let block = format!("\n{BLOCK_START}\n{line}\n{BLOCK_END}\n");
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(file)
        .and_then(|mut handle| handle.write_all(block.as_bytes()))
        .map_err(|err| format!("failed to append the source block to {}: {err}", file.display()))?;
    Ok(Wired::Added)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bashrs_install_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_wired_path_is_left_unexpanded_for_the_shell_to_expand() {
        // A literal `$HOME`, not this machine's home: the rc line must stay portable.
        assert!(UNEXPANDED_SOURCEFILE_PATH.starts_with("$HOME/"));
    }

    /// The closing line must match what actually happens next. When `brs_compile` will reload
    /// the shell, it must NOT tell the user to open one — that was the bug: the message asked for
    /// an action the wrapper had already taken. Only a first-time `./COMPILE.sh` gets the manual
    /// instruction, because only then is there no wrapper to reload.
    #[test]
    fn the_closing_line_tracks_whether_a_reload_will_follow() {
        let sourcefile = Path::new("/home/u/.bashrs/sourcefile.sh");

        let reloading = activation_line(true, sourcefile, None);
        assert!(reloading.contains("reloading"), "the recompile path says a reload is happening");
        assert!(
            !reloading.contains("Open a new shell") && !reloading.contains("run:"),
            "and does NOT ask the user to do what the wrapper already does: {reloading}"
        );

        let manual = activation_line(false, sourcefile, None);
        assert!(manual.contains("Open a new shell"), "first-install names the manual step");
        assert!(manual.contains(&sourcefile.display().to_string()), "with the sourcing command");
        // The command is set off in backticks for readability. Safe here and ONLY here because
        // this goes out via `println!` — the sourcefile's nag, which a shell evaluates, must not.
        assert!(
            manual.contains("`. \"/home/u/.bashrs/sourcefile.sh\"`"),
            "the runnable command is quoted as code: {manual}"
        );
        assert!(!manual.contains("fish"), "fish is not mentioned where it was not wired: {manual}");

        // Wired for fish too: its own command beside bash's, and still nothing on the reload path.
        let fish = Path::new("/home/u/.bashrs/sourcefile.fish");
        let both = activation_line(false, sourcefile, Some(fish));
        assert!(both.contains("`. \"/home/u/.bashrs/sourcefile.sh\"`"), "{both}");
        assert!(both.contains("(in fish:  `source \"/home/u/.bashrs/sourcefile.fish\"`)"), "{both}");
        assert!(!activation_line(true, sourcefile, Some(fish)).contains("source"), "a reload needs no command");
    }

    /// Fish's own rule for where its configuration lives, minus the one case no installer can
    /// follow: a relative `XDG_CONFIG_HOME`, which fish would read against its own working directory.
    #[test]
    fn fish_config_is_found_where_fish_looks_for_it() {
        let home = Path::new("/home/u");
        assert_eq!(fish_config_dir(None, home), Path::new("/home/u/.config/fish"));
        assert_eq!(fish_config_dir(Some("/xdg".as_ref()), home), Path::new("/xdg/fish"));
        assert_eq!(fish_config_dir(Some("relative".as_ref()), home), Path::new("/home/u/.config/fish"), "ignored, per the XDG spec");
        assert_eq!(fish_config_dir(Some("".as_ref()), home), Path::new("/home/u/.config/fish"), "set but empty is unset");
    }

    /// A name collides exactly when a `<name>.fish` file sits in one of fish's completion directories —
    /// a directory of that name, or a file for another name, does not count.
    #[test]
    fn a_name_collides_where_fish_has_a_completion_file_for_it() {
        let dir = scratch("colliding");
        let (shipped, vendor) = (dir.join("completions"), dir.join("vendor"));
        std::fs::create_dir_all(shipped.join("gg.fish")).unwrap(); // a directory, not a file
        std::fs::create_dir_all(&vendor).unwrap();
        std::fs::write(shipped.join("diff.fish"), "").unwrap();
        std::fs::write(vendor.join("hg.fish"), "").unwrap();
        std::fs::write(vendor.join("unrelated.fish"), "").unwrap();
        let completed: Vec<String> = ["diff", "gg", "hg", "lll"].map(String::from).to_vec();
        assert_eq!(colliding(&completed, &[shipped, vendor], &BTreeSet::new()), ["diff", "hg"]);
        assert!(colliding(&completed, &[], &BTreeSet::new()).is_empty(), "no fish, no collisions");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Fish 4.9 keeps its own completions inside its binary, off the completion path: a name it
    /// completes there collides as a file on the path would — and only a whole name counts.
    #[test]
    fn a_completion_built_into_fish_collides_too() {
        let built_in = built_in_names("completions/adiff.fish\ncompletions/diff.fish\ncompletions/hg.fish\nfunctions/ls.fish\ncompletions/.fish\n");
        assert_eq!(built_in, BTreeSet::from(["adiff".to_string(), "diff".to_string(), "hg".to_string()]), "completions only, by name");
        let completed: Vec<String> = ["diff", "gg", "hg", "lll"].map(String::from).to_vec();
        assert_eq!(colliding(&completed, &[], &built_in), ["diff", "hg"]);
        assert!(built_in_names("").is_empty(), "an older fish lists nothing");
    }

    /// The stand-in files track the collisions: written for each, removed when a name stops
    /// colliding, the directory gone once nothing is left — and nothing but `*.fish` touched.
    #[test]
    fn the_stand_in_completions_follow_the_collisions() {
        let dir = scratch("shadow").join("fish_completions");
        assert_eq!(shadow_fish_completions(&dir, &[]), Ok(()));
        assert!(!dir.exists(), "no collisions: no directory at all");

        assert_eq!(shadow_fish_completions(&dir, &["diff", "hg"]), Ok(()));
        let diff = std::fs::read_to_string(dir.join("diff.fish")).unwrap();
        assert!(diff.starts_with("# Generated by bashrs's install") && diff.contains("completes `diff` itself"), "{diff}");
        assert!(diff.lines().all(|line| line.starts_with('#')), "it defines nothing — bashrs's are in the sourcefile");
        assert!(dir.join("hg.fish").is_file());

        assert_eq!(shadow_fish_completions(&dir, &["hg"]), Ok(()));
        assert!(!dir.join("diff.fish").exists(), "a name that stopped colliding loses its file");
        std::fs::write(dir.join("notes.txt"), "mine").unwrap();
        assert_eq!(shadow_fish_completions(&dir, &[]), Ok(()));
        assert!(!dir.join("hg.fish").exists());
        assert!(dir.join("notes.txt").is_file(), "only bashrs's own files are removed — and the directory stays for it");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    /// Fish is wired only once it has a configuration directory — fish having been run — and then
    /// through a file of bashrs's own in conf.d, so `config.fish` is never touched. Created when
    /// absent, left alone on a re-run, and appended to like an rc file when the user made one.
    #[test]
    fn fish_is_wired_through_its_conf_d_and_only_where_fish_has_run() {
        let dir = scratch("fish");
        let config = dir.join("fish");
        assert_eq!(wire_fish(&config), Ok(Wired::Absent), "no fish config: fish was never run here");
        assert!(!config.exists(), "and nothing is created for a shell nobody uses");

        std::fs::create_dir_all(&config).unwrap();
        assert_eq!(wire_fish(&config), Ok(Wired::Added));
        let hook = config.join("conf.d/bashrs.fish");
        let wired = std::fs::read_to_string(&hook).unwrap();
        assert_eq!(
            wired,
            "# >>> bashrs >>>\ntest -r \"$HOME/.bashrs/sourcefile.fish\"; and source \"$HOME/.bashrs/sourcefile.fish\"\n# <<< bashrs <<<\n",
            "the exact guarded source line, literal $HOME"
        );
        assert!(!config.join("config.fish").exists(), "the user's config.fish is never created or edited");
        assert_eq!(wire_fish(&config), Ok(Wired::AlreadyPresent), "re-runs find the marker and skip");
        assert_eq!(std::fs::read_to_string(&hook).unwrap(), wired, "idempotent");

        // A bashrs.fish the user wrote themselves keeps its content; the block joins it.
        std::fs::write(&hook, "# mine\n").unwrap();
        assert_eq!(wire_fish(&config), Ok(Wired::Added));
        assert!(std::fs::read_to_string(&hook).unwrap().starts_with("# mine\n\n# >>> bashrs >>>\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The reload verdict comes off the process tree, so the recognizer decides everything: it
    /// must accept the wrapper's absolute-path invocation and reject look-alikes — a program
    /// merely *handed* the word (`echo brs_compile`) is not a compile.
    #[test]
    fn only_a_real_brs_compile_invocation_counts_as_a_pending_reload() {
        let argv = |parts: &[&str]| parts.iter().map(|p| p.to_string()).collect::<Vec<_>>();
        assert!(
            is_brs_compile(&argv(&["/home/u/.bashrs/bashrs", "brs_compile"])),
            "the wrapper invokes the binary by absolute path"
        );
        assert!(is_brs_compile(&argv(&["bashrs", "brs_compile", "--use-stable-cargo"])));
        // Not a compile: another bashrs command, another program, or the word as mere data.
        assert!(!is_brs_compile(&argv(&["/home/u/.bashrs/bashrs", "brs_test"])));
        assert!(!is_brs_compile(&argv(&["echo", "brs_compile"])));
        assert!(!is_brs_compile(&argv(&["bashrs"])));
        assert!(!is_brs_compile(&[]));
    }

    /// The walk must terminate and stay honest on a machine it can't read: a bounded chain that
    /// ends at init, and no claim of a reload when the ancestry says nothing about one. (The test
    /// binary is not run from `brs_compile`, so the answer here is false.)
    #[test]
    fn the_ancestry_walk_terminates_and_claims_no_reload_by_default() {
        let ancestors = ancestor_argvs();
        assert!(ancestors.len() <= 16, "the walk is bounded: {}", ancestors.len());
        assert!(!reload_follows(), "a plain `cargo test` has no pending shell reload");
    }

    #[test]
    fn the_home_guard_refuses_only_a_dangling_symlink() {
        let dir = scratch("homeguard");
        let fresh = dir.join("fresh/.bashrs");
        assert_eq!(ensure_home(&fresh), Ok(()), "a missing home is simply created");
        assert!(fresh.is_dir());

        let dangling = dir.join(".bashrs_link");
        std::os::unix::fs::symlink(dir.join("no/such/target"), &dangling).unwrap();
        assert!(ensure_home(&dangling).unwrap_err().contains("symlink to a missing target"));

        let live_target = dir.join("dotfiles_bashrs");
        std::fs::create_dir_all(&live_target).unwrap();
        let live = dir.join(".bashrs_live_link");
        std::os::unix::fs::symlink(&live_target, &live).unwrap();
        assert_eq!(ensure_home(&live), Ok(()), "a symlink into a live dir is written through");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The binds-profile store exists from the first install on — empty is fine, missing is
    /// not: it's what makes the store discoverable, and what a bare `autokey_apply` passes.
    #[test]
    fn installing_creates_the_profile_store() {
        let home = scratch("profilestore");
        assert_eq!(ensure_profile_store(&home), Ok(()));
        assert!(home.join("user-custom/sequencer").is_dir());
        // Idempotent, like the rest of the install: a second run changes nothing.
        assert_eq!(ensure_profile_store(&home), Ok(()));
        let _ = std::fs::remove_dir_all(&home);
    }

    fn read(path: PathBuf) -> String {
        std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
    }

    /// An install over the old layout moves it once: `user-data` becomes `user-appdata` whole —
    /// the cookies, the vendor list, and whatever else the user kept there — and the profile
    /// store moves on to `user-custom`. The next install finds nothing left to move.
    #[test]
    fn the_old_user_data_moves_to_appdata_and_its_profiles_to_custom_once() {
        let home = scratch("migrate");
        std::fs::create_dir_all(home.join("user-data/browser_cookies/youtube")).unwrap();
        std::fs::create_dir_all(home.join("user-data/sequencer/sub")).unwrap();
        std::fs::write(home.join("user-data/oui.tsv"), "vendors").unwrap();
        std::fs::write(home.join("user-data/owned-games.txt"), "Portal 2").unwrap();
        std::fs::write(home.join("user-data/sequencer/sub/binds.toml"), "[binds]").unwrap();

        let said = migrate_user_dirs(&home);
        assert!(said.len() == 2 && said.iter().all(Result::is_ok), "{said:?}");
        assert!(home.join("user-data").symlink_metadata().is_err(), "the old name is gone");
        assert_eq!(read(home.join("user-appdata/oui.tsv")), "vendors");
        assert_eq!(read(home.join("user-appdata/owned-games.txt")), "Portal 2", "the user's own additions move too");
        assert!(home.join("user-appdata/browser_cookies/youtube").is_dir());
        assert!(home.join("user-appdata/sequencer").symlink_metadata().is_err(), "the profiles moved on");
        assert_eq!(read(home.join("user-custom/sequencer/sub/binds.toml")), "[binds]");
        assert!(migrate_user_dirs(&home).is_empty(), "once moved, nothing more to do");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A symlinked `user-data` — onto another disk, or into a dotfiles checkout — moves as the
    /// link: the link takes the new name, and what it points at stays where it is.
    #[test]
    fn a_symlinked_user_data_moves_as_the_link() {
        let base = scratch("migrate_link");
        let elsewhere = base.join("elsewhere");
        std::fs::create_dir_all(elsewhere.join("browser_cookies")).unwrap();
        let home = base.join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::os::unix::fs::symlink(&elsewhere, home.join("user-data")).unwrap();

        let said = migrate_user_dirs(&home);
        assert!(said.iter().all(Result::is_ok), "{said:?}");
        assert_eq!(std::fs::read_link(home.join("user-appdata")).unwrap(), elsewhere, "the link, renamed");
        assert!(elsewhere.join("browser_cookies").is_dir(), "its target untouched");

        // A link is never "empty", even to an empty directory: with the new name in use, it is
        // a warning, and the link stays as it is.
        let empty = base.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        std::os::unix::fs::symlink(&empty, home.join("user-data")).unwrap();
        let said = migrate_user_dirs(&home);
        assert!(said.len() == 1 && said[0].is_err(), "{said:?}");
        assert_eq!(std::fs::read_link(home.join("user-data")).unwrap(), empty, "the link stays");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Both names holding something is never merged automatically: a warning names both, and
    /// neither moves. An empty directory on either side is no obstacle — nothing to merge — be
    /// it the empty store an install made before the move, or an old name left empty.
    #[test]
    fn two_names_in_use_are_left_to_the_user_and_an_empty_one_is_no_obstacle() {
        let home = scratch("migrate_both");
        std::fs::create_dir_all(home.join("user-data")).unwrap();
        std::fs::write(home.join("user-data/old.txt"), "old").unwrap();
        std::fs::create_dir_all(home.join("user-appdata")).unwrap();
        std::fs::write(home.join("user-appdata/new.txt"), "new").unwrap();
        let said = migrate_user_dirs(&home);
        let warnings: Vec<&String> = said.iter().filter_map(|line| line.as_ref().err()).collect();
        assert!(said.len() == 1 && warnings.len() == 1, "{said:?}");
        assert!(warnings[0].contains("user-data") && warnings[0].contains("user-appdata") && warnings[0].contains("by hand"), "{said:?}");
        assert_eq!(read(home.join("user-data/old.txt")), "old", "nothing moved");
        assert_eq!(read(home.join("user-appdata/new.txt")), "new");

        let home = scratch("migrate_empty");
        std::fs::create_dir_all(home.join("user-appdata/sequencer")).unwrap();
        std::fs::write(home.join("user-appdata/sequencer/a.toml"), "a").unwrap();
        std::fs::create_dir_all(home.join("user-custom/sequencer")).unwrap();
        let said = migrate_user_dirs(&home);
        assert!(said.len() == 1 && said.iter().all(Result::is_ok), "{said:?}");
        assert_eq!(read(home.join("user-custom/sequencer/a.toml")), "a", "moved into the empty store's place");
        std::fs::create_dir_all(home.join("user-appdata/sequencer")).unwrap();
        assert!(migrate_user_dirs(&home).is_empty(), "an empty old name is just removed, quietly");
        assert!(home.join("user-appdata/sequencer").symlink_metadata().is_err());
        assert_eq!(read(home.join("user-custom/sequencer/a.toml")), "a");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn the_binary_self_installs_and_reinstalls_over_the_previous_copy() {
        let dir = scratch("selfinstall");
        let installed = install_binary(&dir).expect("install works").expect("a copy was made");
        assert_eq!(installed, dir.join("bashrs"));
        let source_len = std::fs::metadata(std::env::current_exe().unwrap()).unwrap().len();
        assert_eq!(std::fs::metadata(&installed).unwrap().len(), source_len, "a full copy");
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&installed).unwrap().permissions().mode();
        assert_eq!(mode & 0o755, 0o755, "executable for everyone, like install -m 755");

        // A re-run replaces the previous copy (unlink-first, so a running old binary survives).
        std::fs::write(&installed, b"stale").unwrap();
        install_binary(&dir).expect("reinstall works").expect("replaced");
        assert_eq!(std::fs::metadata(&installed).unwrap().len(), source_len, "stale copy replaced");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A symlinked install is replaced where it points and the link survives — the case that
    /// used to lose the link, unlinking it and leaving a regular file in its place. A dangling
    /// link has nowhere to be written through, so it is replaced as before; and a link straight
    /// at the running binary is recognised as installed rather than unlinked from under itself.
    #[test]
    fn a_symlinked_binary_is_replaced_where_it_points_and_the_link_survives() {
        use std::os::unix::fs::symlink;
        let dir = scratch("symlinked");
        let (home, elsewhere) = (dir.join("home"), dir.join("elsewhere"));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        let real = elsewhere.join("bashrs");
        std::fs::write(&real, b"stale").unwrap();
        symlink(&real, home.join("bashrs")).unwrap();

        let source_len = std::fs::metadata(std::env::current_exe().unwrap()).unwrap().len();
        let installed = install_binary(&home).expect("install works").expect("a copy was made");
        assert_eq!(installed, real.canonicalize().unwrap(), "written where the link points");
        assert!(home.join("bashrs").is_symlink(), "and the link itself survives");
        assert_eq!(std::fs::metadata(&real).unwrap().len(), source_len, "a full copy, through it");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&real).unwrap().permissions().mode() & 0o755, 0o755);

        std::fs::remove_file(home.join("bashrs")).unwrap();
        symlink(dir.join("no/such/bashrs"), home.join("bashrs")).unwrap();
        install_binary(&home).expect("install works").expect("a copy was made");
        assert!(!home.join("bashrs").is_symlink(), "a dangling link is replaced, as before");
        assert_eq!(std::fs::metadata(home.join("bashrs")).unwrap().len(), source_len);

        std::fs::remove_file(home.join("bashrs")).unwrap();
        symlink(std::env::current_exe().unwrap(), home.join("bashrs")).unwrap();
        assert_eq!(install_binary(&home), Ok(None), "linked at the running binary: installed");
        assert!(home.join("bashrs").is_symlink());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_absent_rc_is_skipped_and_a_present_one_is_wired_exactly_once() {
        let dir = scratch("wire");
        let rc = dir.join(".bashrc");
        assert_eq!(wire_rc(&rc), Ok(Wired::Absent), "only files the user has are touched");

        std::fs::write(&rc, "# my things\nalias ll='ls -l'\n").unwrap();
        assert_eq!(wire_rc(&rc), Ok(Wired::Added));
        let wired = std::fs::read_to_string(&rc).unwrap();
        assert!(wired.starts_with("# my things\n"), "the user's content is preserved, block appended");
        assert!(
            wired.ends_with("\n# >>> bashrs >>>\n[ -r \"$HOME/.bashrs/sourcefile.sh\" ] && . \"$HOME/.bashrs/sourcefile.sh\"\n# <<< bashrs <<<\n"),
            "the exact guarded source line, literal $HOME: {wired}"
        );

        assert_eq!(wire_rc(&rc), Ok(Wired::AlreadyPresent), "re-runs find the marker and skip");
        assert_eq!(std::fs::read_to_string(&rc).unwrap(), wired, "idempotent: nothing re-appended");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oddities_are_errors_not_silent_skips() {
        let dir = scratch("odd");
        let as_dir = dir.join(".bashrc");
        std::fs::create_dir_all(&as_dir).unwrap();
        assert!(wire_rc(&as_dir).unwrap_err().contains("not a regular file"));

        let readonly = dir.join(".zshrc");
        std::fs::write(&readonly, "# locked\n").unwrap();
        let mut perms = std::fs::metadata(&readonly).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o444);
        std::fs::set_permissions(&readonly, perms).unwrap();
        assert!(wire_rc(&readonly).unwrap_err().contains("failed to append"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
