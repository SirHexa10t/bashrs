//! The compile-time side of the bundled tools: [`sync`], run by the hidden `install-stainless` command from
//! `COMPILE.sh`, keeps every tool bundled *and current* — like the companion repos following
//! `main`, each compile discovers the latest published build and re-fetches when it changed
//! (tracked by a `.source_url` marker beside each bundle, so an unchanged build costs one API
//! call, not a re-download). Best-effort throughout: a failure is reported ([`Problems`], which
//! the compile repeats at its end) and keeps what's bundled, never aborts the compile. Asset URLs
//! are discovered from each project's latest GitHub release (ffmpeg's: BtbN's newest month-end
//! one) by name-shape, never hard-pinned — pinned names rot (a BtbN channel 404s once it rotates
//! out).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::conf::config_file;
use crate::support::exec;
use crate::support::problems::Problems;
use crate::tools::{bin_dir, root, Acquire, Group, Tool, TOOLS};

/// The per-bundle marker recording which published archive it came from.
pub(super) const SOURCE_MARKER: &str = ".source_url";

/// The marker an earlier bashrs wrote beside an ffmpeg bundle it fetched from BtbN's rolling
/// `latest` release, whose URL names no build: the dated tag that build came from. Nothing writes
/// it now — month-end URLs name their own build — but such a bundle is still read by it until its
/// next fetch, which removes it.
const TAG_MARKER: &str = ".build_tag";

/// SIDE EFFECTS — bundle the tools, keeping each current per its acquisition mode. The
/// `[tools] always_bundle_languages` / `always_bundle_utilities` settings decide, per group,
/// whether to bundle even what the system already provides (languages default to yes — bashrs
/// runs on them, so the project controlling their versions is the reliable default) or only
/// what the system lacks (the utilities' default).
/// `pins` (from Carstay.toml, via `--use-stable-carstay`) names the exact version to provision per tool —
/// a tool absent from it, or an empty slice, means "the latest published", today's default.
/// Returns whether anything new landed (a fetch or a fresh environment) — the caller's cue to
/// point at the stability-revert path. Whatever could not be done goes to `problems`.
pub fn sync(pins: &[(String, String)], problems: &mut Problems) -> bool {
    let bundle_languages = config_file::always_bundle_languages();
    let bundle_utilities = config_file::always_bundle_utilities();
    let install_extras = config_file::install_extras();

    // Plan first, on local information only: a config flag and a PATH probe decide whether this
    // machine bundles each tool at all, and Carstay names the version when pinned. Settling the
    // whole set up front costs nothing and keeps these messages in table order.
    // `None` — not bundled here; `Some(pin)` — bundled, at `pin`'s recorded version if any.
    // Beside it, whether the system has the tool itself.
    let (plan, on_system): (Vec<Option<Option<&str>>>, Vec<bool>) = TOOLS
        .iter()
        .map(|tool| {
            let probe = tool.bins[0].0;
            let system = system_copy(probe);
            let present = system.is_some();
            // Dependencies (Language/Utility) are bundled unless the flag says "only what's
            // missing" AND the system already has it. An Extra is the mirror: never bundled unless
            // opted in, and even then only to fill a gap — the user asked for the tool, not
            // necessarily for OUR copy over one they already have.
            let (wanted, field) = match tool.group {
                Group::Language => (bundle_languages || !present, "always_bundle_languages"),
                Group::Utility => (bundle_utilities || !present, "always_bundle_utilities"),
                Group::Extra => (install_extras && !present, "install_extras"),
            };
            if !wanted {
                // "You already have it" is worth saying; a switched-off opt-in extra is not —
                // that's the default state for everyone, and a line per compile would be noise.
                let opted_out_extra = matches!(tool.group, Group::Extra) && !install_extras;
                if let (Some(system), false) = (&system, opted_out_extra) {
                    eprintln!(
                        "tools: {probe} is already on this system ({}) — not bundling (override: [tools] {field} in {})",
                        system.display(),
                        config_file::CONFIG_FILE
                    );
                }
                return (None, present);
            }
            let pin = pins.iter().find(|(name, _)| name == tool.dir).map(|(_, pin)| pin.as_str());
            if let Some(pin) = pin {
                eprintln!("tools: {} pinned to the recorded {pin}", tool.dir);
            }
            (Some(pin), present)
        })
        .unzip();

    // A tool the settings leave to the system, while an earlier compile's bundle of it is still
    // there — made while the system lacked it, or while a setting said to bundle anyway — would
    // shadow the system's copy, since `resolve` and the PATH shim both prefer a bundle. The user
    // chose the system's, so the bundle goes, its shim with it. The python environment is the one
    // exception: it holds the user's own packages. And a tool the system lacks keeps its bundle
    // whatever the settings say — it is the only copy there is.
    for ((tool, planned), on_system) in TOOLS.iter().zip(&plan).zip(&on_system) {
        if planned.is_none() && *on_system && !matches!(tool.acquire, Acquire::UvVenv { .. }) {
            remove_leftover_bundle(tool, &root().join(tool.dir), problems);
        }
    }

    // Then discover every release URL at once. This phase IS what an up-to-date sync costs —
    // one GitHub API call per tool, nothing downloaded — so running the calls serially made the
    // no-op case pay their sum. They're independent read-only lookups (a `curl` of the releases
    // API, no shared state), so the wall clock becomes the slowest single call instead.
    let urls: Vec<Option<Result<String, LookupError>>> = std::thread::scope(|scope| {
        let handles: Vec<Option<_>> = TOOLS
            .iter()
            .zip(&plan)
            .map(|(tool, planned)| {
                let pin = (*planned)?;
                let resolve = match &tool.acquire {
                    Acquire::Archive(resolve) | Acquire::Binary(resolve) => *resolve,
                    // A venv has no published asset to look up — `uv python upgrade` is its
                    // whole freshness check, and it needs uv on disk, so it stays in the
                    // ordered phase below.
                    Acquire::UvVenv { .. } => return None,
                };
                Some(scope.spawn(move || resolve(pin)))
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle.map(|handle| handle.join().unwrap_or_else(|_| Err("the lookup panicked".into())))
            })
            .collect()
    });

    // Act in table order — load-bearing here even though discovery wasn't: uv must land before
    // the python venv whose creation runs it, and deno's zip is unpacked by that python. Each
    // step already holds its URL, so keeping the order now costs a comparison, not a round trip.
    let mut fetched_any = false;
    for ((tool, planned), url) in TOOLS.iter().zip(&plan).zip(&urls) {
        let Some(pin) = *planned else { continue };
        let dir = root().join(tool.dir);
        let fetched = match (&tool.acquire, url) {
            (Acquire::Archive(_), Some(url)) => sync_archive(tool, url, &dir, pin, problems),
            (Acquire::Binary(_), Some(url)) => sync_binary(tool, url, &dir, pin, problems),
            (Acquire::UvVenv { python }, _) => sync_python_venv(python, &dir, pin, problems),
            (_, None) => false, // never: every planned release tool was looked up above
        };
        // A one-time post-install runs ONLY on a fresh fetch, so it never repeats on a steady
        // compile — the tool is already current then, and `fetched` is false.
        if fetched {
            if let Some(activate) = tool.post_install {
                if let Err(why) = activate(&dir) {
                    problems.report(why);
                }
            }
        }
        fetched_any |= fetched;
    }
    // GitHub's hourly limit refuses every lookup alike: each tool's line says so, and what to
    // make of it is said once.
    if let Some(note) = rate_limit_note(&urls, &plan) {
        problems.report(note);
    }
    // Whatever is bundled — fetched just now or on an earlier compile — gets a PATH shim, so the
    // tools also win when named as arguments (see `tools::shell_setup`).
    for tool in TOOLS {
        ensure_shims(tool, problems);
    }
    fetched_any
}

/// The GitHub API requests a sync of `plan` makes — [`api_requests`] for each tool it bundles.
fn lookups_needed(plan: &[Option<Option<&str>>]) -> usize {
    TOOLS.iter().zip(plan).filter_map(|(tool, planned)| planned.map(|pin| api_requests(tool, pin))).sum()
}

/// What to make of GitHub's hourly limit, when it refused any of a sync's lookups — said once,
/// however many it refused: the limit's size (as the first refusal to carry one gave it, else the
/// documented one), what this compile's lookups need of it, and what helps. The limit counts per
/// IP address, so a VPN — another address — comes with its own; but a VPN address that many
/// people share may be spent as well. `None` when nothing was refused.
fn rate_limit_note(urls: &[Option<Result<String, LookupError>>], plan: &[Option<Option<&str>>]) -> Option<String> {
    let refusals: Vec<Option<u32>> = urls
        .iter()
        .flatten()
        .filter_map(|url| match url {
            Err(LookupError::RateLimited { limit, .. }) => Some(*limit),
            _ => None,
        })
        .collect();
    if refusals.is_empty() {
        return None;
    }
    let limit = refusals.into_iter().flatten().next().unwrap_or(GITHUB_HOURLY_LIMIT);
    let needed = lookups_needed(plan);
    Some(format!(
        "tools: without signing in, GitHub answers {limit} lookups an hour from one IP address, and this \
         compile's tool lookups need {needed} — the rest went to earlier compiles, or to other programs \
         or machines on this address. Compile again once it resets; a VPN gives this machine another \
         address, with {limit} of its own (unless many people share that one, and spent them already)."
    ))
}

/// This system's own `program`: the first copy on `PATH` that lies outside bashrs's tools tree
/// ([`root`]) — so a bundle's shim, which every bashrs shell puts on `PATH`, is never taken for
/// the system's, and the answer is the same from whichever shell the compile runs. `None` when
/// the system has none.
fn system_copy(program: &str) -> Option<PathBuf> {
    outside(exec::path_hits(program), &root())
}

/// The first of `hits` that, symlinks resolved, lies outside `tree` — which may itself be a
/// symlink (a `~/.bashrs` on another disk), so both sides are resolved before they are compared.
fn outside(hits: Vec<PathBuf>, tree: &Path) -> Option<PathBuf> {
    let tree = std::fs::canonicalize(tree).unwrap_or_else(|_| tree.to_path_buf());
    hits.into_iter().find(|hit| !std::fs::canonicalize(hit).unwrap_or_else(|_| hit.clone()).starts_with(&tree))
}

