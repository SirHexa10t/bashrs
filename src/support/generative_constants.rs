// All the data that drives build-time code generation, in one place (merged from the former
// style/lookup/treegrep "vocab" files). Two consumers: the crate reads the style vocabulary at
// runtime (`doc_style::_wrap`), and `build.rs` `include!`s this whole file (it can't link the
// crate) to regenerate the `recho` matrix and the `g`/`gg` search families. Keep it plain data —
// `const`s plus the one struct they need, no `//!` docs — so it stays safe to `include!` anywhere.

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

// The `g`/`gg` search-family vocabulary, expanded into `autogen_lookup.rs`. Build-time-only — the
// generated shims bake their tuning in, so nothing reads any of this at runtime
// (`allow(dead_code)`); `build.rs` consumes it via `include!`.

/// A generated search-shortcut family (`g`/`g<N>`, `gg`/`gg<N>`): the two differ only in this data,
/// so one `build.rs` template renders both. The bare (0-context) shim reads `-C` from the args; a
/// numbered shim pins it.
#[allow(dead_code)]
pub(crate) struct SearchFamily {
    /// Context sizes to expand (0 = the bare verb).
    pub(crate) contexts: &'static [usize],
    /// Function-name stem (`g` → `g`, `g3`, …).
    pub(crate) stem: &'static str,
    /// The full clap args struct — what the bare shim takes, and what a numbered shim builds by
    /// pinning `context` onto its base.
    pub(crate) args: &'static str,
    /// The reduced args struct the *numbered* shims take: the full set minus the pinned `-C`
    /// (see `args.rs`), so a pinned variant can't silently accept a context it would ignore.
    pub(crate) base_args: &'static str,
    /// The `lookup` runner every shim forwards to.
    pub(crate) runner: &'static str,
    /// Helper attributes each shim carries after `#[unprefixed]` (e.g. `#[trailing_newline]`).
    pub(crate) extra_attrs: &'static str,
    /// Doc line for the bare shim.
    pub(crate) bare_desc: &'static str,
    /// Doc line for a numbered shim, wrapped around its context count.
    pub(crate) ctx_desc: (&'static str, &'static str),
}

#[allow(dead_code)]
pub(crate) const G_FAMILY: SearchFamily = SearchFamily {
    contexts: &[0, 2, 3, 5, 8, 25],
    stem: "g",
    args: "GrepArgs",
    base_args: "GrepBase",
    runner: "_grep",
    extra_attrs: "",
    bare_desc: "Case-insensitive search (literal, or regex with -E), colouring matches",
    ctx_desc: ("Case-insensitive search, showing ", " lines of context around each match"),
};

#[allow(dead_code)]
pub(crate) const GG_FAMILY: SearchFamily = SearchFamily {
    contexts: &[0, 2, 3, 5, 10],
    stem: "gg",
    args: "GgArgs",
    base_args: "GgBase",
    runner: "_gg",
    extra_attrs: "    #[trailing_newline]\n",
    bare_desc: "Recursively search a directory for expression(s) — filenames, then file contents",
    ctx_desc: ("Recursive search, showing ", " lines of context around each file-content match"),
};
