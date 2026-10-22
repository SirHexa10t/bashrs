//! The tool drivers — feature-facing logic that *commands* the bundled tools
//! ([`crate::tools`] acquires, resolves, and exposes them; this layer drives them): the python
//! environment management behind the `py_*` commands, the yt-dlp driving behind `dl` (now the `vidl` crate),
//! and the companion-repo ("stainless") sync. A driver may use any tool and everything below;
//! the command categories stay thin argument shells over drivers. Split from `tools` because
//! the two change at different speeds — plumbing is stable, drivers grow with every feature.

pub mod python;
pub mod carstay;
pub mod comfy_graph;
pub mod stainless;

use std::path::Path;

use crate::support::problems::Problems;

/// SIDE EFFECTS — COMPILE.sh's companion-provisioning step (the hidden `install-stainless`
/// command, run from the freshly built binary before `install-shell`): bundle/update the
/// external tools, install yt-dlp's python helpers, sync the companion repos, and record the
/// landed versions in Carstay.toml. With `use_stable_carstay`, the versions RECORDED there are
/// provisioned instead of the latest releases — the manifest becomes the input and is not
/// rewritten. Best-effort throughout (each sync only warns), so a missing network can't abort
/// the compile. Everything that could not be done is repeated once more at the end
/// (`repeat_problems`): into `problems_file`, which COMPILE.sh prints as the compile's last
/// word, or — run without one — straight to stderr.
pub fn install_stainless(use_stable_carstay: bool, problems_file: Option<&Path>) {
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

    let mut problems = Problems::default();
    // The configuration's shape first (archiving an outdated file) — the syncs read it.
    if let Err(err) = crate::conf::config_file::ensure_current() {
        problems.report(format!("bashrs: could not prepare the configuration: {err}"));
    }
    // Tools before repos: the companion repos' python dependencies install into the bundled
    // environment, so python and uv must already be in place when the repos sync.
    let fetched_new = crate::tools::sync(tool_pins, &mut problems);
    // Provision on top of the bundles: yt-dlp's python helpers, then the companion repos
    // (whose python_deps install into the environment the tools sync just prepared).
    python::ensure_ytdlp_deps(&mut problems);
    stainless::sync(repo_pins, &mut problems);
    // The receipt, last: record what versions all of the above actually landed — every section
    // in one write, so the manifest never shows a half-synced state.
    if recorded.is_none() {
        carstay::record(&mut problems);
        // Anything newly fetched deserves the blue heads-up: how to get back to the last
        // stable set if the fresh versions misbehave.
        if fetched_new {
            carstay::stability_revert_notice();
        }
    }
    repeat_problems(&problems, problems_file);
}

/// Hand the end-of-run summary on — into `file`, for COMPILE.sh to print after everything else,
/// or, run without one, straight to stderr. A clean run writes nothing, so the script's file
/// stays empty and it prints nothing either.
fn repeat_problems(problems: &Problems, file: Option<&Path>) {
    let Some(summary) = problems.summary() else { return };
    match file.map(|file| (file, std::fs::write(file, format!("{summary}\n")))) {
        Some((_, Ok(()))) => {}
        Some((file, Err(err))) => {
            eprintln!("install-stainless: could not write {}: {err}", file.display());
            eprintln!("{summary}");
        }
        None => eprintln!("{summary}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The handover COMPILE.sh relies on: problems land in its file, and a clean run leaves the
    /// file as it found it — empty — so the script has nothing to print.
    #[test]
    fn problems_land_in_the_file_and_a_clean_run_leaves_it_empty() {
        let dir = std::env::temp_dir().join(format!("bashrs_repeat_problems_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("problems");
        std::fs::write(&file, "").unwrap();

        repeat_problems(&Problems::default(), Some(&file));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "", "a clean run writes nothing");

        let mut problems = Problems::default();
        problems.report("tools: could not check for a newer ffmpeg build (could not reach GitHub: …)");
        repeat_problems(&problems, Some(&file));
        let written = std::fs::read_to_string(&file).unwrap();
        assert_eq!(written, format!("{}\n", problems.summary().unwrap()), "the summary, whole");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
