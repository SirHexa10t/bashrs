//! Enforces the crate's layering diagram (see `lib.rs`): imports must only point left in
//!
//! ```text
//! support  <  conf  <  tools  <  drivers  <  categories, cli
//! ```
//!
//! A structural rule as a test, so an upward import (the kind that once threatened a
//! `conf ↔ tools` loop) fails the suite instead of silently eroding the architecture.

use std::path::Path;

/// Each layer's directory (relative to `src/`) paired with the crate layers it may import.
/// A module may always import itself; `support` is the bottom.
const ALLOWED: &[(&str, &[&str])] = &[
    ("support", &["support"]),
    ("conf", &["conf", "support"]),
    ("tools", &["tools", "conf", "support"]),
    ("drivers", &["drivers", "tools", "conf", "support"]),
    ("categories", &["categories", "conf", "support", "tools", "drivers"]),
    // The top of the diagram: parse/dispatch (`mod.rs`) + the sourcefile generator
    // (`sourcefile.rs`) may reach every layer.
    ("cli", &["cli", "categories", "conf", "support", "tools", "drivers"]),
];

/// What the crate root's own files (`lib.rs`, `main.rs`) may name. They are the top of
/// the diagram, so the list is everything — but they are still *scanned*, so the top of the
/// diagram isn't the one place the rule goes unchecked. `elevation` is `lib.rs`'s alias for
/// the elevation module (the `#[elevated]` macro's anchor), not a layer.
const ROOT_MAY_IMPORT: &[&str] =
    &["categories", "cli", "conf", "drivers", "support", "tools", "elevation"];

#[test]
fn imports_only_point_left() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for &(layer, allowed) in ALLOWED {
        for file in rust_files(&src.join(layer)) {
            assert_imports(&file, layer, allowed);
        }
    }
    for file in root_files(&src) {
        assert_imports(&file, "the crate root", ROOT_MAY_IMPORT);
    }
}

/// Every layer directory under `src/` must appear in [`ALLOWED`]. Without this, adding a new
/// subsystem directory would silently exempt it from the rule this file exists to enforce — the
/// one failure mode a per-layer loop can't catch, because it only looks where it's told.
#[test]
fn every_layer_directory_is_covered_by_the_rule_table() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for entry in std::fs::read_dir(&src).unwrap().flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(
            ALLOWED.iter().any(|&(layer, _)| layer == name),
            "`src/{name}/` has no entry in ALLOWED — give the new layer its permitted imports (and \
             a place in lib.rs's diagram), so it isn't exempt from the layering rule",
        );
    }
}

/// Assert every `crate::…` edge in `file` points at something `allowed`.
fn assert_imports(file: &Path, layer: &str, allowed: &[&str]) {
    let text = std::fs::read_to_string(file).unwrap();
    for target in crate_imports(&text) {
        assert!(
            allowed.contains(&target.as_str()),
            "{} imports `crate::{target}` — `{layer}` may only import {allowed:?} (see lib.rs's layering diagram)",
            file.display(),
        );
    }
}

/// The `.rs` files sitting directly in `src/` — the crate root's own, which belong to no layer
/// directory and so are missed by [`rust_files`]'s per-layer walk.
fn root_files(src: &Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(src) else { return Vec::new() };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "rs"))
        .collect()
}

/// `build.rs` reaches four source files by string path — two `include!`d as a mirror of the crate
/// layout, two spliced in place — and the crate itself references none of them, so a rename shows
/// up as a build-script failure naming `build.rs` rather than the file that actually moved. The
/// paths are read back out of `build.rs` here (not restated) so this guard can't drift from it.
#[test]
fn the_source_paths_build_rs_names_all_exist() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script = std::fs::read_to_string(root.join("build.rs")).unwrap();
    // Odd-indexed pieces of a split on `"` are the quoted string literals.
    let named: Vec<&str> = script
        .split('"')
        .skip(1)
        .step_by(2)
        .filter(|literal| literal.starts_with("src/") && literal.ends_with(".rs"))
        .collect();
    for path in &named {
        assert!(
            root.join(path).is_file(),
            "build.rs names `{path}`, which no longer exists — build.rs reaches it by string path, \
             so nothing else in the crate will point you here",
        );
    }
    assert!(named.len() >= 4, "expected build.rs to still name its source paths; found {named:?}");
}

/// Every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return files };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(rust_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
    files
}

/// The first path segment of every `crate::…` reference in `text`'s *code* — `use` statements and
/// inline paths alike (an inline `crate::tools::resolve(…)` is as much an edge as a `use`).
/// Comments are skipped, trailing ones included: doc links like ``[`crate::cli`]`` may point
/// anywhere. (Truncating at `//` also cuts `https://…` string contents — harmless, since URLs
/// never contain `crate::`.)
fn crate_imports(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .flat_map(|code| code.split("crate::").skip(1))
        .map(|rest| {
            rest.chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect::<String>()
        })
        .filter(|segment| !segment.is_empty())
        .collect()
}

#[test]
fn the_scanner_reads_code_edges_and_skips_comments() {
    let sample = "\
//! doc link: [`crate::cli`]\n\
use crate::tools::resolve; // trailing note: crate::categories\n\
let _ = crate::conf::bashrs_home();\n";
    assert_eq!(crate_imports(sample), ["tools", "conf"]);
}
