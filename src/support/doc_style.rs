//! The styling engine — SGR-escape assembly, scope-aware wrapping, and the status vocabulary.
//! The one place raw ANSI escapes are built, shared by the `recho` command matrix
//! ([`crate::categories::autogen_styles`]), the hand-written style commands
//! ([`crate::categories::styles`], e.g. `errcho`), and anything needing a styled string: `lll`'s
//! header, `gg`'s section titles, `code_highlight`.
//!
//! A nested colour/style restores the enclosing one when it ends (rather than clearing to the
//! terminal default), and every span starts from a clean slate so styles don't compound — see
//! [`_scoped`]. Styles are assembled from [`crate::support::theme`]'s `Style`s: [`_wrap`] joins
//! their SGR sub-codes, and the status helpers ([`problematic`], [`approved`], [`notice`],
//! [`broken_link_text`]) name the roles every caller shares.
//!
//! The *markdown* element palette (heading, bold, code, link, …) is not here: only
//! [`crate::support::doc_render`] ever painted those roles, so they live with the renderer that
//! owns markdown structure.

use crate::support::theme::{Basic, Style, Weight};

/// Ends a styled span, restoring the terminal to its default.
pub(crate) const RESET: &str = "\x1b[0m";

/// The ANSI escape for the given SGR sub-codes — e.g. `"1;34"` → `"\x1b[1;34m"`. The single place a
/// raw escape is constructed, so nothing else has to write `\x1b[…m` by hand.
pub(crate) fn escape(sgr: &str) -> String {
    format!("\x1b[{sgr}m")
}

/// Assemble an ANSI escape from a set of `Style`s — read each one's [`Style::sgr`], skip the
/// empties (a `None`-style member), and join with `;`. e.g. `_wrap(&[&Weight::Bold, &Basic::Red])`
/// → `"\x1b[1;31m"`; `_wrap(&[&Weight::Bold])` → `"\x1b[1m"`.
pub(crate) fn _wrap(styles: &[&dyn Style]) -> String {
    let sgr = styles.iter().map(|s| s.sgr()).filter(|c| !c.is_empty()).collect::<Vec<_>>().join(";");
    escape(&sgr)
}

/// The hues a name can land on: twelve, evenly spaced around the wheel.
///
/// QUANTIZED on purpose. Hashing onto a continuous circle clusters — twelve real crate names put
/// two of them 2 degrees apart, which is the worst possible outcome: colours that look identical
/// while claiming to differ. Landing on a step means a clash is an honest clash, two names in
/// plainly the same colour, and everything else is plainly apart.
const HUE_STEPS: u16 = 12;

/// The two vividness tiers a name can land on, doubling the distinguishable looks without
/// touching lightness — which is what readability actually rides on.
const HUE_SATURATIONS: [f64; 2] = [0.78, 0.42];

/// How light every hued name comes out, whichever hue and tier it took.
///
/// One value for all of them, as a deliberate compromise between backgrounds a terminal will not
/// name: light enough that a blue is not a hole on black, dark enough that a yellow is not a
/// smear on white.
const HUE_LIGHTNESS: f64 = 0.58;

/// A hue for `text`, in degrees around the colour wheel — one of [`HUE_STEPS`] evenly spaced
/// steps, the same one every time, on every machine, for as long as the name is spelled the same.
///
/// FNV-1a rather than the standard library's hasher: `DefaultHasher`'s output is explicitly not
/// promised to hold across Rust releases, and a colour that quietly changed under a toolchain
/// upgrade would undo the only thing this is for. A name is worth colouring because you learn
/// its colour.
pub(crate) fn hue(text: &str) -> u16 {
    let step = u16::try_from(_hash(text) % u64::from(HUE_STEPS)).unwrap_or(0);
    step * (360 / HUE_STEPS)
}

