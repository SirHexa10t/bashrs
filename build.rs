//! Regenerates the generated command regions — the styled-echo matrix in
//! `src/categories/autogen_styles.rs` and the `g`/`gg` search families in
//! `src/categories/autogen_lookup.rs` — from the vocabulary in
//! `src/support/generative_constants.rs`, as part of the build.
//!
//! Only the text between each region's `GENERATED-*` markers is rewritten, and only when
//! it would actually change. `rerun-if-changed` scopes this to edits of the vocabulary,
//! this script, or the generated files — so ordinary builds don't re-run it, and there's no
//! per-build cost. The vocabulary is the single source of truth: `build.rs` can't link the
//! crate, so it pulls the same file in textually via `include!`.

use std::{env, fs, path::Path};

include!("src/support/generative_constants.rs");

const CONSTANTS: &str = "src/support/generative_constants.rs";

const AUTOGEN_STYLES: &str = "src/categories/autogen_styles.rs";
const STYLE_MARKERS: (&str, &str) =
    ("    // GENERATED-STYLE-MATRIX-START", "    // GENERATED-STYLE-MATRIX-END");

const AUTOGEN_LOOKUP: &str = "src/categories/autogen_lookup.rs";
// Two regions in the one file — the `g`-family and `gg`-family `#[category]` blocks — each
// spliced independently in `main`.
const GREP_MARKERS: (&str, &str) =
    ("    // GENERATED-LOOKUP-GREP-START", "    // GENERATED-LOOKUP-GREP-END");
const TREEGREP_MARKERS: (&str, &str) =
    ("    // GENERATED-TREEGREP-START", "    // GENERATED-TREEGREP-END");

/// A criterion key contributes to a function name as itself — except bold, which is silent
/// (so `["bo","","r"]` is `recho`, not `borecho`).
fn name_part(key: &str) -> &str {
    if key == "bo" { "" } else { key }
}

/// The generated style region: one `pub fn` per weight × underline × color.
fn style_matrix() -> String {
    let mut blocks = Vec::new();
    for (wk, _, wh) in WEIGHTS {
        for (uk, _, uh) in UNDERLINES {
            for (ck, _, ch) in COLORS {
                let stem = format!("{}{}{}", name_part(wk), name_part(uk), name_part(ck));
                let name = if stem.is_empty() { "bo".to_string() } else { stem };
                // A function's doc names only its non-default criteria — skip empty-key
                // entries so labels like underline's `unchanged` never leak into the doc.
                let desc = [(*wk, *wh), (*uk, *uh), (*ck, *ch)]
                    .into_iter()
                    .filter(|(k, _)| !k.is_empty())
                    .map(|(_, w)| w)
                    .collect::<Vec<_>>()
                    .join(" ");
                blocks.push(format!(
                    "    /// echo in {desc}\n    #[unprefixed]\n    #[alias(\"echo{name}\")]\n    pub fn {name}echo(args: EchoArgs) {{ _styled_echo([\"{wk}\", \"{uk}\", \"{ck}\"], &args); }}"
                ));
            }
        }
    }
    blocks.join("\n\n")
}

/// Render one search family (`G_FAMILY`/`GG_FAMILY`, defined with the rest of the vocabulary in
/// `generative_constants.rs`): a bare `pub fn` per context size, forwarding to its runner.
fn family_matrix(family: &SearchFamily) -> String {
    family
        .contexts
        .iter()
        .map(|&ctx| {
            let name =
                if ctx == 0 { family.stem.to_string() } else { format!("{}{ctx}", family.stem) };
            let desc = if ctx == 0 {
                family.bare_desc.to_string()
            } else {
                let (pre, post) = family.ctx_desc;
                format!("{pre}{ctx}{post}")
            };
            let call = if ctx == 0 {
                format!("{}(&args)", family.runner)
            } else {
                format!("{}(&{} {{ context: {ctx}, ..args }})", family.runner, family.args)
            };
            format!(
                "    /// {desc}\n    #[unprefixed]\n{}    pub fn {name}(args: {}) {{ {call}; }}",
                family.extra_attrs, family.args
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// `current` with the region between `markers` replaced by `generated` (a blank line sets the
/// generated code off from the START marker).
fn splice(current: &str, markers: (&str, &str), generated: &str, file: &str) -> String {
    let (start_marker, end_marker) = markers;
    let start = current
        .find(start_marker)
        .unwrap_or_else(|| panic!("START marker missing from {file}"))
        + start_marker.len();
    let end = current.find(end_marker).unwrap_or_else(|| panic!("END marker missing from {file}"));
    format!("{}\n\n{}\n{}", &current[..start], generated, &current[end..])
}

/// Rewrite `file`'s generated region (computed by `regen`) in place, only when it changes.
fn regenerate(manifest: &str, file: &str, regen: impl Fn(&str) -> String) {
    let path = Path::new(manifest).join(file);
    let current = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {file}: {e}"));
    let want = regen(&current);
    if want != current {
        fs::write(&path, want).unwrap_or_else(|e| panic!("write {file}: {e}"));
    }
}

fn main() {
    // Re-run only when a vocabulary, a generated file, or this script changes.
    for file in [CONSTANTS, AUTOGEN_STYLES, AUTOGEN_LOOKUP, "build.rs"] {
        println!("cargo:rerun-if-changed={file}");
    }

    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo");
    regenerate(&manifest, AUTOGEN_STYLES, |cur| {
        splice(cur, STYLE_MARKERS, &style_matrix(), AUTOGEN_STYLES)
    });
    regenerate(&manifest, AUTOGEN_LOOKUP, |cur| {
        splice(cur, GREP_MARKERS, &family_matrix(&G_FAMILY), AUTOGEN_LOOKUP)
    });
    regenerate(&manifest, AUTOGEN_LOOKUP, |cur| {
        splice(cur, TREEGREP_MARKERS, &family_matrix(&GG_FAMILY), AUTOGEN_LOOKUP)
    });
}
