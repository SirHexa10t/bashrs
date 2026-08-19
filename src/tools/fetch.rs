//! The compile-time side of the bundled tools: [`sync`], run by the hidden `install-stainless` command from
//! `COMPILE.sh`, keeps every tool bundled *and current* — like the companion repos following
//! `main`, each compile discovers the latest published build and re-fetches when it changed
//! (tracked by a `.source_url` marker beside each bundle, so an unchanged build costs one API
//! call, not a re-download). Best-effort throughout: a failure warns and keeps what's bundled,
//! never aborts the compile. Asset URLs are discovered from each project's latest GitHub release
//! by name-shape, never hard-pinned — pinned names rot (a BtbN channel 404s once it rotates out).

use std::path::Path;
use std::process::Command;

use crate::conf::config_file;
use crate::support::exec;
use crate::tools::{bin_dir, root, Acquire, Group, Tool, TOOLS};

/// The per-bundle marker recording which published archive it came from.
pub(super) const SOURCE_MARKER: &str = ".source_url";

/// The per-bundle marker recording the dated release tag a rolling-URL bundle was built from
/// (ffmpeg: BtbN's `autobuild-…`), written at fetch time — the URL alone can't say.
const TAG_MARKER: &str = ".build_tag";

/// SIDE EFFECTS — bundle the tools, keeping each current per its acquisition mode. The
/// `[tools] always_bundle_languages` / `always_bundle_utilities` settings decide, per group,
/// whether to bundle even what the system already provides (languages default to yes — bashrs
/// runs on them, so the project controlling their versions is the reliable default) or only
/// what the system lacks (the utilities' default).
/// `pins` (from Carstay.toml, via `--use-stable-carstay`) names the exact version to provision per tool —
/// a tool absent from it, or an empty slice, means "the latest published", today's default.
/// Returns whether anything new landed (a fetch or a fresh environment) — the caller's cue to
/// point at the stability-revert path.
pub fn sync(pins: &[(String, String)]) -> bool {
    let bundle_languages = config_file::always_bundle_languages();
    let bundle_utilities = config_file::always_bundle_utilities();

    // Plan first, on local information only: a config flag and a PATH probe decide whether this
    // machine bundles each tool at all, and Carstay names the version when pinned. Settling the
    // whole set up front costs nothing and keeps these messages in table order.
    // `None` — not bundled here; `Some(pin)` — bundled, at `pin`'s recorded version if any.
    let plan: Vec<Option<Option<&str>>> = TOOLS
        .iter()
        .map(|tool| {
            let (always, field) = match tool.group {
                Group::Language => (bundle_languages, "always_bundle_languages"),
                Group::Utility => (bundle_utilities, "always_bundle_utilities"),
            };
            let probe = tool.bins[0].0;
            if !always && exec::on_path(probe) {
                eprintln!(
                    "tools: {probe} is already on this system — not bundling (override: [tools] {field} in {})",
                    config_file::CONFIG_FILE
                );
                return None;
            }
            let pin = pins.iter().find(|(name, _)| name == tool.dir).map(|(_, pin)| pin.as_str());
            if let Some(pin) = pin {
                eprintln!("tools: {} pinned to the recorded {pin}", tool.dir);
            }
            Some(pin)
        })
        .collect();

    // Then discover every release URL at once. This phase IS what an up-to-date sync costs —
    // one GitHub API call per tool, nothing downloaded — so running the calls serially made the
    // no-op case pay their sum. They're independent read-only lookups (a `curl` of the releases
    // API, no shared state), so the wall clock becomes the slowest single call instead.
    let urls: Vec<Option<String>> = std::thread::scope(|scope| {
        let handles: Vec<Option<_>> = TOOLS
            .iter()
            .zip(&plan)
            .map(|(tool, planned)| {
                let pin = (*planned)?;
                let resolve = match &tool.acquire {
                    Acquire::Archive { url, .. } => *url,
                    Acquire::Binary(url) => *url,
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
            .map(|handle| handle.and_then(|handle| handle.join().ok().flatten()))
            .collect()
    });

    // Act in table order — load-bearing here even though discovery wasn't: uv must land before
    // the python venv whose creation runs it, and deno's zip is unpacked by that python. Each
    // step already holds its URL, so keeping the order now costs a comparison, not a round trip.
    let mut fetched_any = false;
    for ((tool, planned), url) in TOOLS.iter().zip(&plan).zip(&urls) {
        let Some(pin) = *planned else { continue };
        let dir = root().join(tool.dir);
        fetched_any |= match &tool.acquire {
            Acquire::Archive { dated_tag, .. } => {
                sync_archive(tool, url.as_deref(), &dir, pin, *dated_tag)
            }
            Acquire::Binary(_) => sync_binary(tool, url.as_deref(), &dir, pin),
            Acquire::UvVenv { python } => sync_python_venv(python, &dir, pin),
        };
    }
    // Whatever is bundled — fetched just now or on an earlier compile — gets a PATH shim, so the
    // tools also win when named as arguments (see `tools::shell_setup`).
    for tool in TOOLS {
        ensure_shims(tool);
    }
    fetched_any
}

/// The freshness check both release modes share: `None` when the bundle at `dir` was already
/// built from the latest published URL (per its `.source_url` marker) or no URL could be
/// determined (diagnostics emitted here) — else the URL to (re)build from. `url` is the wanted
/// asset as [`sync`]'s parallel discovery phase resolved it, `None` when that lookup found
/// nothing; the comparison itself is local.
fn release_url_if_stale(
    tool: &Tool,
    url: Option<&str>,
    dir: &Path,
    pin: Option<&str>,
) -> Option<String> {
    let installed = dir.join(tool.bins[0].1).exists();
    let Some(url) = url else {
        if pin.is_some() {
            eprintln!("tools: could not resolve the recorded {} pin (release gone, or offline) — keeping the bundled one", tool.dir);
        } else if installed {
            eprintln!("tools: could not check for a newer {} build — keeping the bundled one", tool.dir);
        } else {
            eprintln!("tools: could not determine a {} build for this machine (offline, or unsupported architecture)", tool.dir);
        }
        return None;
    };
    let recorded = std::fs::read_to_string(dir.join(SOURCE_MARKER)).unwrap_or_default();
    (!installed || recorded.trim() != url).then(|| url.to_string())
}

/// The archive mode: re-fetch when the wanted URL (latest published, or the `pin`ned release in
/// `--use-stable-carstay` mode) differs from the bundled one's marker. Returns whether a fetch
/// happened. A tool with a `dated_tag` resolver gets its exact-build tag captured at fetch time
/// (the stale one is dropped first, so a failed lookup can't mislabel the new bundle).
fn sync_archive(
    tool: &Tool,
    url: Option<&str>,
    dir: &Path,
    pin: Option<&str>,
    dated_tag: Option<fn() -> Option<String>>,
) -> bool {
    let Some(url) = release_url_if_stale(tool, url, dir, pin) else { return false };
    eprintln!("tools: fetching {} into {}", tool.dir, dir.display());
    match fetch(&url, dir) {
        Ok(()) => {
            let _ = std::fs::write(dir.join(SOURCE_MARKER), &url);
            let _ = std::fs::remove_file(dir.join(TAG_MARKER));
            if let Some(tag) = dated_tag.and_then(|resolve| resolve()) {
                let _ = std::fs::write(dir.join(TAG_MARKER), tag);
            }
            true
        }
        Err(msg) => {
            eprintln!("tools: could not bundle {}: {msg}", tool.dir);
            false
        }
    }
}

/// The single-binary mode: same freshness contract as [`sync_archive`], but the download *is*
/// the program — written to the tool's first `bins` path and made executable.
fn sync_binary(tool: &Tool, url: Option<&str>, dir: &Path, pin: Option<&str>) -> bool {
    let Some(url) = release_url_if_stale(tool, url, dir, pin) else { return false };
    eprintln!("tools: fetching {} into {}", tool.dir, dir.display());
    match download_binary(&url, &dir.join(tool.bins[0].1)) {
        Ok(()) => {
            let _ = std::fs::write(dir.join(SOURCE_MARKER), &url);
            true
        }
        Err(msg) => {
            eprintln!("tools: could not bundle {}: {msg}", tool.dir);
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
fn sync_python_venv(python: &str, dir: &Path, pin: Option<&str>) -> bool {
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
        eprintln!("tools: could not create the python environment");
    } else if let Some(pin) = pin {
        // Recorded mode never moves the interpreter — just say so when it drifted from the
        // record (adopting the pin means recreating the environment, which drops installed
        // packages, so it stays the user's call).
        match python_version(dir) {
            Some(actual) if actual != pin => eprintln!(
                "tools: bundled python is {actual} but Carstay.toml records {pin} — delete {} and re-run to adopt the recorded one",
                dir.display()
            ),
            _ => {}
        }
    } else if !uv_managed(&["python", "upgrade", python]) {
        eprintln!("tools: could not check for a python upgrade — keeping the current one");
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
/// the interpreter's own report for the venv; and for rolling-URL tools (ffmpeg) the channel
/// plus the dated build tag captured at fetch time — `"n8.1 @ autobuild-…"` — so a restore can
/// name the exact build. `None` when nothing is bundled.
pub(super) fn provisioned_version(tool: &Tool, dir: &Path) -> Option<String> {
    let marker_url =
        || Some(std::fs::read_to_string(dir.join(SOURCE_MARKER)).ok()?.trim().to_string());
    match &tool.acquire {
        Acquire::UvVenv { .. } => python_version(dir),
        Acquire::Binary(_) | Acquire::Archive { dated_tag: None, .. } => {
            release_tag(&marker_url()?)
        }
        Acquire::Archive { dated_tag: Some(_), .. } => {
            let url = marker_url()?;
            let channel = asset_channel(&url).map(|(major, minor)| format!("n{major}.{minor}"))?;
            // The exact-build tag: in the URL itself when the bundle came from a dated release
            // (a pinned restore), else the tag captured beside the bundle at fetch time.
            let tag = release_tag(&url)
                .filter(|tag| tag != "latest")
                .or_else(|| Some(std::fs::read_to_string(dir.join(TAG_MARKER)).ok()?.trim().to_string()))
                .filter(|tag| !tag.is_empty());
            Some(match tag {
                Some(tag) => format!("{channel} @ {tag}"),
                None => channel,
            })
        }
    }
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
fn ensure_shims(tool: &Tool) {
    for (name, rel) in tool.bins {
        if !root().join(tool.dir).join(rel).exists() {
            let _ = std::fs::remove_file(bin_dir().join(name));
            continue;
        }
        let link = bin_dir().join(name);
        let _ = std::fs::create_dir_all(bin_dir());
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
            eprintln!("tools: could not shim {name}: {err}");
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
fn release_json(repo: &str, tag: Option<&str>) -> Option<String> {
    let api = match tag {
        Some(tag) => format!("https://api.github.com/repos/{repo}/releases/tags/{tag}"),
        None => format!("https://api.github.com/repos/{repo}/releases/latest"),
    };
    exec::capture_stdout("curl", ["-fsSL", &api])
}

/// BtbN's official ffmpeg static builds (ffmpeg + ffprobe in one archive): the newest release
/// channel's asset for this architecture, discovered from the latest autobuild release. A pin
/// (a Carstay value) narrows that: `"n8.1"` picks the channel from the same rolling release,
/// and the full `"n8.1 @ autobuild-…"` form fetches the EXACT recorded build from its dated
/// release (BtbN keeps recent dailies plus long-term month-end builds).
pub(super) fn ffmpeg_url(pin: Option<&str>) -> Option<String> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "linux64",
        "aarch64" => "linuxarm64",
        _ => return None,
    };
    let (channel, tag) = match pin {
        None => (None, None),
        Some(pin) => match pin.split_once(" @ ") {
            Some((channel, tag)) => (Some(channel.trim()), Some(tag.trim())),
            None => (Some(pin), None), // channel-only (older recordings, hand edits)
        },
    };
    // Without a dated tag, the rolling `latest` release serves every still-maintained channel
    // side by side; with one, that dated release's own assets are the exact build.
    find_ffmpeg_asset(&release_json("BtbN/FFmpeg-Builds", tag)?, arch, channel)
}

/// The dated tag of BtbN's newest autobuild release — the exact-build identity the rolling
/// `latest` URL can't provide (its assets are replaced in place). Captured at fetch time and
/// recorded beside the bundle, then in Carstay.toml.
pub(super) fn ffmpeg_dated_tag() -> Option<String> {
    let api = "https://api.github.com/repos/BtbN/FFmpeg-Builds/releases?per_page=5";
    let json = exec::capture_stdout("curl", ["-fsSL", api])?;
    json.split('"')
        .find(|token| token.starts_with("autobuild-"))
        .map(str::to_owned)
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
pub(super) fn uv_url(pin: Option<&str>) -> Option<String> {
    let arch = match std::env::consts::ARCH {
        arch @ ("x86_64" | "aarch64") => arch,
        _ => return None,
    };
    find_uv_asset(&release_json("astral-sh/uv", pin)?, arch)
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
pub(super) fn ytdlp_url(pin: Option<&str>) -> Option<String> {
    find_ytdlp_asset(&release_json("yt-dlp/yt-dlp", pin)?, "yt-dlp")
}

/// deno's static binary zip for this machine, discovered from the latest release — bundled to
/// serve yt-dlp, whose YouTube extractor needs a JS runtime (deno is the one it enables by
/// default) or some formats go missing.
pub(super) fn deno_url(pin: Option<&str>) -> Option<String> {
    let asset = match std::env::consts::ARCH {
        "x86_64" => "deno-x86_64-unknown-linux-gnu.zip",
        "aarch64" => "deno-aarch64-unknown-linux-gnu.zip",
        _ => return None,
    };
    find_ytdlp_asset(&release_json("denoland/deno", pin)?, asset)
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
/// (ffmpeg's rolling `latest` tag names nothing; [`provisioned_version`] derives its identity
/// from [`asset_channel`] and the [`TAG_MARKER`] instead.)
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
