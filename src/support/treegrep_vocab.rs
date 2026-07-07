// The `gg`-family context sizes (lines shown before/after each file-content match), like
// `lookup_vocab` for the `g` family: `0` is the bare `gg`; any `N > 0` becomes `gg<N>`.
//
// Build-time-only data — the generated functions bake their context in, so nothing reads this at
// runtime. It isn't a crate module; only `build.rs` pulls it in via `include!` (it can't link the
// crate) to regenerate the `gg` family in `autogen_treegrep.rs`. Keep it a plain `const`.

pub(crate) const GG_CONTEXTS: &[usize] = &[0, 2, 3, 5, 10];
