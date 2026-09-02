//! Rust support shared across the command categories — clap argument structs ([`args`]), a syntax
//! highlighter ([`theme_code`]), the one shared colour theme ([`theme`]), a Markdown-doc
//! renderer ([`doc_render`]), the styling engine ([`doc_style`]), an external-process runner
//! ([`exec`]), shared support for the wrapped-repo commands ([`comfy_repos`]), text-input
//! resolution ([`input`]), the network-probing engines ([`net`]), an in-process stream grep
//! ([`streamgrep`]), a
//! recursive tree grep ([`treegrep`] — its `--delve` binary decoders are a private child), output
//! capture and the stdout colour policy ([`shell`]), shell-word quoting shared by the
//! string-displaying commands ([`shell_quote`]), filename-stamp formatting ([`preferences`]),
//! privilege escalation ([`superuser`]), and the system package-manager registry
//! ([`package_management`]).

pub mod align;
pub mod args;
pub mod comfy_repos;
pub mod doc_render;
pub mod doc_style;
pub mod exec;
pub mod input;
pub mod net;
// The software-world resource lists moved to their own extractable crate; re-exported here so
// every `crate::support::…` call site keeps reading naturally.
pub use software_inventory::{package_management, prog_langs};
pub mod preferences;
pub mod shell;
pub mod shell_quote;
pub mod streamgrep;
pub mod superuser;
// Data, not a helper module: the `g`/`gg` search-family generation data plus the `BasicLook`
// composite, shared with `build.rs` via `include!`. The generation data is build-time-only, but
// `BasicLook` is read at runtime too — every generated `recho` verb hands one to `_styled_echo`.
// The style vocabulary now lives in `theme`, which `build.rs` also `include!`s.
pub(crate) mod generator_basis;
pub mod theme;
pub mod theme_code;
pub mod treegrep;
pub mod windows;
