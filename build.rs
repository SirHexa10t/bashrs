//! Regenerates the styled-echo command matrix in `src/categories/autogen_styles.rs` from
//! the vocabulary in `src/support/style_vocab.rs`, as part of the build.
//!
//! Only the text between the `GENERATED-STYLE-MATRIX` markers is rewritten, and only when
//! it would actually change. `rerun-if-changed` scopes this to edits of the vocabulary,
//! this script, or the generated file — so ordinary builds don't re-run it, and there's no
//! per-build cost. The vocabulary is the single source of truth: `build.rs` can't link the
//! crate, so it pulls the same file in textually via `include!`.

use std::{env, fs, path::Path};

include!("src/support/style_vocab.rs");
include!("src/support/lookup_vocab.rs");
include!("src/support/treegrep_vocab.rs");

const AUTOGEN: &str = "src/categories/autogen_styles.rs";
const VOCAB: &str = "src/support/style_vocab.rs";
const START: &str = "    // GENERATED-STYLE-MATRIX-START";
const END: &str = "    // GENERATED-STYLE-MATRIX-END";

const AUTOGEN_LOOKUP: &str = "src/categories/autogen_lookup.rs";
const LOOKUP_VOCAB: &str = "src/support/lookup_vocab.rs";
const LOOKUP_START: &str = "    // GENERATED-LOOKUP-GREP-START";
const LOOKUP_END: &str = "    // GENERATED-LOOKUP-GREP-END";

const AUTOGEN_TREEGREP: &str = "src/categories/autogen_treegrep.rs";
const TREEGREP_VOCAB: &str = "src/support/treegrep_vocab.rs";
const TREEGREP_START: &str = "    // GENERATED-TREEGREP-START";
const TREEGREP_END: &str = "    // GENERATED-TREEGREP-END";

/// A criterion key contributes to a function name as itself — except bold, which is silent
/// (so `["bo","","r"]` is `recho`, not `borecho`).
fn name_part(key: &str) -> &str {
    if key == "bo" { "" } else { key }
}

/// The generated region: one `pub fn` per weight × underline × color.
fn matrix() -> String {
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

/// `autogen_styles.rs` with its generated region replaced by the current matrix.
fn expected(current: &str) -> String {
    let start = current.find(START).expect("START marker missing from autogen_styles.rs") + START.len();
    let end = current.find(END).expect("END marker missing from autogen_styles.rs");
    // A blank line sets the generated functions off from the START marker.
    format!("{}\n\n{}\n{}", &current[..start], matrix(), &current[end..])
}

/// The generated grep region: one bare `pub fn` per context size in [`CONTEXTS`] (`g`, `g3`, …).
fn lookup_matrix() -> String {
    CONTEXTS
        .iter()
        .map(|&ctx| {
            let name = if ctx == 0 { "g".to_string() } else { format!("g{ctx}") };
            let desc = if ctx == 0 {
                "Case-insensitive literal search (no regex), colouring matches".to_string()
            } else {
                format!("Case-insensitive literal search, showing {ctx} lines of context around each match")
            };
            format!(
                "    /// {desc}\n    #[unprefixed]\n    pub fn {name}(args: GrepArgs) {{ _grep(&args.pattern, {ctx}, args.source.as_deref(), args.line_number); }}"
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// `autogen_lookup.rs` with its generated region replaced by the current grep family.
fn expected_lookup(current: &str) -> String {
    let start = current.find(LOOKUP_START).expect("START marker missing from autogen_lookup.rs") + LOOKUP_START.len();
    let end = current.find(LOOKUP_END).expect("END marker missing from autogen_lookup.rs");
    format!("{}\n\n{}\n{}", &current[..start], lookup_matrix(), &current[end..])
}

/// The generated `gg`-family: one bare `pub fn` per context size in [`GG_CONTEXTS`] (`gg`, `gg3`, …).
fn treegrep_matrix() -> String {
    GG_CONTEXTS
        .iter()
        .map(|&ctx| {
            let name = if ctx == 0 { "gg".to_string() } else { format!("gg{ctx}") };
            let desc = if ctx == 0 {
                "Recursively search a directory for expression(s) — filenames, then file contents".to_string()
            } else {
                format!("Recursive search, showing {ctx} lines of context around each file-content match")
            };
            format!(
                "    /// {desc}\n    #[unprefixed]\n    #[trailing_newline]\n    pub fn {name}(args: GgArgs) {{ _gg(args, {ctx}); }}"
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// `autogen_treegrep.rs` with its generated region replaced by the current `gg` family.
fn expected_treegrep(current: &str) -> String {
    let start = current.find(TREEGREP_START).expect("START marker missing from autogen_treegrep.rs") + TREEGREP_START.len();
    let end = current.find(TREEGREP_END).expect("END marker missing from autogen_treegrep.rs");
    format!("{}\n\n{}\n{}", &current[..start], treegrep_matrix(), &current[end..])
}

/// Rewrite `file`'s generated region (computed by `splice`) in place, only when it changes.
fn regenerate(manifest: &str, file: &str, splice: impl Fn(&str) -> String) {
    let path = Path::new(manifest).join(file);
    let current = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {file}: {e}"));
    let want = splice(&current);
    if want != current {
        fs::write(&path, want).unwrap_or_else(|e| panic!("write {file}: {e}"));
    }
}

fn main() {
    // Re-run only when a vocabulary, a generated file, or this script changes.
    for file in [VOCAB, AUTOGEN, LOOKUP_VOCAB, AUTOGEN_LOOKUP, TREEGREP_VOCAB, AUTOGEN_TREEGREP, "build.rs"] {
        println!("cargo:rerun-if-changed={file}");
    }

    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo");
    regenerate(&manifest, AUTOGEN, expected);
    regenerate(&manifest, AUTOGEN_LOOKUP, expected_lookup);
    regenerate(&manifest, AUTOGEN_TREEGREP, expected_treegrep);
}
