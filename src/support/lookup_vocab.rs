// The `g`-family vocabulary: the context sizes (lines shown before/after each match). `0`
// means no context — the bare `g`; any `N > 0` becomes `g<N>` with `rg -C N`.
//
// Unlike `style_vocab`, nothing reads this at runtime (the generated functions bake their
// context in), so it isn't a crate module — only `build.rs` pulls it in via `include!` (it
// can't link the crate) to regenerate the grep family in `autogen_lookup.rs`. Keep it a plain
// list of `const`s: no other items and no `//!` docs, so it stays safe to `include!` anywhere.
pub(crate) const CONTEXTS: &[u32] = &[0, 2, 3, 5, 8, 25];
