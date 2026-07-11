//! The compile-time side of the bundled tools: [`sync`], run by the `stainless_sync` binary from
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
const SOURCE_MARKER: &str = ".source_url";

/// SIDE EFFECTS — bundle the tools, keeping each current per its acquisition mode. The
/// `[tools] always_bundle_languages` / `always_bundle_utilities` settings decide, per group,
/// whether to bundle even what the system already provides (languages default to yes — bashrs
/// runs on them, so the project controlling their versions is the reliable default) or only
/// what the system lacks (the utilities' default).
pub fn sync() {
    let bundle_languages = config_file::always_bundle_languages();
    let bundle_utilities = config_file::always_bundle_utilities();
    for tool in TOOLS {
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
            continue;
        }
        let dir = root().join(tool.dir);
        match &tool.acquire {
            Acquire::Archive(url) => sync_archive(tool, *url, &dir),
            Acquire::UvVenv { python } => sync_python_venv(python, &dir),
        }
    }
    // Whatever is bundled — fetched just now or on an earlier compile — gets a PATH shim, so the
    // tools also win when named as arguments (see `tools::shell_setup`).
    for tool in TOOLS {
        ensure_shims(tool);
    }
}

/// The archive mode: re-fetch when the latest published URL differs from the bundled one's marker.
fn sync_archive(tool: &Tool, url: fn() -> Option<String>, dir: &Path) {
    let installed = dir.join(tool.bins[0].1).exists();
    let Some(url) = url() else {
        if installed {
            eprintln!("tools: could not check for a newer {} build — keeping the bundled one", tool.dir);
        } else {
            eprintln!("tools: could not determine a {} build for this machine (offline, or unsupported architecture)", tool.dir);
        }
        return;
    };
    let recorded = std::fs::read_to_string(dir.join(SOURCE_MARKER)).unwrap_or_default();
    if installed && recorded.trim() == url {
        return; // already on the latest published build
    }
    eprintln!("tools: fetching {} into {}", tool.dir, dir.display());
    match fetch(&url, dir) {
        Ok(()) => {
            let _ = std::fs::write(dir.join(SOURCE_MARKER), &url);
        }
        Err(msg) => eprintln!("tools: could not bundle {}: {msg}", tool.dir),
    }
}

/// The uv-venv mode: a venv at `dir` gives the stable `bin/python3` everything else names, over a
/// managed interpreter uv installs under `~/.bashrs/tools/interpreters`. Creation downloads the
/// requested version; afterwards `uv python upgrade` keeps the interpreter's patch level current —
/// the venv, created against the minor version, follows transparently, and its site-packages
/// survive (package updates are `py_update`'s job, not this one's).
fn sync_python_venv(python: &str, dir: &Path) {
    if dir.exists() && !dir.join("pyvenv.cfg").exists() {
        // The pre-uv layout (an unpacked python-build-standalone archive): replace it. The repo
        // dependencies are reinstalled by the stainless sync that follows; manually installed
        // packages need a `py_install` again.
        eprintln!("tools: migrating the bundled python to a uv-managed environment");
        let _ = std::fs::remove_dir_all(dir);
    }
    if !dir.join("pyvenv.cfg").exists() {
        eprintln!("tools: creating the python {python} environment at {}", dir.display());
        if !uv_managed(&["venv", &dir.display().to_string(), "--python", python]) {
            eprintln!("tools: could not create the python environment");
        }
    } else if !uv_managed(&["python", "upgrade", python]) {
        eprintln!("tools: could not check for a python upgrade — keeping the current one");
    }
}

/// Run `uv` with the managed-interpreter story pinned under `~/.bashrs/tools/`: interpreters land
/// in [`super::interpreters_dir`], and system pythons are never linked against (a venv onto one
/// would break whenever the system python moves or updates).
fn uv_managed(args: &[&str]) -> bool {
    let run = Command::new(super::resolve("uv"))
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
    format!("#!/bin/sh\nexec \"$HOME/.bashrs/tools/{tool_dir}/{rel}\" \"$@\"\n")
}

/// Download `url` and unpack the archive's contents (root folder stripped) into `dir`.
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
    let unpacked = Command::new("tar")
        .arg("-xf")
        .arg(&archive)
        .arg("-C")
        .arg(dir)
        .arg("--strip-components=1")
        .status();
    let _ = std::fs::remove_file(&archive);
    if !matches!(unpacked, Ok(status) if status.success()) {
        return Err("could not unpack the archive".into());
    }
    Ok(())
}

/// The latest-release JSON of a GitHub `owner/repo` — one small API fetch, from which the asset
/// URLs are picked by shape.
fn latest_release(repo: &str) -> Option<String> {
    let api = format!("https://api.github.com/repos/{repo}/releases/latest");
    exec::capture_stdout("curl", ["-fsSL", &api])
}

/// BtbN's official ffmpeg static builds (ffmpeg + ffprobe in one archive): the newest release
/// channel's asset for this architecture, discovered from the latest autobuild release.
pub(super) fn ffmpeg_url() -> Option<String> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "linux64",
        "aarch64" => "linuxarm64",
        _ => return None,
    };
    find_ffmpeg_asset(&latest_release("BtbN/FFmpeg-Builds")?, arch)
}

/// The newest release-channel static GPL build for `arch` — assets are named like
/// `ffmpeg-n8.0-latest-linux64-gpl-8.0.tar.xz`. The master-branch build (no version after `gpl`)
/// and the shared/lgpl variants are skipped; among release channels the highest `n<major.minor>`
/// wins, since channels rotate out as new majors land.
fn find_ffmpeg_asset(json: &str, arch: &str) -> Option<String> {
    let marker = format!("-latest-{arch}-gpl-");
    json.split('"')
        .filter(|token| {
            token.starts_with("https://")
                && token.contains(&marker)
                && !token.contains("-shared-")
                && token.ends_with(".tar.xz")
        })
        .filter_map(|token| Some((ffmpeg_channel(token)?, token)))
        .max_by_key(|(channel, _)| *channel)
        .map(|(_, token)| token.to_owned())
}

/// The `(major, minor)` of an ffmpeg asset URL's `…/ffmpeg-n<major.minor>-latest-…` channel.
fn ffmpeg_channel(url: &str) -> Option<(u32, u32)> {
    let version = url.rsplit("/ffmpeg-n").next()?.split("-latest").next()?;
    let (major, minor) = version.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// astral-sh's uv — a single static binary that manages the bundled python's packages
/// (`uv pip … --python <bundled>`; see [`super::python`]).
pub(super) fn uv_url() -> Option<String> {
    let arch = match std::env::consts::ARCH {
        arch @ ("x86_64" | "aarch64") => arch,
        _ => return None,
    };
    find_uv_asset(&latest_release("astral-sh/uv")?, arch)
}

/// The latest release's `uv-<arch>-unknown-linux-gnu.tar.gz` download URL (skipping the musl
/// variants and checksum files by the exact suffix).
fn find_uv_asset(json: &str, arch: &str) -> Option<String> {
    let suffix = format!("/uv-{arch}-unknown-linux-gnu.tar.gz");
    json.split('"')
        .find(|token| token.starts_with("https://") && token.ends_with(&suffix))
        .map(str::to_owned)
}


#[cfg(test)]
mod tests {
    use super::*;

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
            find_ffmpeg_asset(json, "linux64").as_deref(),
            Some("https://x/ffmpeg-n10.0-latest-linux64-gpl-10.0.tar.xz"),
            "must skip master/shared/lgpl/other-arch and order channels numerically (10 > 8)"
        );
        assert_eq!(find_ffmpeg_asset("{}", "linux64"), None);
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
