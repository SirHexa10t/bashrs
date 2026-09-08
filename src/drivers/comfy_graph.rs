//! My own crates, and which of them lean on which — read out of the resolved dependency record.
//!
//! The sibling of [`super::carstay`]: that one owns the NON-cargo record (tools and clones, in
//! `Carstay.toml`), this one reads the cargo record. `Cargo.lock` is the right source because it
//! is the *resolved* graph — what actually got built, not what the manifests asked for — and
//! because it is committed, so the answer is the same for anyone with this checkout.
//!
//! It is baked in at compile time. An installed bashrs has no source tree beside it, and a map
//! that only worked from the repo would be a map that never worked when wanted.
//!
//! ## What counts as mine
//!
//! Not the name on the repository — the WAY the crate is carried. A registry crate has a
//! `source` starting `registry+`; everything else here is either sourceless (this crate and its
//! workspace members, which cargo does not have to fetch) or `git+` (the companions). So the
//! rule is "not from a registry", stated as such rather than as a claim about authorship: a
//! third-party crate pulled straight from git would appear here too, and would deserve to,
//! being carried exactly the same way and pinned by exactly the same lockfile line.

use std::collections::{BTreeMap, BTreeSet};

use crate::support::align::align_columns;
use crate::support::doc_style::hued;

/// The lockfile, as it stood when this binary was built.
const LOCK: &str = include_str!("../../Cargo.lock");

/// One crate of ours: its name, and which of the others it uses directly.
#[derive(Debug, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    /// Names of our OTHER crates it depends on, in lockfile order. Registry dependencies are not
    /// listed — the map is about how my own work fits together, and `clap` says nothing about it.
    pub uses: Vec<String>,
}

/// The map of this build's own crates, drawn from `root` downwards.
///
/// Two views, because they answer different questions. The tree shows the SHAPE — what reaches
/// what, and how deep. The tail lists every crate more than one of ours depends on, which is the
/// question a shared crate actually raises: change it, and who has to be rebuilt and re-checked.
#[must_use]
pub fn map() -> Vec<String> {
    let nodes = own_crates(LOCK);
    let Some(root) = nodes.iter().find(|node| node.name == env!("CARGO_PKG_NAME")) else {
        return vec![format!("{}: not in its own lockfile", env!("CARGO_PKG_NAME"))];
    };
    let mut lines = draw(&nodes, &root.name);
    let shared = shared_by_several(&nodes);
    if !shared.is_empty() {
        lines.push(String::new());
        lines.push("used by more than one:".to_string());
        lines.extend(align_columns(shared, &["← "]));
    }
    lines
}

/// Every crate the lockfile carries outside a registry, with the edges among them. Names only:
/// a lockfile dependency may be written `"name"`, `"name version"` or `"name version (source)"`
/// when it needs disambiguating, and only the first word is the name.
#[must_use]
pub fn own_crates(lock: &str) -> Vec<Node> {
    // `toml::from_str`, as everywhere else in the crate — `str::parse` goes through a `FromStr`
    // that this version of the crate does not read a document with.
    let Ok(value) = toml::from_str::<toml::Value>(lock) else {
        return Vec::new();
    };
    let packages: Vec<&toml::value::Table> = value
        .get("package")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_table)
        .collect();

    let ours: BTreeSet<&str> = packages
        .iter()
        .filter(|package| {
            !package.get("source").and_then(toml::Value::as_str).is_some_and(|source| {
                source.starts_with("registry+")
            })
        })
        .filter_map(|package| package.get("name").and_then(toml::Value::as_str))
        .collect();

    packages
        .iter()
        .filter_map(|package| {
            let name = package.get("name").and_then(toml::Value::as_str)?;
            ours.contains(name).then(|| Node {
                name: name.to_string(),
                uses: package
                    .get("dependencies")
                    .and_then(toml::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(toml::Value::as_str)
                    .filter_map(|dep| dep.split_whitespace().next())
                    .filter(|dep| ours.contains(dep))
                    .map(str::to_string)
                    .collect(),
            })
        })
        .collect()
}

/// The tree under `root`, drawn with box-drawing rules, every name in its own colour.
///
/// The colour comes from the name itself ([`hued`]), so one crate is one colour wherever it is
/// reached — which is what lets the eye pair `software_inventory` under `bashrs` with the one
/// under `vidl` without reading either.
///
/// A crate reached twice is drawn twice — that IS the sharing, seen where it happens — but its
/// subtree is only expanded once, marked so a reader knows the branch continues elsewhere rather
/// than ending. The visited set doubles as the cycle guard: cargo will not resolve one, but a
/// hand-edited lockfile could, and a stack overflow is a poor way to report it.
#[must_use]
pub fn draw(nodes: &[Node], root: &str) -> Vec<String> {
    let by_name: BTreeMap<&str, &Node> =
        nodes.iter().map(|node| (node.name.as_str(), node)).collect();
    let mut lines = vec![hued(root)];
    let mut expanded = BTreeSet::new();
    expanded.insert(root);
    _branch(root, &by_name, &mut expanded, "", &mut lines);
    lines
}

