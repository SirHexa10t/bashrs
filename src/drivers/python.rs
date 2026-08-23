//! Package management for the bundled python environment (`~/.bashrs/tools/python`), via `uv` —
//! itself a bundled tool. Three callers: the stainless repos' runtime dependencies (installed by
//! [`super::stainless`]'s sync), and the user-facing `py_install` / `py_update` commands
//! ([`crate::categories::python`]). Everything targets the *bundled* interpreter — the point is
//! an environment the project controls — so nothing here ever touches the system python. The
//! interpreter itself updates during bashrs's compilation, not here.

use std::path::{Path, PathBuf};

use crate::support::exec;

/// The bundled interpreter, when present (`None`: not bundled on this machine).
pub(crate) fn interpreter() -> Option<PathBuf> {
    let python = crate::tools::root().join("python").join("bin").join("python3");
    python.exists().then_some(python)
}

/// The freeze file capturing the environment before the most recent package change —
/// `py_rollback`'s restore point, rewritten by every mutating operation (and by design *not* by
/// [`rollback`] itself, so rolling back twice is a no-op rather than a ping-pong).
fn snapshot_file() -> PathBuf {
    crate::tools::root().join("python").join(".packages-before-last-change")
}

/// Record the environment's current package set as the rollback point.
fn snapshot(python: &Path) {
    if let Some(frozen) = freeze(python) {
        let _ = std::fs::write(snapshot_file(), frozen);
    }
}

/// `uv pip freeze` of the bundled environment.
fn freeze(python: &Path) -> Option<String> {
    let args: Vec<String> =
        vec!["pip".into(), "freeze".into(), "--python".into(), python.display().to_string()];
    exec::capture_stdout(crate::tools::resolve_uv(), args)
}

/// Restore the environment to exactly its pre-last-change package set (`uv pip sync`: versions
/// reverted AND packages added since are removed) — the escape hatch for a latest-version update
/// that broke something.
pub(crate) fn rollback() -> bool {
    let Some(python) = interpreter() else { return missing_env() };
    let snapshot = snapshot_file();
    if !snapshot.exists() {
        eprintln!("no package snapshot yet — nothing has been installed or updated to roll back");
        return false;
    }
    let args: Vec<String> = vec![
        "pip".into(),
        "sync".into(),
        "--python".into(),
        python.display().to_string(),
        snapshot.display().to_string(),
    ];
    exec::run_reporting(crate::tools::resolve_uv(), args)
}

/// Install `packages` into the bundled environment — latest versions, upgrading any already
/// present (the project follows upstream by default; pinning is a deliberate act, done by passing
/// a spec like `pkg==1.2` — which `--upgrade` still honors). Accepts anything `uv pip install`
/// does: bare names, `==`/`>=` specs, extras. Trivially `true` on an empty list; `false` — after
/// reporting — when the environment is missing or `uv` fails.
pub fn install<S: AsRef<str>>(packages: &[S]) -> bool {
    if packages.is_empty() {
        return true;
    }
    let Some(python) = interpreter() else { return missing_env() };
    snapshot(&python); // the pre-change set: what `py_rollback` restores
    uv_pip(&python, &["install", "--upgrade"], packages)
}

/// Upgrade every package installed in the bundled environment.
pub(crate) fn upgrade_all() -> bool {
    let Some(python) = interpreter() else { return missing_env() };
    let Some(frozen) = freeze(&python) else {
        return false;
    };
    let packages = package_names(&frozen);
    if packages.is_empty() {
        println!("nothing installed in the bundled python yet (see `py_install`)");
        return true;
    }
    let _ = std::fs::write(snapshot_file(), &frozen); // the pre-change set: `py_rollback`'s target
    uv_pip(&python, &["install", "--upgrade"], &packages)
}

/// Run `uv pip <action…> --python <bundled> <packages…>`, reporting failures.
fn uv_pip<S: AsRef<str>>(python: &Path, action: &[&str], packages: &[S]) -> bool {
    let mut argv: Vec<String> = vec!["pip".into()];
    argv.extend(action.iter().map(|word| word.to_string()));
    argv.push("--python".into());
    argv.push(python.display().to_string());
    argv.extend(packages.iter().map(|package| package.as_ref().to_string()));
    exec::run_reporting(crate::tools::resolve_uv(), argv)
}

/// The bare package names in `uv pip freeze` output (`name==version` lines; anything else —
/// editable installs, direct URLs — is skipped rather than guessed at).
fn package_names(freeze: &str) -> Vec<String> {
    freeze
        .lines()
        .filter_map(|line| line.split_once("==").map(|(name, _)| name.trim().to_string()))
        .collect()
}

fn missing_env() -> bool {
    eprintln!(
        "no bundled python at ~/.bashrs/tools/python — enable `[tools] always_bundle_languages` \
         (ALT+W) and recompile, or manage packages in your own environment"
    );
    false
}

/// The optional python packages that complete the bundled yt-dlp zipapp (which runs on this
/// python): `curl_cffi` restores the impersonation support YouTube increasingly demands
/// (403s/429s without it), `mutagen` powers thumbnail/tag embedding into audio formats
/// (ffmpeg covers the video containers), and `pycryptodomex` decrypts the AES-protected
/// streams some sites serve (vidl now expects it). A no-op for whatever is already importable,
/// or when yt-dlp isn't even bundled.
///
/// Pairs are `(import name, package name)` because they differ: `pycryptodomex` installs under
/// that name but is imported as `Cryptodome` — probing the package name would report it missing
/// on every run and reinstall it forever.
pub fn ensure_ytdlp_deps() {
    if crate::tools::resolve("yt-dlp") == "yt-dlp" {
        return;
    }
    let missing: Vec<&str> =
        [("curl_cffi", "curl_cffi"), ("mutagen", "mutagen"), ("Cryptodome", "pycryptodomex")]
        .iter()
        .filter(|(module, _)| {
            !exec::succeeds_quietly(
                crate::tools::resolve("python3"),
                ["-c", &format!("import {module}")],
            )
        })
        .map(|(_, package)| *package)
        .collect();
    if !missing.is_empty() {
        eprintln!("tools: installing yt-dlp's python helpers: {}", missing.join(", "));
        let _ = install(&missing);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_names_take_pinned_lines_and_skip_the_rest() {
        let freeze = "rich==13.7.0\nprompt_toolkit==3.0.51\n-e /some/editable\npkg @ https://x/y.whl\n";
        assert_eq!(package_names(freeze), ["rich", "prompt_toolkit"]);
        assert!(package_names("").is_empty());
    }

    #[test]
    fn interpreter_reports_presence_truthfully() {
        // Environment-adaptive: Some(bundled path) where the bundle exists, None elsewhere.
        match interpreter() {
            Some(python) => assert!(python.ends_with("tools/python/bin/python3"), "{python:?}"),
            None => assert!(!crate::tools::root().join("python/bin/python3").exists()),
        }
    }
}

