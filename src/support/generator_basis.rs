// The `g`/`gg` search-family generation data, expanded into `autogen_lookup.rs`. Build-time-only —
// the generated shims bake their tuning in, so nothing reads this at runtime (`allow(dead_code)`);
// `build.rs` can't link the crate, so it pulls this file in textually via `include!`. Keep it plain
// data — the one struct plus its two consts, no `//!` docs — so it stays safe to `include!`
// anywhere. (The style vocabulary now lives in `theme.rs`, likewise `include!`d by `build.rs`.)

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
