//! Writes out the generated command regions — the styled-echo matrix in
//! `src/categories/autogen_styles.rs` and the `g`/`gg` search families in
//! `src/categories/autogen_lookup.rs`. The regions themselves are *rendered* by
//! `generator_basis::style_matrix`/`family_matrix` (next to the data they draw on); this script is
//! just the file I/O — splice each region in place, and only when it would actually change.
//!
//! `rerun-if-changed` scopes that to edits of the data files, this script, or the generated files —
//! so ordinary builds don't re-run it. `build.rs` can't link the crate, so it pulls the data files
//! in textually via `include!`, inside a `mod support { … }` mirror of the crate layout: that's
//! what lets `generator_basis`'s `use crate::support::theme::…` resolve in the build script exactly
//! as in the crate. (`build.rs` no longer names anything from `theme` itself — only the mirror needs
//! it, so `generator_basis` can be `include!`d.)

use std::{env, fs, path::Path};

mod support {
    pub mod theme {
        include!("src/support/theme.rs");
    }
    pub mod generator_basis {
        include!("src/support/generator_basis.rs");
    }
}

use crate::support::generator_basis::{family_matrix, style_matrix, G_FAMILY, GG_FAMILY};

const THEME: &str = "src/support/theme.rs";
const BASIS: &str = "src/support/generator_basis.rs";

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
    // Re-run only when a data file, a generated file, or this script changes.
    for file in [THEME, BASIS, AUTOGEN_STYLES, AUTOGEN_LOOKUP, "build.rs"] {
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