/// Remove `tool`'s bundle at `dir`, left by an earlier compile, now that the system's own copy is
/// the one to use — the bundle would shadow it. Its PATH shim follows: [`ensure_shims`] prunes a
/// shim whose binary is gone. A `dir` that is a symlink loses the link, never what it points at.
fn remove_leftover_bundle(tool: &Tool, dir: &Path, problems: &mut Problems) {
    if dir.symlink_metadata().is_err() {
        return; // nothing bundled, nothing to remove
    }
    match std::fs::remove_dir_all(dir) {
        Ok(()) => eprintln!("tools: removed the bundled {} ({}) — this system's own is used instead", tool.dir, dir.display()),
        Err(err) => problems.report(format!("tools: could not remove the bundled {} at {}: {err}", tool.dir, dir.display())),
    }
}

/// The freshness check both release modes share: `None` when the bundle at `dir` was already
/// built from the latest published URL (per its `.source_url` marker) or no URL could be
/// determined (reported to `problems`, with the lookup's reason) — else the URL to (re)build
/// from. `url` is the wanted asset as [`sync`]'s parallel discovery phase resolved it; the
/// comparison itself is local.
fn release_url_if_stale(
    tool: &Tool,
    url: &Result<String, LookupError>,
    dir: &Path,
    pin: Option<&str>,
    problems: &mut Problems,
) -> Option<String> {
    let installed = dir.join(tool.bins[0].1).exists();
    let url = match url {
        Ok(url) => url,
        Err(why) => {
            let kept = if installed { " — keeping the bundled one" } else { "" };
            problems.report(match pin {
                Some(pin) => format!("tools: could not resolve the recorded {} pin {pin} ({why}){kept}", tool.dir),
                None if installed => format!("tools: could not check for a newer {} build ({why}){kept}", tool.dir),
                None => format!("tools: could not determine the {} build for this machine ({why})", tool.dir),
            });
            return None;
        }
    };
    let recorded = std::fs::read_to_string(dir.join(SOURCE_MARKER)).unwrap_or_default();
    let recorded_tag = std::fs::read_to_string(dir.join(TAG_MARKER)).ok();
    let stale = archive_is_stale(recorded.trim(), recorded_tag.as_deref().map(str::trim), url, pin);
    (!installed || stale).then(|| url.clone())
}

/// Whether the bundle built from `recorded` should be replaced by `wanted` — by *build identity*,
/// not URL spelling, wherever an identity can be read ([`asset_channel`]; today that's ffmpeg).
/// An ffmpeg build is its channel plus the release it came from: the tag in its URL, or — for a
/// bundle an earlier bashrs fetched from BtbN's rolling `latest` release — `recorded_tag`, the
/// one kept beside it. Unpinned, only a newer build fetches: a newer channel, or the same one
/// from a later release. Never a downgrade, so a newer daily that an earlier bashrs fetched
/// stays until a month-end build passes it.
///
/// Rolling URLs are no longer fetched. BtbN re-creates the `latest` release every day (deleting
/// and re-uploading its assets), and a check that landed in that window got a different answer
/// from the API — the dated release's URLs instead of the rolling ones, or a partial asset list
/// missing the newest channel; compared as URL strings, each such flip re-downloaded ~120 MB of
/// the SAME build, twice. A dated release's URLs never change.
///
/// Pins override in two grades: a dated pin (`n8.1 @ autobuild-…`) exists to restore an *exact*
/// build, so it compares exactly; a channel-only pin restores that channel, newer or older.
/// A URL with no readable channel (every other tool) keeps the plain string comparison.
fn archive_is_stale(recorded: &str, recorded_tag: Option<&str>, wanted: &str, pin: Option<&str>) -> bool {
    if pin.is_some_and(|pin| pin.contains(" @ ")) {
        return recorded != wanted; // exact-build restore: the precise URL is the point
    }
    match (asset_channel(recorded), asset_channel(wanted)) {
        (Some(have), Some(want)) if pin.is_some() => have != want, // pinned channel, either way
        (Some(have), Some(want)) => {
            // Unpinned: upgrades only. A build with no readable release sorts before every
            // dated one — a newer build of its channel is surely out there.
            let have_tag = build_tag(recorded).or_else(|| recorded_tag.map(str::to_string)).unwrap_or_default();
            let want_tag = build_tag(wanted).unwrap_or_default();
            (want, want_tag) > (have, have_tag)
        }
        _ => recorded != wanted,
    }
}

/// The release an ffmpeg build came from, read off its URL — `None` for BtbN's rolling `latest`
/// release, which names no build.
fn build_tag(url: &str) -> Option<String> {
    release_tag(url).filter(|tag| tag != "latest")
}

/// The archive mode: re-fetch when the wanted URL (latest published, or the `pin`ned release in
/// `--use-stable-carstay` mode) is a newer build than the bundled one's marker
/// ([`archive_is_stale`]). Returns whether a fetch happened.
fn sync_archive(
    tool: &Tool,
    url: &Result<String, LookupError>,
    dir: &Path,
    pin: Option<&str>,
    problems: &mut Problems,
) -> bool {
    let Some(url) = release_url_if_stale(tool, url, dir, pin, problems) else { return false };
    eprintln!("tools: fetching {} into {}", tool.dir, dir.display());
    match fetch(&url, dir) {
        Ok(()) => {
            let _ = std::fs::write(dir.join(SOURCE_MARKER), &url);
            // An earlier bashrs's tag marker would now describe another build; the URL names this one.
            let _ = std::fs::remove_file(dir.join(TAG_MARKER));
            true
        }
        Err(msg) => {
            problems.report(format!("tools: could not bundle {}: {msg}", tool.dir));
            false
        }
    }
}

/// The single-binary mode: same freshness contract as [`sync_archive`], but the download *is*
/// the program — written to the tool's first `bins` path and made executable.
fn sync_binary(
    tool: &Tool,
    url: &Result<String, LookupError>,
    dir: &Path,
    pin: Option<&str>,
    problems: &mut Problems,
) -> bool {
    let Some(url) = release_url_if_stale(tool, url, dir, pin, problems) else { return false };
    eprintln!("tools: fetching {} into {}", tool.dir, dir.display());
    match download_binary(&url, &dir.join(tool.bins[0].1)) {
        Ok(()) => {
            let _ = std::fs::write(dir.join(SOURCE_MARKER), &url);
            true
        }
        Err(msg) => {
            problems.report(format!("tools: could not bundle {}: {msg}", tool.dir));
            false
        }
    }
}

/// Download `url` to `dest` (parents created) and mark it executable.
fn download_binary(url: &str, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let downloaded =
        Command::new("curl").args(["-fSL", "--retry", "2", "-o"]).arg(dest).arg(url).status();
    match downloaded {
        Ok(status) if status.success() => {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755))
                .map_err(|err| err.to_string())
        }
        Ok(_) => Err("curl could not download it".into()),
        Err(err) => Err(format!("could not run curl: {err}")),
    }
}

