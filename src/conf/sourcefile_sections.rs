//! The sourcefile's sections — each category's commands, then the file's own blocks — in the order
//! both shells' files write them, each under a header line naming it; and finding them again in a
//! file as written. `brs_sourcefile` takes a flag per section from here. The generator
//! ([`crate::cli`]) writes the headers, and a test of its holds the two to each other: a section
//! added, moved or renamed there fails it until this list says the same.

use std::ops::Range;

/// Every section's label — what its header says first — in the order the files write them: the
/// categories', as the generator's table names them, then the file's own blocks. The stainless
/// one is written only when there are stainless repos to alias.
pub const SECTIONS: &[&str] = &[
    "bashrs",
    "overrides",
    "filesystem",
    "git",
    "download",
    "exposed_helpers",
    "media",
    "gaming",
    "network",
    "packages",
    "processes",
    "project",
    "python",
    "shell",
    "terminal",
    "lookup",
    "style",
    "comfy_repos",
    "stainless comfy repos",
    "bundled tools",
    "environment",
    "keybinds",
    "completion",
    "greeting",
];

/// A section's label as a flag's name: `_` and spaces as `-` — `exposed_helpers` is
/// `--exposed-helpers`.
pub fn flag(label: &str) -> String {
    label.replace(['_', ' '], "-")
}

/// The sections `text` — a sourcefile as written — holds, each with where it is: from its header
/// to the next section's, the last to the end; in the file's order. A header is a line of its
/// own after an empty one, as the generator writes each: `# label`, alone or saying more after a
/// comma or a parenthesis (`# completion (bash only)`, `# greeting, reminders — …`).
pub fn find(text: &str) -> Vec<(&'static str, Range<usize>)> {
    let mut starts: Vec<(&'static str, usize)> = Vec::new();
    let (mut offset, mut after_empty) = (0, false);
    for line in text.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        if after_empty {
            if let Some(label) = SECTIONS.iter().find(|label| heads(bare, label)) {
                starts.push((label, offset));
            }
        }
        after_empty = bare.is_empty();
        offset += line.len();
    }
    let ends = starts.iter().skip(1).map(|(_, start)| *start).chain([text.len()]);
    starts.iter().zip(ends).map(|((label, start), end)| (*label, *start..end)).collect()
}

/// Whether `line` heads the section `label`.
fn heads(line: &str, label: &str) -> bool {
    line.strip_prefix("# ")
        .and_then(|rest| rest.strip_prefix(label))
        .is_some_and(|more| more.is_empty() || more.starts_with(',') || more.starts_with(" ("))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file as the generator writes one: a preamble, then sections, each header after an
    /// empty line — a comment inside a section never heads one.
    const FILE: &str = "#!/usr/bin/env bash\n# Auto-generated\n\
        \n# git\nalias gs='git status'\n\
        \n# git tips: none\nalias gl='git log'\n\
        \n# lookup\n# shell\nalias G='grep'\n\
        \n# completion (bash only)\ncomplete -F _x x\n\
        \n# greeting, reminders — and more\necho hi\n";

    fn found(text: &str) -> Vec<(&str, &str)> {
        find(text).into_iter().map(|(label, at)| (label, &text[at])).collect()
    }

    /// Each section runs from its header to the next one's, the last to the end; a header says
    /// its label alone, or more after a comma or a parenthesis — and only after an empty line.
    #[test]
    fn sections_run_from_header_to_header() {
        assert_eq!(
            found(FILE),
            [
                ("git", "# git\nalias gs='git status'\n\n# git tips: none\nalias gl='git log'\n\n"),
                ("lookup", "# lookup\n# shell\nalias G='grep'\n\n"),
                ("completion", "# completion (bash only)\ncomplete -F _x x\n\n"),
                ("greeting", "# greeting, reminders — and more\necho hi\n"),
            ]
        );
        assert!(found("#!/usr/bin/env fish\n").is_empty(), "a file with none");
    }

    /// A label's flag has `-` for its `_` and spaces; no two sections share a label, or a flag.
    #[test]
    fn every_section_has_a_flag_of_its_own() {
        assert_eq!(flag("exposed_helpers"), "exposed-helpers");
        assert_eq!(flag("stainless comfy repos"), "stainless-comfy-repos");
        let mut flags: Vec<String> = SECTIONS.iter().map(|label| flag(label)).collect();
        flags.sort();
        flags.dedup();
        assert_eq!(flags.len(), SECTIONS.len());
    }
}
