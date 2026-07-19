//! The non-Cargo dependency record (`Carstay.toml`): after every sync, write the versions
//! actually provisioned — bundled tool releases ([`crate::tools::versions`]) and companion-repo
//! revisions ([`super::stainless::clone_revisions`]) — into a VCS-tracked file at the project
//! root. It is `Cargo.lock`'s job for the dependencies Cargo can't see: commit it when things
//! work, and when an upstream release breaks something, `git log Carstay.toml` names the
//! last-known-good set. (Restoring TO a recorded version is future work; this is the record.)
//!
//! Lives in `drivers` because it spans two provisioning domains — `tools` (a layer below, so it
//! can't own the composition) and the sibling [`super::stainless`] — and the `install-stainless`
//! command that triggers it stays a thin sequence of calls.

use std::path::Path;

/// The manifest's filename — one const to change if the name ever sails elsewhere.
const FILENAME: &str = "Carstay.toml";

/// The recorded versions, parsed back — the input side of `--use-stable-carstay` mode. `None` when the
/// manifest is missing or unreadable (a machine that never synced has nothing to restore to).
pub fn recorded() -> Option<Recorded> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(FILENAME);
    parse(&std::fs::read_to_string(path).ok()?)
}

/// What [`recorded`] hands the syncs: `(name, version)` pins per section. `"system"` entries
/// are dropped on parse — they mean nothing was bundled when recorded, so there is nothing
/// to pin.
pub struct Recorded {
    pub tools: Vec<(String, String)>,
    pub repos: Vec<(String, String)>,
}

/// Pure: [`render`]'s inverse (minus the `"system"` placeholders).
fn parse(text: &str) -> Option<Recorded> {
    let value: toml::Value = toml::from_str(text).ok()?;
    let section = |name: &str| -> Vec<(String, String)> {
        value
            .get(name)
            .and_then(|section| section.as_table())
            .map(|table| {
                table
                    .iter()
                    .filter_map(|(key, version)| {
                        let version = version.as_str()?;
                        (version != "system").then(|| (key.clone(), version.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    Some(Recorded { tools: section("tools"), repos: section("stainless_comfy") })
}

/// The blue heads-up a normal sync prints after fetching anything new: name the revert path
/// while the "it worked yesterday" memory is fresh, so a bad upstream release never strands
/// the user hunting for the recovery command.
pub fn stability_revert_notice() {
    let advice = format!(
        "New tool/repo versions were fetched. If they misbehave, revert to the last stable set:\n\
         reset {FILENAME} to the repo's committed version (git checkout -- {FILENAME}), then re-run\n\
         ./COMPILE.sh --use-stable-carstay   (or: bashrs_compile --use-stable-carstay)"
    );
    println!("{}", crate::support::doc_style::notice(&advice));
}

/// SIDE EFFECTS — `install-stainless` runs this after every sync: gather all sections and
/// (re)write the manifest in one move. Skipped when the content is unchanged, so an untouched
/// sync keeps the file's mtime and stays quiet in VCS.
pub fn record() {
    let content = render(&crate::tools::versions(), &super::stainless::clone_revisions());
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(FILENAME);
    if std::fs::read_to_string(&path).is_ok_and(|old| old == content) {
        return;
    }
    match std::fs::write(&path, &content) {
        Ok(()) => println!("carstay: recorded the provisioned versions in {FILENAME}"),
        Err(err) => eprintln!("carstay: could not write {}: {err}", path.display()),
    }
}

/// Pure: the manifest text. A `None` version renders as `"system"` — nothing is bundled and the
/// machine's own installation serves that tool, so there is no version to pin, but the situation
/// itself is worth recording.
fn render(tools: &[(&str, Option<String>)], repos: &[(&str, Option<String>)]) -> String {
    let mut out = String::from(
        "# Non-Cargo dependencies as provisioned on this machine — rewritten by `bashrs install-stainless`\n\
         # (COMPILE.sh / bashrs_compile) after every sync, all sections in one move. Commit it\n\
         # alongside code: when an upstream release breaks something, `git log Carstay.toml`\n\
         # names the last-known-good versions — and `./COMPILE.sh --use-stable-carstay` provisions THESE\n\
         # recorded versions instead of the latest releases (the file becomes the input then,\n\
         # and is not rewritten). Values are release tags (ffmpeg: its release channel —\n\
         # same-channel rebuilds share one id); repos record their clone's commit; \"system\"\n\
         # means the machine provides the tool itself, so nothing was bundled.\n",
    );
    let mut section = |name: &str, rows: &[(&str, Option<String>)]| {
        out.push_str(&format!("\n[{name}]\n"));
        for (key, version) in rows {
            out.push_str(&format!("{key} = \"{}\"\n", version.as_deref().unwrap_or("system")));
        }
    };
    section("tools", tools);
    section("stainless_comfy", repos);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_renders_both_sections_with_system_for_unbundled_tools() {
        let tools = [("ffmpeg", Some("n8.1".to_string())), ("yt-dlp", None)];
        let repos = [("contAInerized", Some("1a2b3c".to_string()))];
        let text = render(&tools, &repos);
        assert!(text.starts_with('#'), "leads with the explanation of what this file is");
        let tools_at = text.find("[tools]").expect("tools section");
        let repos_at = text.find("[stainless_comfy]").expect("comfy section");
        assert!(tools_at < repos_at, "sections in the declared order");
        assert!(text.contains("ffmpeg = \"n8.1\""), "{text}");
        assert!(text.contains("yt-dlp = \"system\""), "unbundled tools are recorded, not omitted");
        assert!(text.contains("contAInerized = \"1a2b3c\""), "{text}");
    }

    #[test]
    fn the_manifest_parses_back_for_carstay_mode_dropping_system_entries() {
        let tools = [("ffmpeg", Some("n8.1".to_string())), ("yt-dlp", None)];
        let repos = [("contAInerized", Some("1a2b3c".to_string()))];
        let recorded = parse(&render(&tools, &repos)).expect("round-trips");
        assert_eq!(
            recorded.tools,
            [("ffmpeg".to_string(), "n8.1".to_string())],
            "a \"system\" entry pins nothing"
        );
        assert_eq!(recorded.repos, [("contAInerized".to_string(), "1a2b3c".to_string())]);
        assert!(parse("not [ toml").is_none(), "garbage reads as nothing-to-restore");
    }

    #[test]
    fn the_manifest_is_valid_toml() {
        let tools = [("ffmpeg", Some("n8.1".to_string()))];
        let repos = [("contAInerized", None)];
        let parsed: Result<toml::Value, _> = toml::from_str(&render(&tools, &repos));
        let parsed = parsed.expect("parses as TOML");
        assert_eq!(parsed["tools"]["ffmpeg"].as_str(), Some("n8.1"));
        assert_eq!(parsed["stainless_comfy"]["contAInerized"].as_str(), Some("system"));
    }
}
