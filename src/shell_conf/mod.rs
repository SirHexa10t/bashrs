//! Interactive-shell configuration emitted into `sourcefile.sh` — the mechanics and
//! settings that shape the shell environment: keybinds, session functions, and
//! environment/prompt settings (editor, grep colours, history, prompt). Assembled by
//! [`crate::cli`].

pub mod environment;
pub mod greeting;
pub mod keybinds;
pub mod session;