/// The uv-venv mode: a venv at `dir` gives the stable `bin/python3` everything else names, over a
/// managed interpreter uv installs under `~/.bashrs/tools/interpreters`. Creation downloads the
/// requested version; afterwards `uv python upgrade` keeps the interpreter's patch level current —
/// the venv, created against the minor version, follows transparently, and its site-packages
/// survive (package updates are `py_update`'s job, not this one's). A `pin` (`--use-stable-carstay`)
/// creates against the recorded exact version instead, and never upgrades an existing one.
/// Self-healing: an environment that exists but no longer *runs* ([`venv_is_dead`] — the
/// moved-disk case) is wiped and recreated here, so migrating `~/.bashrs` to another machine
/// needs no manual cleanup beyond re-running the compile.
fn sync_python_venv(python: &str, dir: &Path, pin: Option<&str>, problems: &mut Problems) -> bool {
    if dir.exists() && !dir.join("pyvenv.cfg").exists() {
        // The pre-uv layout (an unpacked python-build-standalone archive): replace it. The repo
        // dependencies are reinstalled by the stainless sync that follows; manually installed
        // packages need a `py_install` again.
        eprintln!("tools: migrating the bundled python to a uv-managed environment");
        let _ = std::fs::remove_dir_all(dir);
    }
    if venv_is_dead(dir) {
        // Presence isn't health: a venv bakes absolute paths in at creation (pyvenv.cfg's
        // `home`, the inner `bin/python3` symlink), so a `~/.bashrs` disk moved to another
        // machine — different username or mount point — can leave one that exists but no longer
        // runs. Rebuild it from scratch (against the pin, when recording): the repo python
        // dependencies are reinstalled by the stainless sync that follows this; manually
        // `py_install`ed packages need installing again.
        eprintln!(
            "tools: the bundled python environment no longer runs (a moved ~/.bashrs disk?) — rebuilding it"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
    if !dir.join("pyvenv.cfg").exists() {
        let version = pin.unwrap_or(python);
        eprintln!("tools: creating the python {version} environment at {}", dir.display());
        if uv_managed(&["venv", &dir.display().to_string(), "--python", version]) {
            return true;
        }
        problems.report("tools: could not create the python environment");
    } else if let Some(pin) = pin {
        // Recorded mode never moves the interpreter — just say so when it drifted from the
        // record (adopting the pin means recreating the environment, which drops installed
        // packages, so it stays the user's call).
        match python_version(dir) {
            Some(actual) if actual != pin => problems.report(format!(
                "tools: bundled python is {actual} but Carstay.toml records {pin} — delete {} and re-run to adopt the recorded one",
                dir.display()
            )),
            _ => {}
        }
    } else if !uv_managed(&["python", "upgrade", python]) {
        problems.report("tools: could not check for a python upgrade — keeping the current one");
    }
    false
}

/// A venv that *exists* but whose interpreter no longer answers — dead, not merely absent (an
/// absent one is simply "not created yet" and takes the creation path). The verdict comes from
/// actually running `bin/python3 --version`, not from inspecting files: dangling absolute paths
/// (the moved-disk case), a deleted interpreter under `interpreters/`, or a corrupt binary all
/// fail the same honest way.
fn venv_is_dead(dir: &Path) -> bool {
    dir.join("pyvenv.cfg").exists() && python_version(dir).is_none()
}

/// The venv interpreter's version (`Python 3.14.2` → `3.14.2`), `None` when it isn't bundled.
fn python_version(dir: &Path) -> Option<String> {
    let out = Command::new(dir.join("bin/python3")).arg("--version").output().ok()?;
    let version = String::from_utf8_lossy(&out.stdout);
    let version = version.trim().trim_start_matches("Python ").to_string();
    (out.status.success() && !version.is_empty()).then_some(version)
}

/// Each acquisition mode's provisioned-version identity, for [`crate::drivers::carstay`]'s
/// manifest (via [`super::versions`]): release tags read back from the `.source_url` marker;
/// the interpreter's own report for the venv; and for ffmpeg the channel plus the release its
/// build came from — `"n8.1 @ autobuild-…"` — so a restore can name the exact build. `None` when
/// nothing is bundled.
pub(super) fn provisioned_version(tool: &Tool, dir: &Path) -> Option<String> {
    if let Acquire::UvVenv { .. } = tool.acquire {
        return python_version(dir);
    }
    let url = std::fs::read_to_string(dir.join(SOURCE_MARKER)).ok()?.trim().to_string();
    let Some((major, minor)) = asset_channel(&url) else { return release_tag(&url) };
    // The exact-build tag: in the URL itself, or — for a bundle an earlier bashrs fetched from
    // the rolling `latest` release — the tag it kept beside the bundle.
    let tag = build_tag(&url)
        .or_else(|| Some(std::fs::read_to_string(dir.join(TAG_MARKER)).ok()?.trim().to_string()))
        .filter(|tag| !tag.is_empty());
    Some(match tag {
        Some(tag) => format!("n{major}.{minor} @ {tag}"),
        None => format!("n{major}.{minor}"),
    })
}

/// Run `uv` with the managed-interpreter story pinned under `~/.bashrs/tools/`: interpreters land
/// in [`super::interpreters_dir`], and system pythons are never linked against (a venv onto one
/// would break whenever the system python moves or updates).
fn uv_managed(args: &[&str]) -> bool {
    let run = Command::new(super::resolve_uv())
        .args(args)
        .env("UV_PYTHON_INSTALL_DIR", super::interpreters_dir())
        .env("UV_MANAGED_PYTHON", "1")
        .status();
    matches!(run, Ok(status) if status.success())
}

/// Symlink each of `tool`'s bundled binaries into `~/.bashrs/tools/bin` (refreshing stale links).
/// A binary that isn't bundled gets no shim — and any leftover one is pruned, so deleting a
/// bundle by hand doesn't strand a dangling symlink on PATH.
fn ensure_shims(tool: &Tool, problems: &mut Problems) {
    for (name, rel) in tool.bins {
        if !root().join(tool.dir).join(rel).exists() {
            let _ = std::fs::remove_file(bin_dir().join(name));
            continue;
        }
        let link = bin_dir().join(name);
        let _ = std::fs::create_dir_all(bin_dir());
        // Replaced as a LINK, deliberately not through `conf::replacement_target`: this is
        // bashrs's own shim, and resolving it would delete the tool it points at and write the
        // new shim over it. User symlinks are preserved elsewhere in `~/.bashrs`; `tools/` is
        // bashrs's managed tree, rebuilt by every sync.
        let _ = std::fs::remove_file(&link);
        // A venv's binaries must NOT be shimmed by symlink: python discovers its venv by looking
        // for pyvenv.cfg beside the path it was *invoked as* (symlinks deliberately unresolved —
        // that's what makes venvs work at all), so a symlink in tools/bin silently runs the bare
        // base interpreter without the venv's site-packages. An `exec` script hands python its
        // real in-venv path instead. Plain binaries keep the cheaper symlink.
        let result = match tool.acquire {
            Acquire::UvVenv { .. } => std::fs::write(&link, venv_shim(tool.dir, rel)).and_then(|()| {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&link, std::fs::Permissions::from_mode(0o755))
            }),
            _ => std::os::unix::fs::symlink(Path::new("..").join(tool.dir).join(rel), &link),
        };
        if let Err(err) = result {
            problems.report(format!("tools: could not shim {name}: {err}"));
        }
    }
}

/// The venv-aware shim script: `exec` the binary by its real in-venv path (`$HOME`-based, so a
/// symlinked `~/.bashrs` keeps working), preserving pyvenv.cfg discovery.
fn venv_shim(tool_dir: &str, rel: &str) -> String {
    let root = super::TOOLS_ROOT_SHELL;
    format!("#!/bin/sh\nexec \"{root}/{tool_dir}/{rel}\" \"$@\"\n")
}

/// Download `url` and unpack the archive's contents into `dir` — tarballs via `tar` (root
/// folder stripped), zips via the bundled python's `zipfile` (zip is not GNU tar territory,
/// and `unzip` isn't a given on every system; python is, by our own bundling).
fn fetch(url: &str, dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|err| err.to_string())?;
    let archive = dir.join("download.tmp");
    let downloaded = Command::new("curl")
        .args(["-fSL", "--retry", "2", "-o"])
        .arg(&archive)
        .arg(url)
        .status();
    if !matches!(downloaded, Ok(status) if status.success()) {
        return Err(format!("download failed: {url}"));
    }
    let unpacked = if url.ends_with(".zip") { unzip(&archive, dir) } else { untar(&archive, dir) };
    let _ = std::fs::remove_file(&archive);
    unpacked
}

fn untar(archive: &Path, dir: &Path) -> Result<(), String> {
    let unpacked = Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(dir)
        .arg("--strip-components=1")
        .status();
    if !matches!(unpacked, Ok(status) if status.success()) {
        return Err("could not unpack the archive".into());
    }
    Ok(())
}

/// Unpack a zip (deno releases, notably: a bare binary at the archive root). Python's zipfile
/// drops the executable bits, so every extracted top-level file gets them back.
fn unzip(archive: &Path, dir: &Path) -> Result<(), String> {
    let unpacked = Command::new(super::resolve("python3"))
        .arg("-m")
        .arg("zipfile")
        .arg("-e")
        .arg(archive)
        .arg(dir)
        .status();
    if !matches!(unpacked, Ok(status) if status.success()) {
        return Err("could not unpack the zip".into());
    }
    for entry in std::fs::read_dir(dir).map_err(|err| err.to_string())?.flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_file()) {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(entry.path(), std::fs::Permissions::from_mode(0o755));
        }
    }
    Ok(())
}

/// The latest-release JSON of a GitHub `owner/repo` — one small API fetch, from which the asset
/// URLs are picked by shape.
/// A release's JSON from the GitHub API — the latest one, or (in `--use-stable-carstay` mode) the release
/// published under the recorded tag, so the exact recorded build is what gets fetched.
fn release_json(repo: &str, tag: Option<&str>) -> Result<String, LookupError> {
    let api = match tag {
        Some(tag) => format!("https://api.github.com/repos/{repo}/releases/tags/{tag}"),
        None => format!("https://api.github.com/repos/{repo}/releases/latest"),
    };
    github_api(&api)
}

/// Why a release lookup found nothing to bundle. One case is told apart from the rest: GitHub's
/// hourly limit, which refuses every lookup alike and is explained once per sync
/// ([`rate_limit_note`]) rather than once per tool.
#[derive(Debug, PartialEq)]
pub(super) enum LookupError {
    /// GitHub's limit on requests from this IP address is spent. `limit` is its size and
    /// `resets_in` the wait until it counts afresh, each when GitHub said.
    RateLimited { limit: Option<u32>, resets_in: Option<Duration> },
    /// Anything else, as a sentence for the problem report.
    Other(String),
}

impl std::fmt::Display for LookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RateLimited { resets_in: Some(wait), .. } => {
                // Whole minutes, rounded up: "another 0 minutes" would promise a reset already due.
                let minutes = wait.as_secs().div_ceil(60).max(1);
                let wait = if minutes == 1 { "minute".to_string() } else { format!("{minutes} minutes") };
                write!(f, "GitHub's hourly lookup limit is used up, for another {wait}")
            }
            Self::RateLimited { resets_in: None, .. } => f.write_str("GitHub's hourly lookup limit is used up"),
            Self::Other(why) => f.write_str(why),
        }
    }
}

impl From<String> for LookupError {
    fn from(why: String) -> Self {
        Self::Other(why)
    }
}

impl From<&str> for LookupError {
    fn from(why: &str) -> Self {
        Self::Other(why.to_string())
    }
}

/// GitHub's primary limit on REST API requests made without signing in, per originating IP
/// address — documented in "Rate limits for the REST API" (docs.github.com). The figure to
/// explain the limit with when a refusal doesn't carry its own `x-ratelimit-limit`.
const GITHUB_HOURLY_LIMIT: u32 = 60;

/// One GitHub API answer as text — or why there is none, worded for the problem report. The
/// HTTP status and the rate-limit headers ride after the body (`-w`), since `-f` would keep them
/// to itself, and they are what tell "no such release" from "rate-limited". A connection that
/// never forms gives up after 10 s instead of curl's default of minutes.
fn github_api(url: &str) -> Result<String, LookupError> {
    let tail = "\n%{http_code} %header{x-ratelimit-limit} %header{x-ratelimit-remaining} %header{x-ratelimit-reset} %header{retry-after}";
    let out = Command::new("curl")
        .args(["-sSL", "--connect-timeout", "10", "--max-time", "60", "-w", tail, url])
        .output()
        .map_err(|err| format!("could not run curl: {err}"))?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
    api_answer(out.status.code(), &String::from_utf8_lossy(&out.stdout), &String::from_utf8_lossy(&out.stderr), now)
}