/// `text`, coloured by its own spelling — so one name is one colour wherever it appears, and the
/// eye can follow it through a list without reading it.
///
/// Nothing here can promise two names differ: a context-free colour is a hash, and hashes
/// collide. What it promises is that they either match exactly or are far apart, and that the
/// answer never changes.
pub(crate) fn hued(text: &str) -> String {
    // A separate slice of the same hash, so the tier does not track the hue.
    let tier = _hash(text) / u64::from(HUE_STEPS) % HUE_SATURATIONS.len() as u64;
    let (red, green, blue) = _hue_rgb(hue(text), HUE_SATURATIONS[tier as usize]);
    _scoped(&escape(&format!("38;2;{red};{green};{blue}")), text)
}

/// FNV-1a over the bytes. Small, fully specified, and unchanging — the three things that matter
/// for a value someone is going to recognise on sight.
fn _hash(text: &str) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    text.bytes().fold(OFFSET, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(PRIME))
}

/// One hue as truecolor RGB, at `saturation` and the fixed lightness. The standard HSL
/// conversion: `chroma` is the span between the channel extremes, `second` the middle channel's
/// share of it, and `base` lifts the lot to the wanted lightness.
fn _hue_rgb(hue: u16, saturation: f64) -> (u8, u8, u8) {
    let degrees = hue % 360;
    let chroma = (1.0 - HUE_LIGHTNESS.mul_add(2.0, -1.0).abs()) * saturation;
    let position = f64::from(degrees) / 60.0;
    let second = chroma * (1.0 - (position % 2.0 - 1.0).abs());
    // The sector comes from the INTEGER degrees, so it can never disagree with `position`
    // through the same number being rounded twice.
    let (red, green, blue) = match degrees / 60 {
        0 => (chroma, second, 0.0),
        1 => (second, chroma, 0.0),
        2 => (0.0, chroma, second),
        3 => (0.0, second, chroma),
        4 => (second, 0.0, chroma),
        _ => (chroma, 0.0, second),
    };
    let base = chroma.mul_add(-0.5, HUE_LIGHTNESS);
    let byte = |channel: f64| ((channel + base) * 255.0).round().clamp(0.0, 255.0) as u8;
    (byte(red), byte(green), byte(blue))
}

/// Style `text` in the shared bold-blue "header" style — `lll`'s column row and `gg`'s section
/// titles both use it, so the style is defined once and composed from theme styles like the rest.
pub(crate) fn _header(text: &str) -> String {
    _scoped(&_wrap(&[&Weight::Bold, &Basic::Blue]), text)
}

/// Style `text` as a broken/dangling link — bold red — so `lll` flags a Windows `.lnk` shortcut
/// (its name and target) against `ls`'s own colouring. Scoped, so it nests safely.
pub(crate) fn broken_link_text(text: &str) -> String {
    _scoped(&_wrap(&[&Weight::Bold, &Basic::Red]), text)
}

/// Style `text` as a "problematic" / bad-status marker — bold red — e.g. a `dl` heads-up that an
/// imported cookie store expired, or a video missing its thumbnail. Pairs with [`approved`]. A
/// distinct role from [`broken_link_text`], though it shares the colour. Scoped, so it nests safely.
pub(crate) fn problematic(text: &str) -> String {
    _scoped(&_wrap(&[&Weight::Bold, &Basic::Red]), text)
}

/// Style `text` as an "approved" / good-status marker — bold green — e.g. a video that already
/// carries its thumbnail. Pairs with [`problematic`]. Scoped, so it nests safely.
pub(crate) fn approved(text: &str) -> String {
    _scoped(&_wrap(&[&Weight::Bold, &Basic::Green]), text)
}

/// Bold blue — a heads-up worth reading that is neither praise nor a problem (version-drift
/// notices and the like).
pub(crate) fn notice(text: &str) -> String {
    _scoped(&_wrap(&[&Weight::Bold, &Basic::Blue]), text)
}

