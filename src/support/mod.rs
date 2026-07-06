//! Rust support shared across the command categories — clap argument structs ([`args`]),
//! an external-process runner ([`exec`]), text-input resolution ([`input`]), and syntax
//! highlighting ([`syntax`]).

pub mod args;
pub mod exec;
pub mod input;
pub mod syntax;