/// [`github_api`]'s verdict on one curl run: its exit code, its output (the body, then a line of
/// the HTTP status and the `x-ratelimit-limit`, `-remaining` and `-reset` and `retry-after`
/// headers, an absent one empty), its error text, and the time `now` (Unix seconds). A failed
/// curl never got an answer — no DNS, no route, a timeout — and says which itself.
///
/// GitHub refuses a spent limit with a 403 or a 429 and `x-ratelimit-remaining: 0`, its reset due
/// at `x-ratelimit-reset`; a secondary limit says how long to wait in `retry-after` instead. A
/// body naming the rate limit counts too, for a curl too old to write headers out (before 7.84).
fn api_answer(exit: Option<i32>, stdout: &str, stderr: &str, now: u64) -> Result<String, LookupError> {
    if exit != Some(0) {
        let why = stderr.trim().trim_start_matches("curl: ");
        let why = why.split_once(") ").filter(|(code, _)| code.starts_with('(')).map_or(why, |(_, why)| why);
        let why = if why.is_empty() { "curl gave no reason" } else { why };
        return Err(format!("could not reach GitHub: {why}").into());
    }
    let (body, tail) = stdout.rsplit_once('\n').unwrap_or(("", stdout));
    let mut fields = tail.splitn(5, ' ');
    let mut field = || fields.next().unwrap_or("").trim();
    let (status, limit, remaining, reset, retry_after) = (field(), field(), field(), field(), field());
    let retry_after = retry_after.parse::<u64>().ok();
    let limited = status == "429"
        || (status == "403" && (remaining == "0" || retry_after.is_some() || body.contains("rate limit")));
    if limited {
        let reset_wait = reset.parse::<u64>().ok().filter(|_| remaining == "0").map(|at| at.saturating_sub(now));
        let resets_in = retry_after.or(reset_wait).map(Duration::from_secs);
        return Err(LookupError::RateLimited { limit: limit.parse().ok(), resets_in });
    }
    match status {
        "200" => Ok(body.to_string()),
        "404" => Err("GitHub answered 404, no such release".into()),
        status => Err(format!("GitHub answered {status}").into()),
    }
}

/// The reason a release lookup found no asset to bundle: which release of which project lacked
/// which asset.
fn missing_from(project: &str, pin: Option<&str>, asset: &str) -> LookupError {
    match pin {
        Some(tag) => format!("{project}'s release {tag} has no {asset}").into(),
        None => format!("{project}'s latest release has no {asset}").into(),
    }
}

/// BtbN's official ffmpeg static builds (ffmpeg + ffprobe in one archive): the newest release
/// channel's asset for this architecture, from BtbN's newest month-end release
/// ([`newest_month_end`]). Its URL names the exact build, and BtbN keeps month-end releases for
/// years where its dailies go after about two weeks — so the build Carstay.toml records stays
/// fetchable. A pin (a Carstay value) narrows that: `"n8.1"` picks the channel from the same
/// month-end release, and the full `"n8.1 @ autobuild-…"` form fetches the EXACT recorded build
/// from its own release.
pub(super) fn ffmpeg_url(pin: Option<&str>) -> Result<String, LookupError> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "linux64",
        "aarch64" => "linuxarm64",
        other => return Err(format!("BtbN publishes no ffmpeg build for {other}").into()),
    };
    let (channel, tag) = match pin {
        None => (None, None),
        Some(pin) => match pin.split_once(" @ ") {
            Some((channel, tag)) => (Some(channel.trim()), Some(tag.trim())),
            None => (Some(pin), None), // channel-only (older recordings, hand edits)
        },
    };
    let tag = match tag {
        Some(tag) => tag.to_string(),
        None => {
            let tags = github_api("https://api.github.com/repos/BtbN/FFmpeg-Builds/tags?per_page=100")?;
            newest_month_end(tags.split('"')).ok_or("BtbN lists no month-end build")?.to_string()
        }
    };
    let json = release_json("BtbN/FFmpeg-Builds", Some(&tag))?;
    find_ffmpeg_asset(&json, arch, channel).ok_or_else(|| {
        LookupError::from(match channel {
            Some(channel) => format!("BtbN's release {tag} has no {arch} build of {channel}"),
            None => format!("BtbN's release {tag} has no {arch} build"),
        })
    })
}

/// The GitHub API requests looking `tool` up at `pin` takes: one for its release — plus, for
/// ffmpeg without an exact-build pin, the tag listing [`ffmpeg_url`] finds that release in first.
/// A venv makes none: uv fetches interpreters from release downloads, which the API limit doesn't
/// count.
fn api_requests(tool: &Tool, pin: Option<&str>) -> usize {
    match tool.acquire {
        Acquire::UvVenv { .. } => 0,
        Acquire::Archive(_) | Acquire::Binary(_) => {
            let listing = tool.dir == "ffmpeg" && !pin.is_some_and(|pin| pin.contains(" @ "));
            1 + usize::from(listing)
        }
    }
}

/// The tag of BtbN's newest month-end build: the newest `autobuild-YYYY-MM-DD-HH-MM` dated in a
/// month before the newest build's own — which makes it the last of its month, the one BtbN
/// keeps. Read off the builds' own dates, not today's, so neither the clock nor a time zone can
/// move it. Anything else among `tags` (`latest`, other tokens of the API's JSON) is ignored.
fn newest_month_end<'a>(tags: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let builds: Vec<(&str, &str)> = tags.into_iter().filter_map(|tag| Some((build_month(tag)?, tag))).collect();
    let newest_month = builds.iter().map(|(month, _)| *month).max()?;
    // Same-width dates, so the greatest tag is the latest build.
    builds.into_iter().filter(|(month, _)| *month < newest_month).map(|(_, tag)| tag).max()
}

/// `autobuild-2026-09-30-13-08` → `2026-09`; `None` for any tag not of that shape.
fn build_month(tag: &str) -> Option<&str> {
    let date = tag.strip_prefix("autobuild-")?;
    let shaped = date.len() == 16
        && date
            .char_indices()
            .all(|(at, ch)| if matches!(at, 4 | 7 | 10 | 13) { ch == '-' } else { ch.is_ascii_digit() });
    shaped.then(|| &date[..7])
}

/// The release-channel static GPL build for `arch`, from either of BtbN's asset namings —
/// `ffmpeg-n8.1-latest-linux64-gpl-8.1.tar.xz` (the rolling release) or
/// `ffmpeg-n8.1.2-22-g<commit>-linux64-gpl-8.1.tar.xz` (a dated release). The master-branch
/// build (`ffmpeg-N-…`, no trailing version after `gpl`) and the shared/lgpl variants are
/// skipped. `channel` narrows to that `n<major.minor>` — an unparseable or absent match yields
/// `None` outright, never "the newest instead" (that would defeat pinning); without a channel,
/// the highest one wins, since channels rotate out as new majors land.
fn find_ffmpeg_asset(json: &str, arch: &str, channel: Option<&str>) -> Option<String> {
    let want = match channel {
        Some(channel) => Some(parse_channel_id(channel)?),
        None => None,
    };
    let marker = format!("-{arch}-gpl-");
    json.split('"')
        .filter(|token| {
            token.starts_with("https://")
                && token.contains(&marker)
                && !token.contains("-shared-")
                && token.ends_with(".tar.xz")
        })
        .filter_map(|token| Some((asset_channel(token)?, token)))
        .filter(|(channel, _)| want.is_none_or(|want| *channel == want))
        .max_by_key(|(channel, _)| *channel)
        .map(|(_, token)| token.to_owned())
}

/// `n<major>.<minor>` (the channel form Carstay records for ffmpeg) → `(major, minor)`.
fn parse_channel_id(channel: &str) -> Option<(u32, u32)> {
    let (major, minor) = channel.strip_prefix('n')?.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// The `(major, minor)` channel of an ffmpeg asset URL, tolerant of both namings: the segment
/// after `/ffmpeg-n` reads `8.1-latest-…` (rolling) or `8.1.2-22-g…` (dated) — the leading
/// digits of its first two `.`-components are the channel either way.
fn asset_channel(url: &str) -> Option<(u32, u32)> {
    let version = url.rsplit("/ffmpeg-n").next().filter(|rest| *rest != url)?;
    let mut components = version.split('.');
    let digits = |component: &str| -> Option<u32> {
        let digits: String = component.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    };
    let major = digits(components.next()?)?;
    let minor = digits(components.next()?)?;
    Some((major, minor))
}

/// astral-sh's uv — a single static binary that manages the bundled python's packages
/// (`uv pip … --python <bundled>`; see [`super::python`]).
pub(super) fn uv_url(pin: Option<&str>) -> Result<String, LookupError> {
    let arch = match std::env::consts::ARCH {
        arch @ ("x86_64" | "aarch64") => arch,
        other => return Err(format!("uv publishes no build for {other}").into()),
    };
    find_uv_asset(&release_json("astral-sh/uv", pin)?, arch)
        .ok_or_else(|| missing_from("uv", pin, &format!("{arch} Linux build")))
}

/// The latest release's `uv-<arch>-unknown-linux-gnu.tar.gz` download URL (skipping the musl
/// variants and checksum files by the exact suffix).
fn find_uv_asset(json: &str, arch: &str) -> Option<String> {
    let suffix = format!("/uv-{arch}-unknown-linux-gnu.tar.gz");
    json.split('"')
        .find(|token| token.starts_with("https://") && token.ends_with(&suffix))
        .map(str::to_owned)
}

/// yt-dlp's zipapp, discovered from the latest release. Chosen over the standalone
/// `yt-dlp_linux` deliberately: it's arch-independent, and it skips the PyInstaller
/// self-extraction that cost 1.6s on every invocation (the zipapp starts ~4× faster on the
/// bundled python). When the bundle is absent, [`super::resolve`] falls back to whatever
/// `yt-dlp` the system provides.
pub(super) fn ytdlp_url(pin: Option<&str>) -> Result<String, LookupError> {
    find_ytdlp_asset(&release_json("yt-dlp/yt-dlp", pin)?, "yt-dlp").ok_or_else(|| missing_from("yt-dlp", pin, "zipapp"))
}

/// deno's static binary zip for this machine, discovered from the latest release — bundled to
/// serve yt-dlp, whose YouTube extractor needs a JS runtime (deno is the one it enables by
/// default) or some formats go missing.
pub(super) fn deno_url(pin: Option<&str>) -> Result<String, LookupError> {
    let asset = match std::env::consts::ARCH {
        "x86_64" => "deno-x86_64-unknown-linux-gnu.zip",
        "aarch64" => "deno-aarch64-unknown-linux-gnu.zip",
        other => return Err(format!("deno publishes no build for {other}").into()),
    };
    find_ytdlp_asset(&release_json("denoland/deno", pin)?, asset).ok_or_else(|| missing_from("deno", pin, asset))
}

/// Register Git's global LFS filters after git-lfs is freshly bundled — the `git lfs install`
/// step the binary alone doesn't perform, without which LFS repos check out as pointer files
/// rather than real content (see the answer to "are the binaries any use after install": yes —
/// these filters are what *call* the binary on every LFS checkout/commit).
///
/// `--skip-repo` keeps it to the user's *global* config (`~/.gitconfig`) and touches no
/// repository — in particular not the bashrs checkout this happens to run inside. It shells out
/// to Git (the filter lines are written via `git config`), so a machine with no `git` on PATH
/// gets a clear note instead of a silent miss. Never fatal: the binary is installed regardless,
/// and the message — returned, for the problem report — names the one command to run by hand.
/// Reached only on a fresh fetch (see [`sync`]), so it does not re-run on later compiles.
pub(super) fn activate_git_lfs(dir: &Path) -> Result<(), String> {
    let binary = dir.join("git-lfs");
    match std::process::Command::new(&binary).args(["install", "--skip-repo"]).output() {
        Ok(out) if out.status.success() => {
            eprintln!("tools: git-lfs activated (git lfs install) — LFS repositories will now fetch file content");
            Ok(())
        }
        Ok(out) => {
            let why = String::from_utf8_lossy(&out.stderr);
            Err(format!(
                "tools: git-lfs is bundled, but `git lfs install` failed ({}) — is git on PATH? run it yourself to activate LFS filters",
                why.trim()
            ))
        }
        Err(err) => Err(format!(
            "tools: git-lfs is bundled, but `git lfs install` could not run ({err}) — run it yourself to activate LFS filters"
        )),
    }
}

/// git-lfs's Linux tarball for this machine, discovered from the latest release — an optional
/// extra (`[tools] install_extras`), Git's large-file support. Nothing bashrs runs touches it;
/// it's here only to save a manual install.
pub(super) fn gitlfs_url(pin: Option<&str>) -> Result<String, LookupError> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => return Err(format!("git-lfs publishes no build for {other}").into()),
    };
    find_gitlfs_asset(&release_json("git-lfs/git-lfs", pin)?, arch)
        .ok_or_else(|| missing_from("git-lfs", pin, &format!("linux-{arch} tarball")))
}