/// One level of [`draw`], appending to `lines`. `stem` is what every child line begins with —
/// the rules already drawn by the ancestors above it.
fn _branch<'a>(
    name: &str,
    by_name: &BTreeMap<&'a str, &'a Node>,
    expanded: &mut BTreeSet<&'a str>,
    stem: &str,
    lines: &mut Vec<String>,
) {
    let Some(node) = by_name.get(name) else { return };
    for (index, child) in node.uses.iter().enumerate() {
        let last = index + 1 == node.uses.len();
        let (elbow, next_stem) = match last {
            true => ("└─ ", format!("{stem}   ")),
            false => ("├─ ", format!("{stem}│  ")),
        };
        let Some(&reached) = by_name.get_key_value(child.as_str()).map(|(name, _)| name) else {
            continue;
        };
        let has_children = by_name.get(reached).is_some_and(|node| !node.uses.is_empty());
        let again = !expanded.insert(reached);
        // Only a crate with a subtree needs saying twice-over; a leaf drawn twice IS the whole
        // truth about it, and a marker would be noise.
        let note = if again && has_children { "  ·expanded above" } else { "" };
        lines.push(format!("{stem}{elbow}{}{note}", hued(reached)));
        if !again {
            _branch(reached, by_name, expanded, &next_stem, lines);
        }
    }
}

