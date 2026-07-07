//! Rust support shared across the command categories — clap argument structs ([`args`]), a syntax
//! colour theme ([`color_theme`]), the styling engine ([`doc_style`]), an external-process runner
//! ([`exec`]), text-input resolution ([`input`]), an in-process stream grep ([`streamgrep`]), and a
//! recursive tree grep ([`treegrep`]).

pub mod args;
pub mod color_theme;
pub mod doc_style;
pub mod exec;
pub mod input;
pub mod streamgrep;
// Data, not a helper module: the style vocabulary, shared with `build.rs` via `include!`.
// (`lookup_vocab` / `treegrep_vocab` sit here too but are build-time-only — `include!`d, not modules.)
pub(crate) mod style_vocab;
pub mod treegrep;