/// git-lfs's `git-lfs-linux-<arch>-v<ver>.tar.gz` — matched by the `linux-<arch>-v` infix so the
/// other OSes and the wrong architecture don't qualify, and by the `.tar.gz` suffix so the
/// checksum file (`.tar.gz` + a `sha256sums` sibling) can't be mistaken for the archive.
fn find_gitlfs_asset(json: &str, arch: &str) -> Option<String> {
    let infix = format!("/git-lfs-linux-{arch}-v");
    json.split('"')
        .find(|token| {
            token.starts_with("https://") && token.contains(&infix) && token.ends_with(".tar.gz")
        })
        .map(str::to_owned)
}

/// The release asset named exactly `asset` — a `/`-anchored suffix match on the download URL,
/// which sibling assets (`yt-dlp` / `yt-dlp_linux` / `yt-dlp_linux_aarch64`) can't satisfy for
/// one another.
fn find_ytdlp_asset(json: &str, asset: &str) -> Option<String> {
    let suffix = format!("/{asset}");
    json.split('"')
        .find(|token| token.starts_with("https://") && token.ends_with(&suffix))
        .map(str::to_owned)
}

/// The GitHub release tag inside a download URL (`…/releases/download/<tag>/<asset>`) — the
/// machine-independent version identity a [`SOURCE_MARKER`] records for tag-per-release tools.
/// (An ffmpeg bundle an earlier bashrs fetched from the rolling `latest` release names nothing
/// here; [`build_tag`] reads that as no build, and [`provisioned_version`] falls back to the
/// [`TAG_MARKER`].)
fn release_tag(url: &str) -> Option<String> {
    let mut segments = url.split('/').skip_while(|segment| *segment != "download");
    segments.nth(1).map(str::to_string)
}


