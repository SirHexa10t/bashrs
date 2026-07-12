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
];

#[test]
fn imports_only_point_left() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for &(layer, allowed) in ALLOWED {
        for file in rust_files(&src.join(layer)) {
            let text = std::fs::read_to_string(&file).unwrap();
            for target in crate_imports(&text) {
                assert!(
                    allowed.contains(&target.as_str()),
                    "{} imports `crate::{target}` — `{layer}` may only import {allowed:?} (see lib.rs's layering diagram)",
                    file.display(),
                );
            }
        }
    }
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
