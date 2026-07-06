//! Rust support shared across the command categories — clap argument structs ([`args`]),
//! an external-process runner ([`exec`]), text-input resolution ([`input`]), syntax highlighting
//! ([`syntax`]), and an in-process stream grep ([`streamgrep`]).

pub mod args;
pub mod exec;
pub mod input;
pub mod streamgrep;
pub mod syntax;
