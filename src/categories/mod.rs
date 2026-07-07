//! The command categories — each module groups related commands (via the
//! `#[category]` macro). Assembled into the CLI and `sourcefile.sh` by [`crate::cli`].

pub mod autogen_lookup;
pub mod autogen_styles;
pub mod autogen_treegrep;
pub mod bashrs;
pub mod comfy_repos;
pub mod filesystem;
pub mod git;
pub mod lookup;
pub mod media;
pub mod packages;
pub mod project;
pub mod styles;
