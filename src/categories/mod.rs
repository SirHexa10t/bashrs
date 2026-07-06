//! The command categories — each module groups related commands (via the
//! `#[category]` macro). Assembled into the CLI and `sourcefile.sh` by [`crate::cli`].

pub mod autogen_lookup;
pub mod autogen_styles;
pub mod bashrs;
pub mod comfy_repos;
pub mod filesystem;
pub mod git;
pub mod lookup;
pub mod media;
pub mod packages;
pub mod project;
// Data, not a command category: the style vocabulary, shared with `build.rs` via `include!`.
pub(crate) mod style_vocab;
// `lookup_vocab.rs` is build-time-only data (the g-family context sizes); nothing reads it at
// runtime, so it's `include!`d by `build.rs` and not declared as a module here.
pub mod styles;
