//! Interactive-shell configuration emitted into `sourcefile.sh` — the mechanics and
//! settings that shape the shell environment (keybinds, session functions, and later
//! prompt, history, date-format preferences). Assembled by [`crate::cli`].

pub mod keybinds;
pub mod session;
