//! The install itself — the tail end of COMPILE.sh, moved into one testable implementation:
//! guard and create `~/.bashrs`, install the running binary into it (self-copy), write the
//! generated sourcefile, then wire each rc file the user has to source it (idempotent-by-marker,
//! so re-runs find the block and skip). Run via the hidden `install-shell` command, COMPILE.sh's
//! last step — invoked from the freshly built `target/release/bashrs`, which is what makes the
//! self-copy the installation.

use std::path::{Path, PathBuf};

/// The sourcefile path as written INTO rc files — deliberately UNEXPANDED: the literal `$HOME`
/// expands when the rc file is *sourced*, so the wired line stays correct across machines and
/// users sharing one rc (dotfile repos), where an expanded home would freeze this machine's
/// path in. (COMPILE.sh's old `SRC_PATH`, renamed for what it is.)
const UNEXPANDED_SOURCEFILE_PATH: &str = "$HOME/.bashrs/sourcefile.sh";

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

/// SIDE EFFECTS — the whole install: guard and create `~/.bashrs`, copy the running binary in,
/// write `content` (the generated wrappers) to `~/.bashrs/sourcefile.sh`, then wire `.bashrc`
/// and `.zshrc`. Any failure prints and exits non-zero, aborting COMPILE.sh like its old `die`
/// did.
pub fn install_shell(content: &str) {
    let home = super::bashrs_home();
    let die = |msg: String| -> ! {
        eprintln!("ERROR: {msg}");
        std::process::exit(1);
    };
    if let Err(msg) = ensure_home(&home) {
        die(msg);
    }
    match install_binary(&home) {
        Ok(Some(bin)) => println!("Installed {}", bin.display()),
        Ok(None) => {} // already running as the installed copy — nothing to move
        Err(msg) => die(msg),
    }
    let sourcefile = super::sourcefile();
    if let Err(err) = std::fs::write(&sourcefile, content) {
        die(format!("could not write {}: {err}", sourcefile.display()));
    }
    println!("Generated {}", sourcefile.display());

    // The user's own home now, not `~/.bashrs` — a different directory, so a distinct name.
    let user_home = super::home();
    let mut any_present = false;
    for rc in [user_home.join(".bashrc"), user_home.join(".zshrc")] {
        match wire_rc(&rc) {
            Ok(Wired::Absent) => {}
            Ok(Wired::AlreadyPresent) => {
                any_present = true;
                println!("Source block already present in {} — leaving it untouched.", rc.display());
            }
            Ok(Wired::Added) => {
                any_present = true;
                println!("Added source block to {}", rc.display());
            }
            Err(msg) => {
                eprintln!("ERROR: {msg}");
                std::process::exit(1);
            }
        }
    }
    if !any_present {
        println!("Note: neither ~/.bashrc nor ~/.zshrc exists; nothing was wired up.");
    }
    println!();
    // A `bashrs_compile` (the everyday recompile) reloads the shell for you the instant this
    // returns, so telling the user to open one themselves is wrong — it already happens. Only a
    // run with no reload coming (a first-time `./COMPILE.sh`, or the script run by hand) needs
    // the manual instruction. Which one this is is read off the process tree, where the fact
    // already lives — see [`reload_follows`].
    println!("{}", activation_line(reload_follows(), &sourcefile));
}

/// The install's closing line. When a shell reload will follow (the `bashrs_compile` flow),
/// say so — the prompt is about to reset under the user, and asking them to act would contradict
/// what's happening. Otherwise (a first-time `./COMPILE.sh`), name the one step that activates it.
///
/// The backticks around the command are display only: this string leaves through `println!`, not
/// through a shell, so nothing here is ever evaluated — unlike the sourcefile's stale-config nag,
/// where a backtick once meant command substitution at every prompt.
fn activation_line(reload_follows: bool, sourcefile: &Path) -> String {
    if reload_follows {
        "Done — reloading your shell.".to_string()
    } else {
        format!("Done. Open a new shell, or run:  `. \"{}\"`", sourcefile.display())
    }
}

