//! The tool drivers — feature-facing logic that *commands* the bundled tools
//! ([`crate::tools`] acquires, resolves, and exposes them; this layer drives them): the python
//! environment management behind the `py_*` commands, the yt-dlp orchestration behind `dl`,
//! and the companion-repo ("stainless") sync. A driver may use any tool and everything below;
//! the command categories stay thin argument shells over drivers. Split from `tools` because
//! the two change at different speeds — plumbing is stable, drivers grow with every feature.

pub mod python;
pub mod carstay;
pub mod stainless;
pub(crate) mod youtube;

/// SIDE EFFECTS — COMPILE.sh's companion-provisioning step (the hidden `install-stainless`
/// command, run from the freshly built binary before `install-shell`): bundle/update the
/// external tools, install yt-dlp's python helpers, sync the companion repos, and record the
/// landed versions in Carstay.toml. With `use_stable_carstay`, the versions RECORDED there are
/// provisioned instead of the latest releases — the manifest becomes the input and is not
/// rewritten. Best-effort throughout (each sync only warns), so a missing network can't abort
/// the compile.
pub fn install_stainless(use_stable_carstay: bool) {
    let recorded = if use_stable_carstay {
        match carstay::recorded() {
            Some(recorded) => {
                println!("install-stainless: provisioning the versions recorded in Carstay.toml — not the latest releases");
                Some(recorded)
            }
            None => {
                eprintln!("install-stainless: --use-stable-carstay needs a readable Carstay.toml (a normal sync writes one)");
                std::process::exit(1);
            }
        }
    } else {
        None
    };
    let tool_pins = recorded.as_ref().map(|rec| rec.tools.as_slice()).unwrap_or(&[]);
    let repo_pins = recorded.as_ref().map(|rec| rec.repos.as_slice()).unwrap_or(&[]);

    // The configuration's shape first (archiving an outdated file) — the syncs read it.
    if let Err(err) = crate::conf::config_file::ensure_current() {
        eprintln!("bashrs: could not prepare the configuration: {err}");
    }
    // Tools before repos: the companion repos' python dependencies install into the bundled
    // environment, so python and uv must already be in place when the repos sync.
    let fetched_new = crate::tools::sync(tool_pins);
    // Provision on top of the bundles: yt-dlp's python helpers, then the companion repos
    // (whose python_deps install into the environment the tools sync just prepared).
    python::ensure_ytdlp_deps();
    stainless::sync(repo_pins);
    // The receipt, last: record what versions all of the above actually landed — every section
    // in one write, so the manifest never shows a half-synced state.
    if recorded.is_none() {
        carstay::record();
        // Anything newly fetched deserves the blue heads-up: how to get back to the last
        // stable set if the fresh versions misbehave.
        if fetched_new {
            carstay::stability_revert_notice();
        }
    }
}
