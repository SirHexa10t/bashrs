// All the constants that drive build-time code generation, in one place (merged from the former
// style/lookup/treegrep "vocab" files). Two consumers: the crate reads the style vocabulary at
// runtime (`doc_style::_wrap`), and `build.rs` `include!`s this whole file (it can't link the
// crate) to regenerate the `recho` matrix and the `g`/`gg` search families. Keep it a plain list of
// `const`s — no other items, no `//!` docs — so it stays safe to `include!` anywhere.

// Style vocabulary for the `recho` matrix: `(key, SGR sub-code, human word)`. Read at runtime by
// `doc_style::_wrap` and baked into `autogen_styles` by `build.rs`.
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

// Search-family context sizes (lines shown around each match): `0` is the bare `g`/`gg`; any `N`
// becomes `g<N>`/`gg<N>`. Build-time-only — the generated functions bake their context in, so
// nothing reads these at runtime (`allow(dead_code)`); `build.rs` consumes them via `include!`.
#[allow(dead_code)]
pub(crate) const CONTEXTS: &[u32] = &[0, 2, 3, 5, 8, 25];
#[allow(dead_code)]
pub(crate) const GG_CONTEXTS: &[usize] = &[0, 2, 3, 5, 10];