/// Style `text` with `codes`, keeping nested styles scoped instead of compounded.
///
/// Scopes are encoded in the stream itself, so nesting survives across the separate processes of
/// `recho "$(gecho …)"`: a span *opens* with `RESET + codes` (the reset is a clean start; the codes
/// set this style) and *closes* with a lone `RESET`. So we re-assert `codes` after each *closing*
/// reset already in `text` (a nested span that ended), and leave *opening* resets (a reset
/// immediately followed by an SGR) alone. Both are ordinary ANSI — harmless when not re-processed —
/// so the output renders the same in a terminal, a pipe, a file, or another `recho`.
pub(crate) fn _scoped(codes: &str, text: &str) -> String {
    let mut out = format!("{RESET}{codes}"); // open: clean start + this style
    let mut rest = text;
    while let Some(i) = rest.find(RESET) {
        let cut = i + RESET.len();
        out.push_str(&rest[..cut]); // text up to and including the reset
        rest = &rest[cut..];
        if !rest.starts_with("\x1b[") {
            out.push_str(codes); // a nested span closed here → restore my style
        }
    }
    out.push_str(rest);
    out.push_str(RESET); // close: lone reset
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every hue is one of the steps, and the value never drifts: these numbers are pinned
    /// because the whole point is that a name looks the same tomorrow. Change the hash and this
    /// fails, which is the warning it exists to give.
    #[test]
    fn a_name_always_lands_on_the_same_step() {
        for name in ["bashrs", "vidl", "", "a", "software_inventory", "🙂"] {
            let degrees = hue(name);
            assert!(degrees < 360, "{name:?} left the wheel: {degrees}");
            assert_eq!(degrees % (360 / HUE_STEPS), 0, "{name:?} landed off-step: {degrees}");
            assert_eq!(hue(name), degrees, "twice running must agree");
        }
        assert_eq!(hue("bashrs"), 0);
        assert_eq!(hue("vidl"), 0, "a clash is allowed — and is an honest one, not a near-miss");
        assert_eq!(hue("terminal_choice"), 30);
        assert_eq!(hue("software_inventory"), 210);
    }

    /// A clash in hue is survivable when the tier differs, which is what the second axis buys.
    /// The real crate names of this build must come out wholly distinguishable — that is the
    /// claim the feature makes, so it is the claim under test.
    #[test]
    fn this_build_s_own_crate_names_are_all_tellable_apart() {
        let names = [
            "bashrs", "ai_detection", "bashrs-macros", "filesync", "sequencer-cli",
            "sequencer-core", "sequencer-input", "software_inventory", "table_formatter",
            "terminal_choice", "vidl", "os_ricing",
        ];
        let looks: std::collections::BTreeSet<String> =
            names.iter().map(|name| hued(name)).collect();
        assert_eq!(looks.len(), names.len(), "two of these look alike");

        // `bashrs` and `vidl` share a hue; the tier is what keeps them apart.
        assert_eq!(hue("bashrs"), hue("vidl"));
        assert_ne!(hued("bashrs"), hued("vidl"), "same hue, and still distinguishable");
    }

    /// The colouring is a truecolor foreground, scoped like every other style here so it nests
    /// safely, and readable at every step rather than going dark at the blues.
    #[test]
    fn a_hued_name_is_scoped_truecolor_and_never_too_dark() {
        let painted = hued("bashrs");
        assert!(painted.contains("bashrs"), "the text survives: {painted:?}");
        assert!(painted.contains("\x1b[38;2;"), "truecolor foreground: {painted:?}");
        assert!(painted.ends_with(RESET), "closes like every other span: {painted:?}");
        let empty = hued("");
        assert!(empty.contains("\x1b[38;2;"), "empty text still resolves to a colour: {empty:?}");
        assert_eq!(empty, hued(""), "and to the same one twice");

        // Every step, at both tiers, keeps its channels off the floor — a hue that came out near
        // black would be a name you cannot read on a dark terminal.
        for step in 0..HUE_STEPS {
            for saturation in HUE_SATURATIONS {
                let (red, green, blue) = _hue_rgb(step * (360 / HUE_STEPS), saturation);
                let brightest = red.max(green).max(blue);
                assert!(brightest > 120, "step {step} at {saturation} is too dark: {red},{green},{blue}");
            }
        }
    }

    use crate::support::theme::Underline; // only the multi-style `_wrap` case needs it

    // ANSI shorthands for readable expectations.
    const RED: &str = "\x1b[1;31m";
    const GREEN: &str = "\x1b[1;32m";
    const BLUE: &str = "\x1b[1;34m";
    const ULINE: &str = "\x1b[1;4m";

    #[test]
    fn escape_builds_an_sgr_sequence() {
        assert_eq!(escape("1;34"), "\x1b[1;34m");
        assert_eq!(escape(""), "\x1b[m");
    }

    #[test]
    fn wrap_joins_style_sgrs_and_skips_empties() {
        assert_eq!(_wrap(&[&Weight::Bold, &Basic::Red]), "\x1b[1;31m"); // bold red
        assert_eq!(_wrap(&[&Weight::Bold]), "\x1b[1m"); // bold only (boecho)
        assert_eq!(
            _wrap(&[&Weight::Dark, &Underline::Underlined, &Basic::Red]),
            "\x1b[2;4;31m" // dark underlined red
        );
        // `None`-style empties drop out rather than leaving a stray `;`.
        assert_eq!(_wrap(&[&Basic::None, &Basic::Red]), "\x1b[31m");
    }

    #[test]
    fn header_is_bold_blue() {
        assert_eq!(_header("hi"), format!("{RESET}{BLUE}hi{RESET}"));
    }

    #[test]
    fn status_styles_paint_the_expected_colours() {
        // The vocabulary every caller shares. (The markdown palette these once sat beside is
        // tested in `doc_render`, which now owns it.)
        assert!(broken_link_text("x").contains(&_wrap(&[&Weight::Bold, &Basic::Red]))); // bold red
        assert_eq!(problematic("x"), format!("{RESET}{RED}x{RESET}")); // bold red, scoped
        assert_eq!(approved("x"), format!("{RESET}{GREEN}x{RESET}")); // bold green, scoped
    }

    #[test]
    fn plain_text_is_wrapped_and_reset() {
        assert_eq!(_scoped(RED, "hi"), format!("{RESET}{RED}hi{RESET}"));
    }

    #[test]
    fn empty_text_is_just_the_style_and_reset() {
        assert_eq!(_scoped(RED, ""), format!("{RESET}{RED}{RESET}"));
    }

    #[test]
    fn a_closing_reset_restores_the_enclosing_style() {
        let out = _scoped(RED, &format!("red {} y", _scoped(GREEN, "g")));
        assert_eq!(out, format!("{RESET}{RED}red {RESET}{GREEN}g{RESET}{RED} y{RESET}"));
    }

    #[test]
    fn a_nested_span_starts_clean() {
        let out = _scoped(RED, &format!("r {}", _scoped(ULINE, "u")));
        assert!(out.contains(&format!("{RESET}{ULINE}u{RESET}")), "underline span not clean: {out}");
    }

    #[test]
    fn an_already_processed_reset_re_asserts_the_style() {
        let out = _scoped(RED, "a \x1b[0m b");
        assert_eq!(out, format!("{RESET}{RED}a {RESET}{RED} b{RESET}"));
    }

    #[test]
    fn deeply_nested_spans_unwind_to_each_enclosing_style() {
        let inner = _scoped(GREEN, &format!("b {} d", _scoped(BLUE, "c")));
        let out = _scoped(RED, &format!("a {inner} e"));
        assert_eq!(
            out,
            format!("{RESET}{RED}a {RESET}{GREEN}b {RESET}{BLUE}c{RESET}{GREEN} d{RESET}{RED} e{RESET}"),
        );
    }

    #[test]
    fn sibling_spans_each_restore_the_enclosing_style() {
        let out = _scoped(RED, &format!("{} mid {}", _scoped(GREEN, "g"), _scoped(BLUE, "b")));
        assert!(out.contains(&format!("{GREEN}g{RESET}")), "green sibling not scoped: {out}");
        assert!(out.contains(&format!("{BLUE}b{RESET}")), "blue sibling not scoped: {out}");
        assert!(out.contains(&format!("{RED} mid {RESET}")), "middle should be red: {out}");
    }
}
