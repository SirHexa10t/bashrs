//! Self-contained external tools, bundled under `~/.bashrs/tools/` — so commands that lean on a
//! heavyweight program (ffmpeg, python) work without asking the user to install anything, and so
//! the *project* controls the versions it relies on (system copies drift — apt lags — and may
//! lack features we use). Its own pillar (not `support`) because it spans three concerns:
//! compile-time acquisition ([`fetch`], run by the `install-stainless` command), runtime resolution
//! ([`resolve`], used by [`crate::drivers`] and the command categories), and the interactive
//! side — a shim directory
//! ([`bin_dir`]) whose PATH prepend is emitted into `sourcefile.sh` ([`shell_setup`], used by
//! [`crate::cli`]). The bundled copy is the project-pinned default; the system installation is
//! the fallback — except that an activated venv still wins for interactive `python3`.

mod fetch;

pub use fetch::sync;

use std::ffi::OsString;
use std::path::PathBuf;

/// A bundled tool: where it lives, what it provides, and how its bundle comes to exist.
struct Tool {
    /// Directory under `~/.bashrs/tools/` holding the bundle.
    dir: &'static str,
    /// The programs this tool provides: `(name on PATH, path inside `dir`)`. The first one
    /// doubles as the probe for "already installed / already bundled".
    bins: &'static [(&'static str, &'static str)],
    /// How the bundle is acquired and kept current.
    acquire: Acquire,
    /// Which `always_bundle_*` configuration group governs it.
    group: Group,
}

/// The configuration groups of [`Tool`]s — each with its own `[tools] always_bundle_*` flag.
/// Languages default to bundled (bashrs commands and companion repos run on them — near
/// non-negotiable); utilities default to bundle-only-what's-missing.
#[derive(Clone, Copy)]
enum Group {
    Language,
    Utility,
}

