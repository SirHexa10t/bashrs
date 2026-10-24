//! Terminal commands (`term_*`): what the terminal emulator itself does, as opposed to the shell
//! running in it ([`crate::categories::shell`]) — so far its opacity, stepped by
//! [`crate::support::opacity`]. `term_opacity_up` and `term_opacity_down` are bound to ALT+= (and
//! ALT++) and ALT+- (see [`crate::conf::keybinds`]), run in place, beside the line being typed.

#[bashrs_macros::category(command = TerminalCommand, prefix = "term_")]
mod commands {
    use clap::Args;

    use crate::support::opacity::{self, Level, Stepped};

    /// Make the terminal opaque, by 5 points. If unsupported, turns its whole window opaque
    pub fn opacity_up(args: OpacityArgs) {
        _step("term_opacity_up", f64::from(args.step) / 100.0);
    }

    /// Make the terminal transparent, by 5 points. If unsupported, turns its whole window transparent
    pub fn opacity_down(args: OpacityArgs) {
        _step("term_opacity_down", -f64::from(args.step) / 100.0);
    }

    /// How far one step goes.
    #[derive(Args)]
    pub struct OpacityArgs {
        /// The step, in percent
        #[arg(default_value_t = 5, value_parser = clap::value_parser!(u8).range(1..=100))]
        pub step: u8,
    }

    /// Step, and say what happened: the change on stdout, any way passed over on its way there on
    /// stderr — or, when nothing could, why, and a failing exit.
    fn _step(command: &str, delta: f64) {
        match opacity::step(delta) {
            Ok(stepped) => {
                println!("{}", _done(&stepped, delta));
                for (name, why) in &stepped.passed_over {
                    eprintln!("{command}: {name} couldn't — {why}");
                }
            }
            Err(reasons) => {
                eprintln!("{}", _undone(command, &reasons));
                std::process::exit(1);
            }
        }
    }

    /// What a step did, in a line: who did it, what fades, and where it got to — or by how much
    /// it moved, where the backend can't tell.
    fn _done(stepped: &Stepped, delta: f64) -> String {
        let what = match stepped.level {
            Level::Background => "background opacity",
            Level::Window => "window opacity",
        };
        let amount = match stepped.now {
            Some(now) => format!("{:.0}%", now * 100.0),
            None => format!("{:+.0}%", delta * 100.0),
        };
        format!("{}: {what} {amount}", stepped.by)
    }

    /// Why nothing changed: each way that applied, with its reason — or, where none did, the
    /// terminals and window managers that can.
    fn _undone(command: &str, reasons: &[(&str, String)]) -> String {
        if reasons.is_empty() {
            return format!(
                "{command}: nothing here can change the terminal's opacity — that takes a terminal that does it itself ({}), or a window manager that does it to the window ({})",
                opacity::names(Level::Background).join(", "),
                opacity::names(Level::Window).join(", "),
            );
        }
        let mut text = format!("{command}: the terminal's opacity could not be changed:");
        for (name, why) in reasons {
            text.push_str(&format!("\n  {name}: {why}"));
        }
        text
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The line names who did it and what fades, then where it got to — or, from a backend
        /// that can't tell, by how much it moved, signed.
        #[test]
        fn a_step_is_reported_by_who_what_and_how_far() {
            let stepped = |by, level, now| Stepped { by, level, now, passed_over: Vec::new() };
            assert_eq!(_done(&stepped("kitty", Level::Background, Some(0.85)), 0.05), "kitty: background opacity 85%");
            assert_eq!(_done(&stepped("KWin", Level::Window, None), -0.1), "KWin: window opacity -10%");
            assert_eq!(_done(&stepped("sway", Level::Window, None), 0.05), "sway: window opacity +5%");
        }

        /// Nothing done reads two ways: every reason, one per way that applied — or, where none
        /// did, which terminals and window managers would.
        #[test]
        fn nothing_done_says_why_or_what_would() {
            let reasons = [("kitty", "remote control is off".to_string()), ("KWin", "busctl failed".to_string())];
            assert_eq!(
                _undone("term_opacity_up", &reasons),
                "term_opacity_up: the terminal's opacity could not be changed:\n  kitty: remote control is off\n  KWin: busctl failed"
            );
            let nothing = _undone("term_opacity_down", &[]);
            assert!(nothing.starts_with("term_opacity_down: nothing here can change the terminal's opacity"), "{nothing}");
            for name in opacity::names(Level::Background).into_iter().chain(opacity::names(Level::Window)) {
                assert!(nothing.contains(name), "{name} is named: {nothing}");
            }
        }
    }
}
