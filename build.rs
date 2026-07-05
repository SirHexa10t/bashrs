//! Regenerates the styled-echo command matrix in `src/categories/autogen_styles.rs` from
//! the vocabulary in `src/categories/style_vocab.rs`, as part of the build.
//!
//! Only the text between the `GENERATED-STYLE-MATRIX` markers is rewritten, and only when
//! it would actually change. `rerun-if-changed` scopes this to edits of the vocabulary,
//! this script, or the generated file — so ordinary builds don't re-run it, and there's no
//! per-build cost. The vocabulary is the single source of truth: `build.rs` can't link the
//! crate, so it pulls the same file in textually via `include!`.

use std::{env, fs, path::Path};

include!("src/categories/style_vocab.rs");

const AUTOGEN: &str = "src/categories/autogen_styles.rs";
const VOCAB: &str = "src/categories/style_vocab.rs";
const START: &str = "    // GENERATED-STYLE-MATRIX-START";
const END: &str = "    // GENERATED-STYLE-MATRIX-END";

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

fn main() {
    // Re-run only when the vocabulary, this script, or the generated file changes.
    println!("cargo:rerun-if-changed={VOCAB}");
    println!("cargo:rerun-if-changed={AUTOGEN}");
    println!("cargo:rerun-if-changed=build.rs");

    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo");
    let autogen = Path::new(&manifest).join(AUTOGEN);
    let current = fs::read_to_string(&autogen).expect("read autogen_styles.rs");
    let want = expected(&current);
    if want != current {
        fs::write(&autogen, want).expect("write autogen_styles.rs");
    }
}