/// `name  ← user, user` for every crate more than one of ours uses, in name order.
#[must_use]
pub fn shared_by_several(nodes: &[Node]) -> Vec<String> {
    let mut users: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for node in nodes {
        for used in &node.uses {
            users.entry(used).or_default().push(&node.name);
        }
    }
    users
        .into_iter()
        .filter(|(_, users)| users.len() > 1)
        .map(|(used, users)| {
            let users: Vec<String> = users.into_iter().map(hued).collect();
            format!("{}  ← {}", hued(used), users.join(", "))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The drawing colours every name ([`hued`]); the shape is what these assertions are about,
    /// so they read it plainly. One test below checks the colour is really there.
    fn plain(lines: Vec<String>) -> Vec<String> {
        lines.iter().map(|line| console::strip_ansi_codes(line).into_owned()).collect()
    }

    const SAMPLE: &str = r#"
        [[package]]
        name = "root_crate"
        version = "0.1.0"
        dependencies = ["clap", "helper", "shared", "sibling"]

        [[package]]
        name = "clap"
        version = "4.0.0"
        source = "registry+https://github.com/rust-lang/crates.io-index"

        [[package]]
        name = "helper"
        version = "0.1.0"

        [[package]]
        name = "sibling"
        version = "0.1.0"
        source = "git+https://example.invalid/sibling.git?branch=main#abc"
        dependencies = ["clap", "shared"]

        [[package]]
        name = "shared"
        version = "0.1.0"
        source = "git+https://example.invalid/shared.git?branch=main#def"
    "#;

    /// Ours is decided by HOW a crate is carried, not by who wrote it: registry crates are out,
    /// the sourceless root and its path members are in, git checkouts are in. And a dependency
    /// on something outside that set is not an edge on this map.
    #[test]
    fn a_registry_crate_is_not_ours_and_neither_are_its_edges() {
        let nodes = own_crates(SAMPLE);
        let names: Vec<&str> = nodes.iter().map(|node| node.name.as_str()).collect();
        assert_eq!(names, ["root_crate", "helper", "sibling", "shared"], "clap is not ours");

        let root = nodes.iter().find(|node| node.name == "root_crate").expect("the root");
        assert_eq!(root.uses, ["helper", "shared", "sibling"], "clap is not an edge either");
        let sibling = nodes.iter().find(|node| node.name == "sibling").expect("a companion");
        assert_eq!(sibling.uses, ["shared"]);
    }

    /// A lockfile may disambiguate a dependency by version or by source; only the first word of
    /// such an entry is the crate's name.
    #[test]
    fn a_qualified_dependency_entry_is_read_by_name() {
        let nodes = own_crates(
            r#"
            [[package]]
            name = "a"
            dependencies = ["b 1.2.3", "c 0.1.0 (git+https://example.invalid/c.git)"]
            [[package]]
            name = "b"
            [[package]]
            name = "c"
        "#,
        );
        assert_eq!(nodes[0].uses, ["b", "c"]);
    }

    /// The tree shows shape, and shows a shared crate at every place it is reached — which is
    /// what makes the sharing visible rather than merely recorded.
    #[test]
    fn the_tree_shows_a_shared_crate_wherever_it_is_reached() {
        let drawn = plain(draw(&own_crates(SAMPLE), "root_crate"));
        assert_eq!(
            drawn,
            [
                "root_crate",
                "├─ helper",
                "├─ shared",
                "└─ sibling",
                "   └─ shared",
            ],
            "{drawn:#?}"
        );
    }

    /// A crate with a subtree, reached a second time, says so instead of drawing the branch
    /// twice — but a leaf reached twice needs no such apology.
    #[test]
    fn a_repeated_subtree_is_marked_rather_than_redrawn() {
        let nodes = own_crates(
            r#"
            [[package]]
            name = "root"
            dependencies = ["mid", "other"]
            [[package]]
            name = "other"
            dependencies = ["mid"]
            [[package]]
            name = "mid"
            dependencies = ["leaf"]
            [[package]]
            name = "leaf"
        "#,
        );
        assert_eq!(
            plain(draw(&nodes, "root")),
            [
                "root",
                "├─ mid",
                "│  └─ leaf",
                "└─ other",
                "   └─ mid  ·expanded above",
            ]
        );
    }

    /// A hand-edited lockfile could name a cycle. Cargo will not resolve one, but a stack
    /// overflow is a poor way to report the fact.
    #[test]
    fn a_cycle_terminates_instead_of_recursing_forever() {
        let nodes = own_crates(
            r#"
            [[package]]
            name = "a"
            dependencies = ["b"]
            [[package]]
            name = "b"
            dependencies = ["a"]
        "#,
        );
        assert_eq!(plain(draw(&nodes, "a")), ["a", "└─ b", "   └─ a  ·expanded above"]);
    }

    /// The question a shared crate raises is "who else breaks if this moves", so the tail names
    /// them — and stays quiet about anything only one crate uses.
    #[test]
    fn the_tail_names_every_user_of_a_shared_crate() {
        let shared = plain(shared_by_several(&own_crates(SAMPLE)));
        assert_eq!(shared, ["shared  ← root_crate, sibling"]);
        assert!(
            !shared.iter().any(|line| line.starts_with("helper")),
            "one user is not sharing: {shared:?}"
        );
    }

    /// A name wears the same colour everywhere it is drawn — under `bashrs` and under `vidl`
    /// alike — which is the whole reason to colour it: the eye pairs the two without reading
    /// either. And two different names do not come out the same.
    #[test]
    fn one_name_is_one_colour_wherever_it_appears() {
        let drawn = draw(&own_crates(SAMPLE), "root_crate");
        let coloured = |want: &str| -> Vec<&String> {
            drawn.iter().filter(|line| console::strip_ansi_codes(line).contains(want)).collect()
        };
        let paint = |line: &str| -> String {
            line.split('m').next().unwrap_or_default().to_string() + "m"
        };

        let shared = coloured("shared");
        assert_eq!(shared.len(), 2, "reached twice: {shared:?}");
        assert!(shared[0].contains("\x1b[38;2;"), "and coloured: {:?}", shared[0]);
        let colours: Vec<String> = shared.iter().map(|line| paint(line)).collect();
        assert_ne!(colours[0], colours[1], "the stems differ, so compare the ESCAPES");

        // The escape itself, wherever in the line it sits, is the same on both.
        let escape_of = |line: &str| {
            let at = line.find("\x1b[38;2;").expect("coloured");
            line[at..line[at..].find('m').expect("terminated") + at + 1].to_string()
        };
        assert_eq!(escape_of(shared[0]), escape_of(shared[1]), "one name, one colour");
        let helper = coloured("helper");
        assert_ne!(escape_of(helper[0]), escape_of(shared[0]), "different names, different colour");
    }

    /// The real thing: this build's own lockfile, which must describe this build.
    #[test]
    fn our_own_lockfile_maps_this_crate() {
        let nodes = own_crates(LOCK);
        let names: Vec<&str> = nodes.iter().map(|node| node.name.as_str()).collect();
        assert!(names.contains(&env!("CARGO_PKG_NAME")), "we are not in our own map: {names:?}");
        assert!(names.len() > 1, "a map of one crate is not a map");
        assert!(!names.contains(&"clap"), "a registry crate leaked in: {names:?}");

        let drawn = plain(map());
        assert_eq!(drawn.first().map(String::as_str), Some(env!("CARGO_PKG_NAME")));
        assert!(drawn.len() > 2, "{drawn:#?}");
    }
}