#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture venv dir: `pyvenv.cfg` present, and `bin/python3` either a working stub (a
    /// script answering `--version` like a real interpreter) or a dangling symlink (the exact
    /// artifact a `~/.bashrs` disk moved to another machine leaves behind).
    fn venv_fixture(tag: &str, python: Option<&str>) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("bashrs_venv_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("pyvenv.cfg"), "home = /somewhere/interpreters/bin\n").unwrap();
        match python {
            Some(script) => {
                use std::os::unix::fs::PermissionsExt;
                let bin = dir.join("bin/python3");
                std::fs::write(&bin, script).unwrap();
                std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            // The moved-disk casualty: a symlink whose absolute target no longer exists.
            None => std::os::unix::fs::symlink("/no/such/interpreter/python3", dir.join("bin/python3")).unwrap(),
        }
        dir
    }

    #[test]
    fn a_venv_is_dead_only_when_present_but_unrunnable() {
        // Healthy: the interpreter answers → not dead.
        let ok = venv_fixture("ok", Some("#!/bin/sh\necho 'Python 9.9.9'\n"));
        assert!(!venv_is_dead(&ok), "an answering interpreter is healthy");
        assert_eq!(python_version(&ok).as_deref(), Some("9.9.9"));
        // Dead: pyvenv.cfg exists, but bin/python3 dangles (stale absolute paths) → rebuild.
        let dead = venv_fixture("dead", None);
        assert!(venv_is_dead(&dead), "a dangling interpreter must read as dead");
        // Absent: no pyvenv.cfg at all is NOT dead — it's "not created yet" (the creation path).
        let missing = std::env::temp_dir().join(format!("bashrs_venv_none_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&missing);
        assert!(!venv_is_dead(&missing), "absence is not death");
        for dir in [ok, dead] {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn version_identity_is_the_release_tag_recorded_verbatim() {
        assert_eq!(
            release_tag("https://github.com/astral-sh/uv/releases/download/0.11.29/uv-x86_64-unknown-linux-gnu.tar.gz").as_deref(),
            Some("0.11.29")
        );
        assert_eq!(
            release_tag("https://github.com/denoland/deno/releases/download/v2.9.3/deno-x86_64-unknown-linux-gnu.zip").as_deref(),
            Some("v2.9.3"),
            "tags are recorded verbatim (the URL is reconstructable from them)"
        );
        assert_eq!(release_tag("https://example.com/no/release/here"), None);
    }

    #[test]
    fn the_ffmpeg_channel_reads_from_both_rolling_and_dated_asset_namings() {
        assert_eq!(
            asset_channel("https://x/ffmpeg-n8.1-latest-linux64-gpl-8.1.tar.xz"),
            Some((8, 1)),
            "the rolling release's naming"
        );
        assert_eq!(
            asset_channel("https://x/ffmpeg-n8.1.2-22-g94138f6973-linux64-gpl-8.1.tar.xz"),
            Some((8, 1)),
            "a dated release's naming (exact version, channel = first two components)"
        );
        assert_eq!(
            asset_channel("https://x/ffmpeg-N-125674-g9bc73ba344-linux64-gpl.tar.xz"),
            None,
            "the master-branch build (capital N, no channel) is not a channel"
        );
        assert_eq!(asset_channel("https://x/other.tar.xz"), None);
    }

    #[test]
    fn ytdlp_asset_matches_its_exact_name_never_a_sibling_prefix() {
        let json = r#"{"assets":[
            {"browser_download_url":"https://github.com/yt-dlp/yt-dlp/releases/download/2026.07.01/yt-dlp"},
            {"browser_download_url":"https://github.com/yt-dlp/yt-dlp/releases/download/2026.07.01/yt-dlp_linux_aarch64"},
            {"browser_download_url":"https://github.com/yt-dlp/yt-dlp/releases/download/2026.07.01/yt-dlp_linux"}]}"#;
        assert_eq!(
            find_ytdlp_asset(json, "yt-dlp_linux").unwrap(),
            "https://github.com/yt-dlp/yt-dlp/releases/download/2026.07.01/yt-dlp_linux",
            "must not stop at the bare `yt-dlp` or grab the aarch64 sibling"
        );
        assert_eq!(
            find_ytdlp_asset(json, "yt-dlp").unwrap(),
            "https://github.com/yt-dlp/yt-dlp/releases/download/2026.07.01/yt-dlp",
            "the bare zipapp asset must not match its `yt-dlp_*` siblings"
        );
        assert!(find_ytdlp_asset(json, "yt-dlp_windows").is_none());
    }

    /// An ffmpeg build is its channel plus the release it came from, compared as such: a later
    /// month-end build fetches; the same build in its other spelling — an earlier bashrs's
    /// rolling URL with the tag it kept beside it — does not; and unpinned, nothing ever goes
    /// backwards.
    #[test]
    fn ffmpeg_freshness_is_by_build_and_only_ever_upgrades() {
        // (asset spellings as BtbN's rolling and dated releases name them)
        const ROLLING_81: &str = "https://x/download/latest/ffmpeg-n8.1-latest-linux64-gpl-8.1.tar.xz";
        const ROLLING_90: &str = "https://x/download/latest/ffmpeg-n9.0-latest-linux64-gpl-9.0.tar.xz";
        const AUG_DAILY_90: &str = "https://x/download/autobuild-2026-08-24-13-10/ffmpeg-n9.0.1-6-g9d4ca21220-linux64-gpl-9.0.tar.xz";
        const AUG_END_90: &str = "https://x/download/autobuild-2026-08-31-13-27/ffmpeg-n9.0.1-11-ge47273f4d9-linux64-gpl-9.0.tar.xz";
        const SEP_END_90: &str = "https://x/download/autobuild-2026-09-30-13-08/ffmpeg-n9.0.1-20-g1a2b3c4d5e-linux64-gpl-9.0.tar.xz";
        const SEP_END_91: &str = "https://x/download/autobuild-2026-09-30-13-08/ffmpeg-n9.1-3-g0f1e2d3c4b-linux64-gpl-9.1.tar.xz";
        const SEP_END_81: &str = "https://x/download/autobuild-2026-09-30-13-08/ffmpeg-n8.1.2-30-g94138f6973-linux64-gpl-8.1.tar.xz";

        // A later build of the channel is an upgrade — after a month-end, or after the daily an
        // earlier bashrs left (its rolling URL naming no build, the kept tag saying which).
        assert!(archive_is_stale(AUG_END_90, None, SEP_END_90, None), "the next month-end");
        assert!(archive_is_stale(ROLLING_90, Some("autobuild-2026-08-24-13-10"), SEP_END_90, None), "past the kept daily");
        assert!(archive_is_stale(ROLLING_90, None, SEP_END_90, None), "a rolling bundle with no tag kept at all");
        assert!(archive_is_stale(ROLLING_81, None, ROLLING_90, None), "n8.1 → n9.0");
        assert!(archive_is_stale(ROLLING_90, Some("autobuild-2026-10-05-13-01"), SEP_END_91, None), "a newer channel, whatever the dates");
        // The same build is current, in either spelling.
        assert!(!archive_is_stale(SEP_END_90, None, SEP_END_90, None));
        assert!(!archive_is_stale(ROLLING_90, Some("autobuild-2026-09-30-13-08"), SEP_END_90, None), "rolling URL + kept tag");
        // Never backwards: not to an older build — a newer daily stays until a month-end passes
        // it — nor to an older channel, as when the newest asset is missing mid-upload.
        assert!(!archive_is_stale(SEP_END_90, None, AUG_END_90, None), "an older month-end");
        assert!(!archive_is_stale(ROLLING_90, Some("autobuild-2026-10-05-13-01"), SEP_END_90, None), "a newer daily stays");
        assert!(!archive_is_stale(SEP_END_90, None, SEP_END_81, None), "no channel downgrade");

        // Pins: a dated pin restores an exact build (URL-precise); a channel pin restores its
        // channel in either direction — a downgrade is the user's explicit ask there.
        let exact = Some("n9.0 @ autobuild-2026-08-24-13-10");
        assert!(archive_is_stale(SEP_END_90, None, AUG_DAILY_90, exact), "an exact-build pin compares exactly");
        assert!(!archive_is_stale(AUG_DAILY_90, None, AUG_DAILY_90, exact));
        assert!(archive_is_stale(SEP_END_90, None, SEP_END_81, Some("n8.1")), "a pinned downgrade fetches");
        assert!(!archive_is_stale(AUG_END_90, None, SEP_END_90, Some("n9.0")), "any build of a pinned channel holds");

        // No readable channel (every non-ffmpeg archive): the plain URL comparison stands.
        assert!(archive_is_stale("https://x/uv-1.tar.gz", None, "https://x/uv-2.tar.gz", None));
        assert!(!archive_is_stale("https://x/uv-2.tar.gz", None, "https://x/uv-2.tar.gz", None));
    }

    /// The month-end build is the last one dated before the newest build's month — read off the
    /// builds' own dates, so a month counts as ended only once a build of the next one exists.
    #[test]
    fn the_newest_month_end_build_is_the_last_before_the_newest_builds_month() {
        let listed = ["latest", "autobuild-2026-10-21-13-06", "autobuild-2026-10-08-13-05", "autobuild-2026-09-30-13-08", "autobuild-2026-08-31-13-27"];
        assert_eq!(newest_month_end(listed), Some("autobuild-2026-09-30-13-08"));
        // In any order, among the API's other tokens (a tag inside a URL is no tag).
        let shuffled = ["autobuild-2026-08-31-13-27", "https://x/zipball/refs/tags/autobuild-2026-11-01-13-00", "name", "autobuild-2026-10-01-12-59", "autobuild-2026-09-30-13-08"];
        assert_eq!(newest_month_end(shuffled), Some("autobuild-2026-09-30-13-08"));
        // Across a new year: December's last, once January's first is out.
        assert_eq!(newest_month_end(["autobuild-2027-01-01-13-00", "autobuild-2026-12-15-13-00", "autobuild-2026-12-31-13-02"]), Some("autobuild-2026-12-31-13-02"));
        // One month alone: none of it is known to be its month's last yet.
        assert_eq!(newest_month_end(["autobuild-2026-10-21-13-06", "autobuild-2026-10-20-13-08"]), None);
        // Only the exact dated shape is a build.
        assert_eq!(newest_month_end(["latest", "autobuild-2026-9-30-13-08", "autobuild-2026-09-30-13-0x", "autobuild-2026-09-30"]), None);
    }

    /// The migration an existing machine goes through: an earlier bashrs's bundle — fetched from
    /// the rolling release, its build named only by the kept tag — is replaced by the month-end
    /// build, whose URL then names it alone (the kept tag goes, as it would now describe another
    /// build). Offline: the "release" is a local archive under a path shaped like BtbN's.
    #[test]
    fn a_rolling_bundle_gives_way_to_the_month_end_build_and_forgets_its_kept_tag() {
        let ffmpeg = TOOLS.iter().find(|tool| tool.dir == "ffmpeg").expect("ffmpeg in the table");
        let base = std::env::temp_dir().join(format!("bashrs_month_end_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let release = base.join("download/autobuild-2026-09-30-13-08");
        std::fs::create_dir_all(base.join("src/ffmpeg-n9.0.2/bin")).unwrap();
        std::fs::create_dir_all(&release).unwrap();
        std::fs::write(base.join("src/ffmpeg-n9.0.2/bin/ffmpeg"), "september").unwrap();
        let archive = release.join("ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0.tar.xz"); // gzip inside; tar reads either
        assert!(Command::new("tar")
            .arg("-czf").arg(&archive)
            .arg("-C").arg(base.join("src"))
            .arg("ffmpeg-n9.0.2")
            .status().unwrap().success());

        let dir = base.join("bundle");
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/ffmpeg"), "august").unwrap();
        std::fs::write(dir.join(SOURCE_MARKER), "https://x/download/latest/ffmpeg-n9.0-latest-linux64-gpl-9.0.tar.xz").unwrap();
        std::fs::write(dir.join(TAG_MARKER), "autobuild-2026-08-24-13-10\n").unwrap();
        assert_eq!(provisioned_version(ffmpeg, &dir).as_deref(), Some("n9.0 @ autobuild-2026-08-24-13-10"), "before");

        let url = format!("file://{}", archive.display());
        let mut problems = Problems::default();
        assert!(sync_archive(ffmpeg, &Ok(url.clone()), &dir, None, &mut problems), "a later build fetches");
        assert_eq!(std::fs::read_to_string(dir.join("bin/ffmpeg")).unwrap(), "september");
        assert_eq!(std::fs::read_to_string(dir.join(SOURCE_MARKER)).unwrap(), url);
        assert!(!dir.join(TAG_MARKER).exists(), "the kept tag described the build that is gone");
        assert_eq!(provisioned_version(ffmpeg, &dir).as_deref(), Some("n9.0 @ autobuild-2026-09-30-13-08"), "after");
        assert!(!sync_archive(ffmpeg, &Ok(url), &dir, None, &mut problems), "and once current, it stays put");
        assert!(problems.lines().is_empty(), "{:?}", problems.lines());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A download that fails is reported, in both release modes, and leaves no marker behind —
    /// so the next compile still sees nothing current there and tries again.
    #[test]
    fn a_failed_download_is_reported_and_marks_nothing_current() {
        let base = std::env::temp_dir().join(format!("bashrs_failed_download_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let gone = Ok("file:///no/such/release/asset.tar.xz".to_string());
        let mut problems = Problems::default();
        for dir in ["ffmpeg", "yt-dlp"] {
            let tool = TOOLS.iter().find(|tool| tool.dir == dir).expect("in the table");
            let fetched = match &tool.acquire {
                Acquire::Archive(_) => sync_archive(tool, &gone, &base.join(dir), None, &mut problems),
                Acquire::Binary(_) => sync_binary(tool, &gone, &base.join(dir), None, &mut problems),
                Acquire::UvVenv { .. } => unreachable!("{dir} is a release tool"),
            };
            assert!(!fetched, "{dir}");
            assert!(!base.join(dir).join(SOURCE_MARKER).exists(), "{dir}: nothing marked current");
        }
        assert_eq!(problems.lines().len(), 2, "{:?}", problems.lines());
        assert!(problems.lines()[0].starts_with("tools: could not bundle ffmpeg: "), "{:?}", problems.lines());
        assert!(problems.lines()[1].starts_with("tools: could not bundle yt-dlp: "), "{:?}", problems.lines());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A bundle's shim — on PATH in every bashrs shell — is no copy of the system's: the system's
    /// is the first hit outside the tools tree, symlinks resolved on both sides, so a `~/.bashrs`
    /// that is a link onto another disk is still the tree. Only shims means the system has none.
    #[test]
    fn the_systems_copy_is_the_first_one_outside_the_tools_tree() {
        let base = std::env::temp_dir().join(format!("bashrs_system_copy_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let disk = base.join("disk/tools");
        std::fs::create_dir_all(disk.join("ffmpeg/bin")).unwrap();
        std::fs::create_dir_all(disk.join("bin")).unwrap();
        std::fs::write(disk.join("ffmpeg/bin/ffmpeg"), "").unwrap();
        std::os::unix::fs::symlink("../ffmpeg/bin/ffmpeg", disk.join("bin/ffmpeg")).unwrap();
        std::fs::create_dir_all(base.join("home/.bashrs")).unwrap();
        std::os::unix::fs::symlink(&disk, base.join("home/.bashrs/tools")).unwrap();
        let tree = base.join("home/.bashrs/tools");
        let os = base.join("usr/bin/ffmpeg");
        std::fs::create_dir_all(base.join("usr/bin")).unwrap();
        std::fs::write(&os, "").unwrap();

        let shim = tree.join("bin/ffmpeg");
        assert_eq!(outside(vec![shim.clone(), os.clone()], &tree), Some(os), "past the shim, to the system's");
        assert_eq!(outside(vec![shim], &tree), None, "a shim alone: the system has none");
        assert_eq!(outside(vec![disk.join("bin/ffmpeg")], &tree), None, "the tree by its other spelling");
        assert_eq!(outside(Vec::new(), &tree), None);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A leftover bundle goes whole; a bundle directory that is a symlink loses only the link,
    /// never what it points at; and where nothing was bundled there is nothing to do or report.
    #[test]
    fn a_leftover_bundle_is_removed_but_never_through_a_symlink() {
        let ffmpeg = TOOLS.iter().find(|tool| tool.dir == "ffmpeg").expect("ffmpeg in the table");
        let base = std::env::temp_dir().join(format!("bashrs_leftover_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let bundle = base.join("tools/ffmpeg");
        std::fs::create_dir_all(bundle.join("bin")).unwrap();
        std::fs::write(bundle.join("bin/ffmpeg"), "").unwrap();
        let mut problems = Problems::default();

        remove_leftover_bundle(ffmpeg, &bundle, &mut problems);
        assert!(bundle.symlink_metadata().is_err(), "the bundle is gone");
        let elsewhere = base.join("elsewhere");
        std::fs::create_dir_all(elsewhere.join("bin")).unwrap();
        std::fs::write(elsewhere.join("bin/ffmpeg"), "keep").unwrap();
        std::os::unix::fs::symlink(&elsewhere, &bundle).unwrap();
        remove_leftover_bundle(ffmpeg, &bundle, &mut problems);
        assert!(bundle.symlink_metadata().is_err(), "the link is gone");
        assert_eq!(std::fs::read_to_string(elsewhere.join("bin/ffmpeg")).unwrap(), "keep", "what it pointed at is not");
        remove_leftover_bundle(ffmpeg, &base.join("tools/never-bundled"), &mut problems);
        assert!(problems.lines().is_empty(), "{:?}", problems.lines());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The month-end lookup against the real BtbN: it must land on a dated release — never the
    /// rolling one — and on an asset whose channel reads. Opt-in, as it queries the GitHub API:
    /// `cargo test --lib -- --ignored live_month_end`.
    #[test]
    #[ignore = "network: queries the GitHub API"]
    fn live_month_end_lookup_names_a_dated_build_of_a_channel() {
        let url = ffmpeg_url(None).expect("BtbN answers");
        let tag = build_tag(&url).unwrap_or_else(|| panic!("a dated release: {url}"));
        assert!(build_month(&tag).is_some(), "{url}");
        assert!(asset_channel(&url).is_some(), "{url}");
        eprintln!("month-end build: {url}");
    }

    /// A GitHub answer is its body, or the reason there is none — told apart as the problem
    /// report needs: no connection at all, a refusal, no such release.
    #[test]
    fn a_github_answer_is_its_body_or_a_reason_worth_reporting() {
        let answer = |exit, stdout, stderr| api_answer(exit, stdout, stderr, 1_792_636_000);
        assert_eq!(answer(Some(0), "{\"tag_name\": \"v1\"}\n200 60 59 1792639338 ", ""), Ok("{\"tag_name\": \"v1\"}".to_string()));
        assert_eq!(
            answer(Some(6), "", "curl: (6) Could not resolve host: api.github.com\n"),
            Err("could not reach GitHub: Could not resolve host: api.github.com".into()),
            "offline: curl's own words, without its code"
        );
        assert_eq!(
            answer(Some(28), "", "curl: (28) Connection timed out after 10002 milliseconds\n"),
            Err("could not reach GitHub: Connection timed out after 10002 milliseconds".into())
        );
        assert_eq!(answer(None, "", ""), Err("could not reach GitHub: curl gave no reason".into()), "a curl killed by a signal");
        assert_eq!(answer(Some(0), "{\"message\": \"Not Found\"}\n404 60 58 1792639338 ", ""), Err("GitHub answered 404, no such release".into()));
        assert_eq!(answer(Some(0), "{\"message\": \"Forbidden\"}\n403 60 41 1792639338 ", ""), Err("GitHub answered 403".into()), "a 403 with the limit unspent is no rate limit");
        assert_eq!(answer(Some(0), "bad gateway\n502    ", ""), Err("GitHub answered 502".into()));
    }

    /// GitHub's refusal of a spent limit as its documentation describes it — a 403 or a 429,
    /// `x-ratelimit-remaining: 0`, the reset at `x-ratelimit-reset` (Unix seconds) — or a
    /// secondary limit's `retry-after`, read as the one refusal worth its own explanation, with
    /// the wait until it lifts. A body naming the rate limit stands in for headers a curl older
    /// than 7.84 cannot write out.
    #[test]
    fn a_spent_rate_limit_is_told_apart_with_the_wait_until_it_lifts() {
        let now = 1_792_636_000;
        let limited = |limit, wait: Option<u64>| Err(LookupError::RateLimited { limit, resets_in: wait.map(Duration::from_secs) });
        let primary = format!("{{\"message\": \"API rate limit exceeded for 203.0.113.9.\"}}\n403 60 0 {} ", now + 1380);
        assert_eq!(api_answer(Some(0), &primary, "", now), limited(Some(60), Some(1380)), "the primary limit, 23 minutes to go");
        let too_many = format!("{{}}\n429 60 0 {} ", now + 90);
        assert_eq!(api_answer(Some(0), &too_many, "", now), limited(Some(60), Some(90)), "a 429 is the same refusal");
        assert_eq!(api_answer(Some(0), "{}\n403 60 12 1792639338 30", "", now), limited(Some(60), Some(30)), "a secondary limit's retry-after");
        let secondary = "{\"message\": \"You have exceeded a secondary rate limit.\"}\n403 60 12 1792639338 ";
        assert_eq!(api_answer(Some(0), secondary, "", now), limited(Some(60), None), "requests left: the reset time is not the wait");
        assert_eq!(api_answer(Some(0), "{\"message\": \"API rate limit exceeded\"}\n403", "", now), limited(None, None), "no headers to read");
        let past = format!("{{}}\n403 60 0 {} ", now - 5);
        assert_eq!(api_answer(Some(0), &past, "", now), limited(Some(60), Some(0)), "a reset already due waits for nothing");
    }

    /// A refused lookup's own line says the limit is spent and, when GitHub said, for how long —
    /// in whole minutes rounded up, so a reset seconds away is still "another minute".
    #[test]
    fn a_rate_limited_lookup_says_how_long_until_the_limit_lifts() {
        let line = |secs: Option<u64>| {
            LookupError::RateLimited { limit: Some(60), resets_in: secs.map(Duration::from_secs) }.to_string()
        };
        assert_eq!(line(Some(1380)), "GitHub's hourly lookup limit is used up, for another 23 minutes");
        assert_eq!(line(Some(61)), "GitHub's hourly lookup limit is used up, for another 2 minutes");
        assert_eq!(line(Some(5)), "GitHub's hourly lookup limit is used up, for another minute");
        assert_eq!(line(Some(0)), "GitHub's hourly lookup limit is used up, for another minute");
        assert_eq!(line(None), "GitHub's hourly lookup limit is used up");
    }

    /// A sync that met GitHub's spent limit says once what it means: the limit's size (as the
    /// refusal gave it, else the documented one), what this compile's lookups need, and that a
    /// VPN's address has a limit of its own. A sync the limit never refused says nothing.
    #[test]
    fn a_rate_limited_sync_explains_the_limit_once_with_what_a_compile_needs() {
        let everything: Vec<Option<Option<&str>>> = TOOLS.iter().map(|_| Some(None)).collect();
        let needed = lookups_needed(&everything);
        let refused = |limit| Some(Err(LookupError::RateLimited { limit, resets_in: None }));

        let note = rate_limit_note(&[refused(Some(5000)), Some(Ok("https://x".into())), None], &everything)
            .expect("a refusal was met");
        assert!(note.contains("GitHub answers 5000 lookups an hour from one IP address"), "the size it was told: {note}");
        assert!(note.contains(&format!("this compile's tool lookups need {needed}")), "{note}");
        assert!(note.contains("a VPN gives this machine another address"), "{note}");
        let unsized_note = rate_limit_note(&[refused(None)], &everything).expect("a refusal was met");
        assert!(unsized_note.contains(&format!("answers {GITHUB_HOURLY_LIMIT} lookups")), "the documented size: {unsized_note}");
        assert_eq!(rate_limit_note(&[Some(Err("GitHub answered 404, no such release".into())), None], &everything), None);
    }

    /// What a compile's lookups cost: one GitHub API request per bundled release tool, and ffmpeg
    /// one more — the tag listing its month-end release is found in — unless an exact-build pin
    /// names that release outright. A tool not bundled here costs nothing.
    #[test]
    fn a_compile_needs_one_lookup_per_bundled_tool_and_one_more_for_ffmpeg() {
        let release_tools = TOOLS.iter().filter(|tool| !matches!(tool.acquire, Acquire::UvVenv { .. })).count();
        let everything: Vec<Option<Option<&str>>> = TOOLS.iter().map(|_| Some(None)).collect();
        assert_eq!(lookups_needed(&everything), release_tools + 1, "ffmpeg's listing on top");
        let exact: Vec<Option<Option<&str>>> = TOOLS
            .iter()
            .map(|tool| Some((tool.dir == "ffmpeg").then_some("n9.0 @ autobuild-2026-09-30-13-08")))
            .collect();
        assert_eq!(lookups_needed(&exact), release_tools, "an exact pin names the release itself");
        let nothing: Vec<Option<Option<&str>>> = TOOLS.iter().map(|_| None).collect();
        assert_eq!(lookups_needed(&nothing), 0);
    }

    /// A failed lookup is reported with its reason, worded by what it means here: nothing is
    /// fetched, and a bundle already in place is kept.
    #[test]
    fn a_failed_lookup_is_reported_with_its_reason_and_fetches_nothing() {
        let ffmpeg = TOOLS.iter().find(|tool| tool.dir == "ffmpeg").expect("ffmpeg in the table");
        let dir = std::env::temp_dir().join(format!("bashrs_failed_lookup_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let offline = Err("could not reach GitHub: Could not resolve host: api.github.com".into());
        let gone = Err("GitHub answered 404, no such release".into());
        let limited = Err(LookupError::RateLimited { limit: Some(60), resets_in: Some(Duration::from_secs(1380)) });
        let mut problems = Problems::default();

        assert_eq!(release_url_if_stale(ffmpeg, &offline, &dir, None, &mut problems), None, "nothing bundled yet");
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/ffmpeg"), "").unwrap();
        assert_eq!(release_url_if_stale(ffmpeg, &offline, &dir, None, &mut problems), None, "a bundle in place");
        let pin = Some("n9.0 @ autobuild-2026-08-24-13-10");
        assert_eq!(release_url_if_stale(ffmpeg, &gone, &dir, pin, &mut problems), None, "a pin that is gone");
        assert_eq!(release_url_if_stale(ffmpeg, &limited, &dir, None, &mut problems), None, "GitHub's limit spent");
        assert_eq!(
            problems.lines(),
            [
                "tools: could not determine the ffmpeg build for this machine (could not reach GitHub: Could not resolve host: api.github.com)",
                "tools: could not check for a newer ffmpeg build (could not reach GitHub: Could not resolve host: api.github.com) — keeping the bundled one",
                "tools: could not resolve the recorded ffmpeg pin n9.0 @ autobuild-2026-08-24-13-10 (GitHub answered 404, no such release) — keeping the bundled one",
                "tools: could not check for a newer ffmpeg build (GitHub's hourly lookup limit is used up, for another 23 minutes) — keeping the bundled one",
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gitlfs_asset_picks_this_arch_linux_tarball_over_every_sibling() {
        // A real release carries every OS/arch plus a checksum file; the finder must land on
        // exactly the Linux tarball for the asked arch — not another OS, not the other arch, and
        // never the `.sha256` beside it (which also contains the tarball's name).
        let json = r#"{"assets":[
            {"browser_download_url":"https://github.com/git-lfs/git-lfs/releases/download/v3.5.1/git-lfs-darwin-amd64-v3.5.1.zip"},
            {"browser_download_url":"https://github.com/git-lfs/git-lfs/releases/download/v3.5.1/git-lfs-linux-arm64-v3.5.1.tar.gz"},
            {"browser_download_url":"https://github.com/git-lfs/git-lfs/releases/download/v3.5.1/git-lfs-linux-amd64-v3.5.1.tar.gz"},
            {"browser_download_url":"https://github.com/git-lfs/git-lfs/releases/download/v3.5.1/git-lfs-linux-amd64-v3.5.1.tar.gz.sha256"}]}"#;
        assert_eq!(
            find_gitlfs_asset(json, "amd64").unwrap(),
            "https://github.com/git-lfs/git-lfs/releases/download/v3.5.1/git-lfs-linux-amd64-v3.5.1.tar.gz",
            "the amd64 Linux tarball, not the arm64 sibling, the mac build, or the .sha256"
        );
        assert_eq!(
            find_gitlfs_asset(json, "arm64").unwrap(),
            "https://github.com/git-lfs/git-lfs/releases/download/v3.5.1/git-lfs-linux-arm64-v3.5.1.tar.gz"
        );
        // The release tag still reads out of the chosen URL, so the version marker records it.
        assert_eq!(
            release_tag(&find_gitlfs_asset(json, "amd64").unwrap()).as_deref(),
            Some("v3.5.1")
        );
        // A machine whose arch git-lfs doesn't publish gets nothing, not a wrong build.
        assert!(find_gitlfs_asset(json, "riscv64").is_none());
    }

    #[test]
    fn venv_shims_exec_the_real_in_venv_path() {
        // A symlink here would break pyvenv.cfg discovery (python checks beside the path it was
        // invoked as, symlinks unresolved) — the script must exec the true venv path, spelled
        // via $HOME so a symlinked ~/.bashrs keeps working.
        let shim = venv_shim("python", "bin/python3");
        assert!(shim.starts_with("#!/bin/sh\n"), "{shim}");
        assert!(
            shim.contains("exec \"$HOME/.bashrs/tools/python/bin/python3\" \"$@\""),
            "{shim}"
        );
    }

    #[test]
    fn fetch_unpacks_an_archive_with_its_root_folder_stripped_and_cleans_up() {
        // Offline end-to-end of the download+unpack contract: curl reads a file:// URL, tar
        // strips the archive's root folder, and the temporary download is removed.
        let base = std::env::temp_dir().join(format!("bashrs_fetch_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("src/pkg-1.0/bin")).unwrap();
        std::fs::write(base.join("src/pkg-1.0/bin/hello"), "hi").unwrap();
        let archive = base.join("pkg.tgz");
        assert!(Command::new("tar")
            .arg("-czf").arg(&archive)
            .arg("-C").arg(base.join("src"))
            .arg("pkg-1.0")
            .status().unwrap().success());

        let dest = base.join("installed");
        fetch(&format!("file://{}", archive.display()), &dest).expect("fetch should succeed");
        assert!(dest.join("bin/hello").exists(), "root folder must be stripped");
        assert!(!dest.join("download.tmp").exists(), "the temporary archive must be removed");

        // A dead URL fails cleanly, with the URL named.
        let err = fetch("file:///no/such/archive.tgz", &base.join("failed")).unwrap_err();
        assert!(err.contains("download failed"), "{err}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn ffmpeg_asset_takes_the_newest_release_channel_static_gpl_build() {
        let json = r#"{"assets":[
            {"browser_download_url":"https://x/ffmpeg-master-latest-linux64-gpl.tar.xz"},
            {"browser_download_url":"https://x/ffmpeg-n7.1-latest-linux64-gpl-7.1.tar.xz"},
            {"browser_download_url":"https://x/ffmpeg-n8.1-latest-linux64-gpl-shared-8.1.tar.xz"},
            {"browser_download_url":"https://x/ffmpeg-n8.1-latest-linux64-lgpl-8.1.tar.xz"},
            {"browser_download_url":"https://x/ffmpeg-n8.1-latest-linuxarm64-gpl-8.1.tar.xz"},
            {"browser_download_url":"https://x/ffmpeg-n8.1-latest-linux64-gpl-8.1.tar.xz"},
            {"browser_download_url":"https://x/ffmpeg-n10.0-latest-linux64-gpl-10.0.tar.xz"}
        ]}"#;
        assert_eq!(
            find_ffmpeg_asset(json, "linux64", None).as_deref(),
            Some("https://x/ffmpeg-n10.0-latest-linux64-gpl-10.0.tar.xz"),
            "must skip master/shared/lgpl/other-arch and order channels numerically (10 > 8)"
        );
        assert_eq!(find_ffmpeg_asset("{}", "linux64", None), None);
        // A Carstay pin narrows the pick to the recorded channel — never "newest instead".
        assert_eq!(
            find_ffmpeg_asset(json, "linux64", Some("n7.1")).as_deref(),
            Some("https://x/ffmpeg-n7.1-latest-linux64-gpl-7.1.tar.xz"),
            "the pinned channel wins even though n10.0 exists"
        );
        assert_eq!(
            find_ffmpeg_asset(json, "linux64", Some("n9.9")),
            None,
            "a pinned channel that's no longer published yields nothing, not the newest"
        );
        assert_eq!(find_ffmpeg_asset(json, "linux64", Some("garbage")), None);
    }

    #[test]
    fn a_dated_ffmpeg_release_serves_the_pinned_channels_exact_build() {
        // A dated autobuild release's asset naming (exact versions, no "-latest-") — what a
        // full `"n8.1 @ autobuild-…"` Carstay pin restores from.
        let json = r#"{"assets":[
            {"browser_download_url":"https://x/ffmpeg-N-125674-g9bc73ba344-linux64-gpl.tar.xz"},
            {"browser_download_url":"https://x/ffmpeg-n7.1.5-2-g998de74adf-linux64-gpl-7.1.tar.xz"},
            {"browser_download_url":"https://x/ffmpeg-n8.1.2-22-g94138f6973-linux64-gpl-shared-8.1.tar.xz"},
            {"browser_download_url":"https://x/ffmpeg-n8.1.2-22-g94138f6973-linux64-gpl-8.1.tar.xz"}
        ]}"#;
        assert_eq!(
            find_ffmpeg_asset(json, "linux64", Some("n8.1")).as_deref(),
            Some("https://x/ffmpeg-n8.1.2-22-g94138f6973-linux64-gpl-8.1.tar.xz"),
            "the exact build of the pinned channel, skipping master and shared variants"
        );
        assert_eq!(
            find_ffmpeg_asset(json, "linux64", Some("n7.1")).as_deref(),
            Some("https://x/ffmpeg-n7.1.5-2-g998de74adf-linux64-gpl-7.1.tar.xz")
        );
    }

    #[test]
    fn the_provisioned_ffmpeg_version_pairs_the_channel_with_the_dated_build_tag() {
        let ffmpeg = TOOLS.iter().find(|tool| tool.dir == "ffmpeg").expect("ffmpeg in the table");
        let dir = std::env::temp_dir().join(format!("bashrs_prov_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let rolling = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-n8.1-latest-linux64-gpl-8.1.tar.xz";
        std::fs::write(dir.join(SOURCE_MARKER), rolling).unwrap();
        assert_eq!(
            provisioned_version(ffmpeg, &dir).as_deref(),
            Some("n8.1"),
            "no captured tag → channel-only (older bundles keep working)"
        );
        std::fs::write(dir.join(TAG_MARKER), "autobuild-2026-07-19-13-12\n").unwrap();
        assert_eq!(
            provisioned_version(ffmpeg, &dir).as_deref(),
            Some("n8.1 @ autobuild-2026-07-19-13-12"),
            "the fetch-time tag makes the record an exact build"
        );
        // A bundle restored FROM a dated release carries its tag in the URL itself.
        let dated = "https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-06-30-13-01/ffmpeg-n8.1.2-22-g94138f6973-linux64-gpl-8.1.tar.xz";
        std::fs::write(dir.join(SOURCE_MARKER), dated).unwrap();
        assert_eq!(
            provisioned_version(ffmpeg, &dir).as_deref(),
            Some("n8.1 @ autobuild-2026-06-30-13-01"),
            "the URL's own tag wins over a stale marker"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn uv_asset_is_found_by_shape_and_ignores_lookalikes() {
        let json = r#"{"assets":[
            {"browser_download_url":"https://x/uv-x86_64-unknown-linux-gnu.tar.gz.sha256"},
            {"browser_download_url":"https://x/uv-x86_64-unknown-linux-musl.tar.gz"},
            {"browser_download_url":"https://x/uv-aarch64-unknown-linux-gnu.tar.gz"},
            {"browser_download_url":"https://x/uv-x86_64-unknown-linux-gnu.tar.gz"}
        ]}"#;
        assert_eq!(
            find_uv_asset(json, "x86_64").as_deref(),
            Some("https://x/uv-x86_64-unknown-linux-gnu.tar.gz"),
            "must skip the .sha256, the musl build, and the other arch"
        );
        assert_eq!(find_uv_asset("{}", "x86_64"), None);
    }

}
