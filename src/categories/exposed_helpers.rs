//! Machinery exposed as commands (`_`-prefixed) — the specs behind flags and defaults, for
//! when a help line says "for details, run …" instead of carrying a list that outgrew it.
//!
//! The underscore is the contract: these are lookups about how bashrs behaves, not actions.
//! Nothing here touches a file or the network.

#[bashrs_macros::category(command = ExposedHelpersCommand, prefix = "_")]
mod commands {
    use crate::support::args::NoArgs;
    use crate::support::prog_langs;

    /// Print exactly what `--lean` skips (gg, GG, detect_ai_textual_fingerprint): every
    /// pattern, grouped by ecosystem, with what fills each directory
    pub fn arg_lean_spec(_args: NoArgs) {
        print!("{}", prog_langs::spec_text());
    }
}
