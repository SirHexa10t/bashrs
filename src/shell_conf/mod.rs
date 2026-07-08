//! Interactive-shell configuration emitted into `sourcefile.sh` — the mechanics and
//! settings that shape the shell environment: keybinds, session functions, and
//! environment/prompt settings (editor, grep colours, history, prompt), and aliases to
//! cloned non-Rust companion repos ([`stainless`]). Assembled by [`crate::cli`].

pub mod environment;
pub mod greeting;
pub mod keybinds;
pub mod session;
pub mod stainless;