/// How a tool's bundle comes to exist (and updates). The URL functions take an optional
/// version pin (a Carstay.toml value, in `--use-stable-carstay` mode) — `None` means the latest release.
enum Acquire {
    /// Download a static archive (URL discovered per release, root folder stripped on unpack);
    /// re-fetched when the published URL changes, tracked by a `.source_url` marker. Tools whose
    /// download URL alone can't name an exact build (ffmpeg's rolling `latest` tag) also carry a
    /// `dated_tag` resolver — its result is stored beside the bundle and lands in Carstay.toml,
    /// so `--use-stable-carstay` can restore the exact build, not just the channel.
    Archive { url: fn(Option<&str>) -> Option<String>, dated_tag: Option<fn() -> Option<String>> },
    /// A single released binary — same URL-discovery and `.source_url` freshness contract as
    /// [`Acquire::Archive`], but the download *is* the program (written to the tool's first
    /// `bins` path, made executable).
    Binary(fn(Option<&str>) -> Option<String>),
    /// A `uv venv` at the tool's dir — a stable `bin/python3` over an interpreter uv installs
    /// into [`interpreters_dir`]; kept current via `uv python upgrade`.
    UvVenv { python: &'static str },
}

/// Bundled in table order — `uv` must precede `python`, whose venv step runs it.
const TOOLS: &[Tool] = &[
    Tool {
        dir: "ffmpeg",
        bins: &[("ffmpeg", "bin/ffmpeg"), ("ffprobe", "bin/ffprobe")],
        // BtbN's rolling `latest` URL can't name an exact build — the dated autobuild tag can.
        acquire: Acquire::Archive { url: fetch::ffmpeg_url, dated_tag: Some(fetch::ffmpeg_dated_tag) },
        group: Group::Utility,
    },
    Tool {
        dir: "yt-dlp",
        bins: &[("yt-dlp", "bin/yt-dlp")],
        acquire: Acquire::Binary(fetch::ytdlp_url),
        group: Group::Utility,
    },
    // uv's archive carries the binaries at its root (no bin/); it manages the python below —
    // which is why it belongs to the Language group despite being a utility in shape.
    Tool {
        dir: "uv",
        bins: &[("uv", "uv"), ("uvx", "uvx")],
        acquire: Acquire::Archive { url: fetch::uv_url, dated_tag: None },
        group: Group::Language,
    },
    Tool {
        dir: "python",
        bins: &[("python3", "bin/python3")],
        acquire: Acquire::UvVenv { python: "3.14" },
        group: Group::Language,
    },
    // deno exists here to serve yt-dlp: YouTube extraction needs a JS runtime (EJS) or formats
    // go missing. Listed after python on purpose — its release is a .zip, and the fetcher
    // unpacks those with the python bundled just above.
    Tool {
        dir: "deno",
        bins: &[("deno", "deno")],
        acquire: Acquire::Archive { url: fetch::deno_url, dated_tag: None },
        group: Group::Utility,
    },
];

/// `~/.bashrs/tools/interpreters` — where uv keeps the managed CPython builds the python venv
/// links against (`UV_PYTHON_INSTALL_DIR`), so the whole python story stays under `~/.bashrs`.
pub(crate) fn interpreters_dir() -> PathBuf {
    root().join("interpreters")
}

/// `~/.bashrs/tools` — the bundled tools' home.
pub(crate) fn root() -> PathBuf {
    crate::conf::bashrs_home().join("tools")
}

/// Each tool's provisioned version, in table order — derived per acquisition mode by
/// [`fetch::provisioned_version`] (which owns the on-disk markers). `None` means nothing is
/// bundled — the system installation serves that tool. Feeds [`crate::drivers::carstay`]'s
/// manifest.
pub fn versions() -> Vec<(&'static str, Option<String>)> {
    TOOLS
        .iter()
        .map(|tool| (tool.dir, fetch::provisioned_version(tool, &root().join(tool.dir))))
        .collect()
}

/// Where bashrs points uv's cache: beside what the cache feeds, so both share a filesystem.
///
/// `~/.bashrs` is commonly a symlink onto another disk, and uv installs by *hardlinking* out
/// of its cache — links cannot cross filesystems, so uv's stock cache location (`~/.cache/uv`,
/// on the home disk) degrades every install into a full copy and prints a warning on every
/// compile. Keeping the cache under [`root`] restores the links, and it travels with the disk:
/// one less piece of bashrs state a machine migration would strand in `~/.cache`.
///
/// `None` when the user has set `UV_CACHE_DIR` themselves — their placement wins. Scoped to
/// bashrs's own uv invocations on purpose: personal uv work targets home-disk projects, where
/// the default cache location hardlinks fine.
fn uv_cache_dir(existing: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    existing.is_none().then(|| root().join("uv-cache"))
}

/// The bundled `uv`, with its cache pinned onto the bashrs disk (see [`uv_cache_dir`]).
///
/// Every place bashrs drives uv resolves it through here rather than through [`resolve`], so
/// the cache policy exists exactly once. Set process-wide because the exec helpers the callers
/// use take a program, not an environment — and the variable means nothing to any other child.
pub(crate) fn resolve_uv() -> OsString {
    if let Some(dir) = uv_cache_dir(std::env::var_os("UV_CACHE_DIR").as_deref()) {
        std::env::set_var("UV_CACHE_DIR", dir);
    }
    resolve("uv")
}

/// The command to run for `program`: its bundled copy when present — the project-pinned version,
/// kept current by `bashrs_compile` — else the bare name, so the system installation (or its
/// normal "command not found" error) takes over. A program not in the table passes through.
pub(crate) fn resolve(program: &str) -> OsString {
    for tool in TOOLS {
        for (name, rel) in tool.bins {
            if *name == program {
                let bundled = root().join(tool.dir).join(rel);
                if bundled.exists() {
                    return bundled.into_os_string();
                }
            }
        }
    }
    OsString::from(program)
}

/// `~/.bashrs/tools/bin` — one symlink per bundled binary (maintained by [`fetch`]), prepended to
/// PATH by the sourcefile. PATH is the only mechanism that also covers *argument* position —
/// `which python3`, `env python3`, `xargs`, `#!/usr/bin/env python3` shebangs — where a shell
/// function or alias is invisible.
pub(crate) fn bin_dir() -> PathBuf {
    root().join("bin")
}

/// The setup emitted into `sourcefile.sh` — the interactive side of the bundled tools, in two
/// layers. The PATH prepend (idempotent; harmless when nothing is bundled) makes the shims win in
/// *argument* position — `which`, `env`, shebangs. The `python3` function covers what PATH can't:
/// sourced last in the rc chain, it replaces any same-named function the user's own rc defined
/// earlier (the `unalias` clears an alias shadow likewise). An activated venv still wins either
/// way, and a machine with no bundle falls through to its PATH python.
/// The tools root as the *generated shell* spells it — `$HOME` expanded at use time (so a
/// symlinked `~/.bashrs` keeps working). The one shell-side spelling of the directory
/// [`root`] owns on the Rust side, shared by the PATH/shim setup below and the shim scripts
/// [`fetch`] writes.
const TOOLS_ROOT_SHELL: &str = "$HOME/.bashrs/tools";

pub(crate) fn shell_setup() -> String {
    let root = TOOLS_ROOT_SHELL;
    format!(
        "case \":$PATH:\" in *\":{root}/bin:\"*) ;; *) export PATH=\"{root}/bin:$PATH\";; esac  \
         # bundled tools (python3, ffmpeg, …) win by PATH — as arguments and in shebangs\n\
         unalias python3 2>/dev/null || true  \
         # own line, on purpose: an active alias would mangle the function definition at parse time\n\
         python3() {{ if [ -z \"$VIRTUAL_ENV\" ] && [ -x \"{root}/bin/python3\" ]; \
         then \"{root}/bin/python3\" \"$@\"; else command python3 \"$@\"; fi; }}  \
         # defined last so it beats older same-named rc definitions; a venv (or a missing bundle) falls through to PATH\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cache policy, tested as the pure decision — the setter is a thin shell around it,
    /// and mutating the real environment in a parallel test suite is a race.
    #[test]
    fn uv_cache_lands_on_the_bashrs_disk_unless_the_user_placed_it() {
        let ours = uv_cache_dir(None).expect("unset: bashrs decides");
        assert!(
            ours.ends_with("tools/uv-cache"),
            "beside what it feeds, so hardlinks work across a symlinked ~/.bashrs: {}",
            ours.display()
        );
        assert!(ours.starts_with(crate::conf::bashrs_home()), "and it migrates with the disk");
        assert_eq!(
            uv_cache_dir(Some(std::ffi::OsStr::new("/somewhere/mine"))),
            None,
            "a user-set UV_CACHE_DIR wins"
        );
    }

    #[test]
    fn uv_is_listed_before_the_python_venv_that_needs_it() {
        let uv = TOOLS.iter().position(|tool| tool.dir == "uv").expect("uv row");
        let python = TOOLS.iter().position(|tool| tool.dir == "python").expect("python row");
        assert!(uv < python, "sync bundles in table order; the python venv step runs uv");
    }

    #[test]
    fn resolve_prefers_the_bundle_and_falls_back_to_the_bare_name() {
        // A tool program resolves per-environment: to its bundle when fetched, else to itself.
        let ffmpeg = resolve("ffmpeg");
        if root().join("ffmpeg/bin/ffmpeg").exists() {
            assert!(ffmpeg.to_string_lossy().ends_with(".bashrs/tools/ffmpeg/bin/ffmpeg"), "{ffmpeg:?}");
        } else {
            assert_eq!(ffmpeg, OsString::from("ffmpeg"));
        }
        // Programs outside the table pass through, so the shell's own error stays meaningful.
        assert_eq!(resolve("sh"), OsString::from("sh"));
        assert_eq!(resolve("no_such_tool_xyz"), OsString::from("no_such_tool_xyz"));
    }

    #[test]
    fn the_shell_setup_prepends_the_shim_dir_idempotently_and_arms_the_function() {
        let setup = shell_setup();
        assert!(setup.contains("PATH=\"$HOME/.bashrs/tools/bin:$PATH\""), "{setup}");
        assert!(setup.starts_with("case \":$PATH:\" in"), "re-sourcing must not stack duplicates: {setup}");
        // The function layer: beats older same-named rc functions/aliases, respects venvs,
        // routes through the shim so both layers name one canonical binary.
        assert!(setup.contains("unalias python3"), "{setup}");
        assert!(setup.contains("python3() {"), "{setup}");
        assert!(setup.contains("VIRTUAL_ENV"), "{setup}");
        assert!(setup.contains("$HOME/.bashrs/tools/bin/python3"), "{setup}");
        assert!(setup.contains("command python3"), "fall-throughs bypass the function itself: {setup}");
    }
}
