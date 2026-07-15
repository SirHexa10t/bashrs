// The program's one colour theme — the whole style vocabulary as pure data, and the single basis
// every other styling module builds on. No imports and plain `//` comments (not `//!`): `build.rs`
// `include!`s this file textually (it can't link the crate) to regenerate the `recho` command
// matrix, so it must stay dependency-free and safe to paste mid-file.
//
// Every variant of a category enum (Weight, Underline, Basic, Md) is a `Style`, carrying its
// (name, particle, sgr) through that trait. The doc-only shades in `Md` never enter the recho
// matrix; the three recho dimensions bundle into a `BasicLook`, which — being specific to the
// autogenerator — lives in `generator_basis`, not here. Colour *policy* (which role gets which
// `Style`) lives with the consumers — `doc_style` for docs, `theme_code` for code highlighting.

/// The shared shape of every `Style`: its human `name`, its `recho` command `particle`, and its
/// SGR sub-code. One `parts()` per category; the accessors are derived from it. Object-safe (`&self`,
/// no `Copy` bound) so a heterogeneous `&[&dyn Style]` can be composed by `doc_style::_wrap`.
/// (`allow(dead_code)`: `name`/`particle` are used only by `build.rs`'s codegen and `sgr` only at
/// runtime, so each looks dead in the other compilation context.)
#[allow(dead_code)]
pub(crate) trait Style {
    /// `(name, particle, sgr)` — name first for readability.
    fn parts(&self) -> (&'static str, &'static str, &'static str);
    fn name(&self) -> &'static str {
        self.parts().0
    }
    fn particle(&self) -> &'static str {
        self.parts().1
    }
    fn sgr(&self) -> &'static str {
        self.parts().2
    }
}

/// Font weight. No `None` variant — every generated `recho` command carries a weight (bold is the
/// silent default). Room to grow (e.g. a combined variant later).
#[derive(Clone, Copy)]
pub(crate) enum Weight {
    Bold,
    Dark,
}

/// Underline, on or off. `None` is the "off" member so the `BasicLook` composite needs no `Option`.
#[derive(Clone, Copy)]
pub(crate) enum Underline {
    None,
    Underlined,
}

/// The base palette — the `recho` command colours. `None` is the "no colour" member.
#[derive(Clone, Copy)]
pub(crate) enum Basic {
    None,
    Red,
    Green,
    Blue,
    Cyan,
    Yellow,
    Orange,
    White,
    Magenta,
}

/// Doc-only shades the recho vocabulary deliberately doesn't carry (so they never spawn
/// `<name>echo` commands): the muted grey, the high-intensity cyan/magenta, and the italic attribute.
/// (`allow(dead_code)`: unused when `build.rs` `include!`s this file, which only touches the recho
/// dimensions.)
#[derive(Clone, Copy)]
#[allow(dead_code)]
pub(crate) enum Md {
    Grey,
    BrightCyan,
    BrightMagenta,
    Italic,
}

impl Style for Weight {
    fn parts(&self) -> (&'static str, &'static str, &'static str) {
        match self {
            Weight::Bold => ("bold", "bo", "1"),
            Weight::Dark => ("dark", "da", "2"),
        }
    }
}
impl Weight {
    #[allow(dead_code)] // build.rs-only: the recho matrix iterates it
    pub(crate) const ALL: &'static [Weight] = &[Weight::Bold, Weight::Dark];
}

impl Style for Underline {
    fn parts(&self) -> (&'static str, &'static str, &'static str) {
        match self {
            Underline::None => ("none", "", ""),
            Underline::Underlined => ("underlined", "u", "4"),
        }
    }
}
impl Underline {
    #[allow(dead_code)] // build.rs-only: the recho matrix iterates it
    pub(crate) const ALL: &'static [Underline] = &[Underline::None, Underline::Underlined];
}

impl Style for Basic {
    fn parts(&self) -> (&'static str, &'static str, &'static str) {
        match self {
            Basic::None => ("none", "", ""),
            Basic::Red => ("red", "r", "31"),
            Basic::Green => ("green", "g", "32"),
            Basic::Blue => ("blue", "b", "34"),
            Basic::Cyan => ("cyan", "c", "36"),
            Basic::Yellow => ("yellow", "y", "33"),
            Basic::Orange => ("orange", "or", "38;5;208"),
            Basic::White => ("white", "w", "37"),
            Basic::Magenta => ("magenta", "m", "35"),
        }
    }
}
impl Basic {
    #[allow(dead_code)] // build.rs-only: the recho matrix iterates it
    pub(crate) const ALL: &'static [Basic] = &[
        Basic::None,
        Basic::Red,
        Basic::Green,
        Basic::Blue,
        Basic::Cyan,
        Basic::Yellow,
        Basic::Orange,
        Basic::White,
        Basic::Magenta,
    ];
}

impl Style for Md {
    fn parts(&self) -> (&'static str, &'static str, &'static str) {
        match self {
            Md::Grey => ("grey", "gr", "90"),
            Md::BrightCyan => ("bright cyan", "bc", "96"),
            Md::BrightMagenta => ("bright magenta", "bm", "95"),
            Md::Italic => ("italic", "it", "3"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_report_name_particle_and_sgr() {
        assert_eq!(Basic::Cyan.name(), "cyan");
        assert_eq!(Basic::Cyan.particle(), "c");
        assert_eq!(Basic::Cyan.sgr(), "36");
        assert_eq!(Weight::Bold.sgr(), "1");
        assert_eq!(Md::Italic.sgr(), "3");
    }

    #[test]
    fn none_members_are_empty_so_they_contribute_nothing() {
        assert_eq!(Basic::None.sgr(), "");
        assert_eq!(Underline::None.sgr(), "");
    }

    #[test]
    fn all_covers_every_variant_for_the_generator_to_iterate() {
        assert_eq!(Basic::ALL.len(), 9); // None + 8 colours
        assert_eq!(Weight::ALL.len(), 2);
        assert_eq!(Underline::ALL.len(), 2);
    }
}
