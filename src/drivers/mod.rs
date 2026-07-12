//! The tool drivers — feature-facing logic that *commands* the bundled tools
//! ([`crate::tools`] acquires, resolves, and exposes them; this layer drives them): the python
//! environment management behind the `py_*` commands, the yt-dlp orchestration behind `dl_yt`,
//! and the companion-repo ("stainless") sync. A driver may use any tool and everything below;
//! the command categories stay thin argument shells over drivers. Split from `tools` because
//! the two change at different speeds — plumbing is stable, drivers grow with every feature.

pub mod python;
pub mod stainless;
pub(crate) mod youtube;
