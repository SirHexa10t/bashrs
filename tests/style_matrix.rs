//! Generator and drift guard for the styled-echo matrix in `src/categories/autogen_styles.rs`.
//!
//! Emits (into the marked region) the criterion→code maps and one `pub fn` per
//! `weight × underline × color` combination. Each function names its style by a
//! `[weight, underline, color]` triple that `_wrap` resolves against the maps — so the
//! source reads as its criteria, not opaque escapes. Edit the lists below, then:
//!   `cargo test --test style_matrix regenerate -- --ignored`
//! `matrix_is_current` fails if `autogen_styles.rs` has drifted from these lists.

use std::fs;

const AUTOGEN_STYLES_RS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/categories/autogen_styles.rs");
const START: &str = "    // GENERATED-STYLE-MATRIX-START";
const END: &str = "    // GENERATED-STYLE-MATRIX-END";

// Each dimension: (criterion key, SGR code, human word). The key is what a function
// passes to `_wrap` and what the map stores; the human word feeds both the per-function
// doc (`/// echo in bold red`) and the map's legend comment (see `map_const`). An
// empty-key entry is a default — it adds nothing to a style, so it's left out of function
// docs, but its word (when set, e.g. underline's `unchanged`) still labels that "off"
// state in the legend.
// A function's name is the keys concatenated — except `bo` (bold) is silent, so a plain
// color is `recho`, surfacing as `bo` only when a combination is otherwise nameless
// (`boecho`).
const WEIGHTS: &[(&str, &str, &str)] = &[("bo", "1", "bold"), ("da", "2", "dark")];
const UNDERLINES: &[(&str, &str, &str)] = &[("", "", "unchanged"), ("u", "4", "underlined")];
const COLORS: &[(&str, &str, &str)] = &[
    ("", "", ""),
    ("r", "31", "red"),
    ("g", "32", "green"),
    ("b", "34", "blue"),
    ("c", "36", "cyan"),
    ("y", "33", "yellow"),
    ("or", "38;5;208", "orange"),
    ("w", "37", "white"),
];

fn map_const(name: &str, entries: &[(&str, &str, &str)]) -> String {
    let items = entries.iter().map(|(k, code, _)| format!("(\"{k}\", \"{code}\")")).collect::<Vec<_>>();
    // Legend: the human words joined, so the map reads as its meaning. An empty-key entry
    // contributes only when its "off" state is worth naming (e.g. underline's `unchanged`).
    let legend = entries.iter().map(|(_, _, word)| *word).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" / ");
    format!("    const {name}: &[(&str, &str)] = &[{}];  // {legend}", items.join(", "))
}

/// A criterion key contributes to a function name as itself — except bold, which is
/// silent (so `["bo","","r"]` is `recho`, not `borecho`).
fn name_part(key: &str) -> &str {
    if key == "bo" { "" } else { key }
}

/// The generated region: the three maps, then a `pub fn` per combination.
fn matrix() -> String {
    let maps = [
        map_const("WEIGHTS", WEIGHTS),
        map_const("UNDERLINES", UNDERLINES),
        map_const("COLORS", COLORS),
    ]
    .join("\n");

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
                    .filter(|(key, _)| !key.is_empty())
                    .map(|(_, word)| word)
                    .collect::<Vec<_>>()
                    .join(" ");
                // Each command also gets an `echo`-suffixed alias (e.g. `recho` -> `echor`)
                // so the whole family completes after typing `echo` in a shell.
                blocks.push(format!(
                    "    /// echo in {desc}\n    #[unprefixed]\n    #[alias(\"echo{name}\")]\n    pub fn {name}echo(args: EchoArgs) {{ _styled_echo([\"{wk}\", \"{uk}\", \"{ck}\"], &args); }}"
                ));
            }
        }
    }

    // A sentinel COMPILE.sh can grep for to tell whether this region has actually been
    // generated: it appears only in generated output, so a cleared/never-run region lacks
    // it and COMPILE.sh knows to run `regenerate` before building.
    let sentinel = "    // STYLIZED_ECHO_COMMANDS — generated; do not edit (regenerate via COMPILE.sh).";
    format!("{sentinel}\n{maps}\n\n{}", blocks.join("\n\n"))
}

/// `autogen_styles.rs` with its generated region replaced by the current matrix.
fn expected(current: &str) -> String {
    let start = current.find(START).expect("START marker missing from autogen_styles.rs") + START.len();
    let end = current.find(END).expect("END marker missing from autogen_styles.rs");
    format!("{}\n{}\n{}", &current[..start], matrix(), &current[end..])
}

#[test]
fn matrix_is_current() {
    let current = fs::read_to_string(AUTOGEN_STYLES_RS).unwrap();
    assert_eq!(
        current,
        expected(&current),
        "autogen_styles.rs matrix is stale — regenerate: cargo test --test style_matrix regenerate -- --ignored",
    );
}

#[test]
#[ignore = "writes src/categories/autogen_styles.rs"]
fn regenerate() {
    let current = fs::read_to_string(AUTOGEN_STYLES_RS).unwrap();
    fs::write(AUTOGEN_STYLES_RS, expected(&current)).unwrap();
}
