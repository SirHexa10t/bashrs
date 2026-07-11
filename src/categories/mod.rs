//! The command categories — each module groups related commands (via the
//! `#[category]` macro). Assembled into the CLI and `sourcefile.sh` by [`crate::cli`].
//! One exception in mechanism, not in nature: [`session`] holds commands that must run in the
//! shell itself, contributed as raw shell functions rather than clap commands.

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
pub mod python;
pub mod session;
pub mod styles;
