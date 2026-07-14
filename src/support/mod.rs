//! Rust support shared across the command categories — clap argument structs ([`args`]), a syntax
//! colour theme ([`color_theme`]), a Markdown-doc renderer ([`doc_render`]), the styling engine
//! ([`doc_style`]), an external-process runner
//! ([`exec`]), text-input resolution ([`input`]), an in-process stream grep ([`streamgrep`]), a
//! recursive tree grep ([`treegrep`]), binary-format decoders for `gg --delve` ([`delve`]), output
//! capture and the stdout colour policy ([`shell`]), filename-stamp formatting ([`preferences`]),
//! privilege escalation ([`superuser`]), the system package-manager registry
//! ([`package_management`]), and browser cookie-store discovery ([`browsers`]).

pub mod args;
pub mod browsers;
pub mod color_theme;
pub mod delve;
pub mod doc_render;
pub mod doc_style;
pub mod exec;
pub mod input;
pub mod package_management;
pub mod preferences;
pub mod shell;
pub mod streamgrep;
pub mod superuser;
// Data, not a helper module: every build-time-generation constant (style vocab + search families),
// merged into one file and shared with `build.rs` via `include!`. Only the style vocab is also read
// at runtime (by `doc_style`); the search families are build-time-only.
pub(crate) mod generative_constants;
pub mod treegrep;
