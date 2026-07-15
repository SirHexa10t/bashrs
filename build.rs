//! Regenerates the generated command regions — the styled-echo matrix in
//! `src/categories/autogen_styles.rs` and the `g`/`gg` search families in
//! `src/categories/autogen_lookup.rs` — from the vocabulary in `src/support/theme.rs` and the
//! search families in `src/support/generator_basis.rs`, as part of the build.
//!
//! Only the text between each region's `GENERATED-*` markers is rewritten, and only when it would
//! actually change. `rerun-if-changed` scopes this to edits of those data files, this script, or
//! the generated files — so ordinary builds don't re-run it, and there's no per-build cost. Those
//! data files are the single source of truth: `build.rs` can't link the crate, so it pulls each in
//! textually via `include!` (which is why they stay dependency-free).

use std::{env, fs, path::Path};

include!("src/support/theme.rs");
include!("src/support/generator_basis.rs");

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

/// A particle contributes to a function name as itself — except bold, which is silent (so a
/// `(Bold, None, Red)` look is `recho`, not `borecho`).
fn name_part(particle: &str) -> &str {
    if particle == "bo" {
        ""
    } else {
        particle
    }
}

/// Capitalise the first letter, turning an atom's `name` into its enum variant (`"bold"` → `"Bold"`,
/// `"underlined"` → `"Underlined"`). The recho dimensions all have single-word names.
fn cap(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The generated style region: one `pub fn` per weight × underline × colour, each forwarding a
/// typed [`BasicLook`] to `_styled_echo`.
fn style_matrix() -> String {
    let mut blocks = Vec::new();
    for &w in Weight::ALL {
        for &u in Underline::ALL {
            for &c in Basic::ALL {
                let stem = format!(
                    "{}{}{}",
                    name_part(w.particle()),
                    name_part(u.particle()),
                    name_part(c.particle())
                );
                let name = if stem.is_empty() { "bo".to_string() } else { stem };
                // A function's doc names only its non-default criteria — skip the empty-particle
                // members (a `None` colour/underline) so labels never leak into the doc.
                let desc = [
                    (w.particle(), w.name()),
                    (u.particle(), u.name()),
                    (c.particle(), c.name()),
                ]
                .into_iter()
                .filter(|(particle, _)| !particle.is_empty())
                .map(|(_, name)| name)
                .collect::<Vec<_>>()
                .join(" ");
                let look = format!(
                    "BasicLook {{ weight: Weight::{}, underline: Underline::{}, colour: Basic::{} }}",
                    cap(w.name()),
                    cap(u.name()),
                    cap(c.name())
                );
                blocks.push(format!(
                    "    /// echo in {desc}\n    #[unprefixed]\n    #[alias(\"echo{name}\")]\n    pub fn {name}echo(args: EchoArgs) {{ _styled_echo({look}, &args); }}"
                ));
            }
        }
    }
    blocks.join("\n\n")
}

/// Render one search family (`G_FAMILY`/`GG_FAMILY`, defined in `generator_basis.rs`): a bare
/// `pub fn` per context size, forwarding to its runner.
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
            // The bare shim takes the full args; a numbered shim takes the base (no `-C` at all)
            // and pins its context while building the full set.
            let (args_ty, call) = if ctx == 0 {
                (family.args, format!("{}(&args)", family.runner))
            } else {
                (family.base_args, format!("{}(&{} {{ base: args, context: {ctx} }})", family.runner, family.args))
            };
            format!(
                "    /// {desc}\n    #[unprefixed]\n{}    pub fn {name}(args: {args_ty}) {{ {call}; }}",
                family.extra_attrs
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
