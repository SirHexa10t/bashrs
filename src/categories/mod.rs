//! The command categories — each module groups related commands (via the
//! `#[category]` macro). Assembled into the CLI and `sourcefile.sh` by [`crate::cli`].

pub mod autogen_styles;
pub mod bashrs;
pub mod comfy_repos;
pub mod filesystem;
pub mod media;
pub mod packages;
// Data, not a command category: the style vocabulary, shared with `build.rs` via `include!`.
pub(crate) mod style_vocab;
pub mod styles;
