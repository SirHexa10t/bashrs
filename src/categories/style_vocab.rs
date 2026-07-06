// The style vocabulary — the single source of truth for the `recho` family.
//
// Each entry is `(key, SGR code, human word)`: the key names the criterion in a command's
// `[weight, underline, color]` triple and keys the runtime lookup in `_wrap`; the code is
// the SGR sub-code; the human word feeds the generated doc comments. An empty-key entry is
// a default (adds nothing to a style); its word, when set (e.g. underline's `unchanged`),
// is only descriptive.
//
// Two consumers share this file: the crate uses it at runtime (`_wrap`), and `build.rs`
// pulls it in via `include!` — it can't link the crate — to regenerate the command matrix
// in `autogen_styles.rs`. Keep it a plain list of `const`s: no other items and no `//!`
// docs, so it stays safe to `include!` anywhere. Edit here to add or change a style; the
// matrix regenerates on the next build.

pub(crate) const WEIGHTS: &[(&str, &str, &str)] = &[("bo", "1", "bold"), ("da", "2", "dark")];
pub(crate) const UNDERLINES: &[(&str, &str, &str)] = &[("", "", "unchanged"), ("u", "4", "underlined")];
pub(crate) const COLORS: &[(&str, &str, &str)] = &[
    ("", "", ""),
    ("r", "31", "red"),
    ("g", "32", "green"),
    ("b", "34", "blue"),
    ("c", "36", "cyan"),
    ("y", "33", "yellow"),
    ("or", "38;5;208", "orange"),
    ("w", "37", "white"),
    ("m", "35", "magenta"),
];