/// Whether the shell will reload on its own once this install returns — true exactly when a
/// `bashrs_compile` invocation sits in this process's ancestry (its generated wrapper catches the
/// reload exit code and runs `shell_new`). Read straight off `/proc`'s parent chain rather than
/// signalled through an env var: the process tree already carries the fact, and a second channel
/// for it could only drift or leak. The chain here is short — install-shell ← bash(COMPILE.sh) ←
/// bashrs(bashrs_compile) ← the user's shell — but the walk is bounded anyway.
fn reload_follows() -> bool {
    ancestor_argvs().into_iter().any(|argv| is_bashrs_compile(&argv))
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

/// Whether one process's argv is a `bashrs_compile` run — the bashrs binary (by file name, since
/// the wrapper invokes it by absolute path) told to compile. Both halves matter: `bashrs_compile`
/// alone would also match some unrelated program handed that word as data.
fn is_bashrs_compile(argv: &[String]) -> bool {
    let binary_is_bashrs = argv
        .first()
        .map(Path::new)
        .and_then(Path::file_name)
        .is_some_and(|name| name == "bashrs");
    binary_is_bashrs && argv.get(1).is_some_and(|arg| arg == "bashrs_compile")
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

/// Install the running executable as `<home>/bashrs` — the self-copy that IS the installation
/// (COMPILE.sh runs this command from the freshly built `target/release/bashrs`). The old copy
/// is unlinked first, like `install(1)` did: overwriting a *running* executable in place fails
/// with ETXTBSY (the `bashrs_compile` flow keeps the old binary alive while this runs), while
/// unlinking leaves that process its inode and frees the name. `None` when already running AS
/// the installed copy — copying a file onto itself would truncate it.
fn install_binary(home: &Path) -> Result<Option<PathBuf>, String> {
    let source = std::env::current_exe().map_err(|err| format!("could not locate the running binary: {err}"))?;
    let dest = home.join("bashrs");
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
    if !rc.is_file() {
        return Err(format!("{} exists but is not a regular file.", rc.display()));
    }
    let current = std::fs::read(rc).map_err(|err| format!("could not read {}: {err}", rc.display()))?;
    if String::from_utf8_lossy(&current).contains(BLOCK_START) {
        return Ok(Wired::AlreadyPresent);
    }
    let block = format!(
        "\n{BLOCK_START}\n[ -r \"{path}\" ] && . \"{path}\"\n{BLOCK_END}\n",
        path = UNEXPANDED_SOURCEFILE_PATH
    );
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(rc)
        .and_then(|mut file| file.write_all(block.as_bytes()))
        .map_err(|err| format!("failed to append the source block to {}: {err}", rc.display()))?;
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

    /// The closing line must match what actually happens next. When `bashrs_compile` will reload
    /// the shell, it must NOT tell the user to open one — that was the bug: the message asked for
    /// an action the wrapper had already taken. Only a first-time `./COMPILE.sh` gets the manual
    /// instruction, because only then is there no wrapper to reload.
    #[test]
    fn the_closing_line_tracks_whether_a_reload_will_follow() {
        let sourcefile = Path::new("/home/u/.bashrs/sourcefile.sh");

        let reloading = activation_line(true, sourcefile);
        assert!(reloading.contains("reloading"), "the recompile path says a reload is happening");
        assert!(
            !reloading.contains("Open a new shell") && !reloading.contains("run:"),
            "and does NOT ask the user to do what the wrapper already does: {reloading}"
        );

        let manual = activation_line(false, sourcefile);
        assert!(manual.contains("Open a new shell"), "first-install names the manual step");
        assert!(manual.contains(&sourcefile.display().to_string()), "with the sourcing command");
        // The command is set off in backticks for readability. Safe here and ONLY here because
        // this goes out via `println!` — the sourcefile's nag, which a shell evaluates, must not.
        assert!(
            manual.contains("`. \"/home/u/.bashrs/sourcefile.sh\"`"),
            "the runnable command is quoted as code: {manual}"
        );
    }

    /// The reload verdict comes off the process tree, so the recognizer decides everything: it
    /// must accept the wrapper's absolute-path invocation and reject look-alikes — a program
    /// merely *handed* the word (`echo bashrs_compile`) is not a compile.
    #[test]
    fn only_a_real_bashrs_compile_invocation_counts_as_a_pending_reload() {
        let argv = |parts: &[&str]| parts.iter().map(|p| p.to_string()).collect::<Vec<_>>();
        assert!(
            is_bashrs_compile(&argv(&["/home/u/.bashrs/bashrs", "bashrs_compile"])),
            "the wrapper invokes the binary by absolute path"
        );
        assert!(is_bashrs_compile(&argv(&["bashrs", "bashrs_compile", "--use-stable-cargo"])));
        // Not a compile: another bashrs command, another program, or the word as mere data.
        assert!(!is_bashrs_compile(&argv(&["/home/u/.bashrs/bashrs", "bashrs_test"])));
        assert!(!is_bashrs_compile(&argv(&["echo", "bashrs_compile"])));
        assert!(!is_bashrs_compile(&argv(&["bashrs"])));
        assert!(!is_bashrs_compile(&[]));
    }

    /// The walk must terminate and stay honest on a machine it can't read: a bounded chain that
    /// ends at init, and no claim of a reload when the ancestry says nothing about one. (The test
    /// binary is not run from `bashrs_compile`, so the answer here is false.)
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
