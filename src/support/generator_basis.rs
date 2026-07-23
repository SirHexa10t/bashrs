// The recho autogenerator's crate-side pieces: the `BasicLook` composite (the weight/underline/
// colour triple the matrix emits, plus its command-name/desc derivation) and the `g`/`gg` search
// families.
//
// `build.rs` `include!`s this file inside a `mod support { … }` mirror of the crate, so the
// `use crate::support::theme::…` below resolves in the build script too — that's what lets
// `BasicLook` name `theme`'s enums while living here. Plain `//` comments (not `//!`) for that same
// `include!` reason. The search families are build-time-only (`allow(dead_code)`); `BasicLook` is
// also used at runtime by `_styled_echo`.

use crate::support::theme::{Basic, Style, Underline, Weight};

/// The recho autogenerator's composite — the three matrix dimensions, each typed to its category
/// (`None` variants stand in for "absent", so no `Option`). `build.rs` builds one per combo to
/// derive its name/doc; `_styled_echo` takes one at runtime.
pub(crate) struct BasicLook {
    pub(crate) weight: Weight,
    pub(crate) underline: Underline,
    pub(crate) colour: Basic,
}

#[allow(dead_code)] // command_name/desc drive `build.rs`'s codegen; unused at runtime
impl BasicLook {
    /// The recho command name for this look: each particle contributes itself, except bold, which
    /// is silent (so `(Bold, None, Red)` is `recho`, not `borecho`), with an all-silent result
    /// falling back to `bo` (so a plain bold look is `boecho`).
    pub(crate) fn command_name(&self) -> String {
        fn part(particle: &str) -> &str {
            if particle == "bo" {
                ""
            } else {
                particle
            }
        }
        let stem = format!(
            "{}{}{}",
            part(self.weight.particle()),
            part(self.underline.particle()),
            part(self.colour.particle())
        );
        if stem.is_empty() {
            "bo".to_string()
        } else {
            stem
        }
    }

    /// The human words for this look's non-default criteria — its generated doc line. Empty-particle
    /// members (a `None` colour/underline) are skipped so they never leak in.
    pub(crate) fn desc(&self) -> String {
        [
            (self.weight.particle(), self.weight.name()),
            (self.underline.particle(), self.underline.name()),
            (self.colour.particle(), self.colour.name()),
        ]
        .into_iter()
        .filter(|(particle, _)| !particle.is_empty())
        .map(|(_, name)| name)
        .collect::<Vec<_>>()
        .join(" ")
    }
}

/// A generated search-shortcut family (`g`/`g<N>`, `gg`/`gg<N>`): the two differ only in this data,
/// so one `build.rs` template renders both. The bare (0-context) shim reads `-C` from the args; a
/// numbered shim pins it.
#[allow(dead_code)]
pub(crate) struct SearchFamily {
    /// Context sizes to expand (0 = the bare verb).
    pub(crate) contexts: &'static [usize],
    /// Function-name stem (`g` → `g`, `g3`, …).
    pub(crate) stem: &'static str,
    /// The full clap args struct — what the bare shim takes, and what a numbered shim builds by
    /// pinning `context` onto its base.
    pub(crate) args: &'static str,
    /// The reduced args struct the *numbered* shims take: the full set minus the pinned `-C`
    /// (see `args.rs`), so a pinned variant can't silently accept a context it would ignore.
    pub(crate) base_args: &'static str,
    /// The `lookup` runner every shim forwards to.
    pub(crate) runner: &'static str,
    /// Helper attributes each shim carries after `#[unprefixed]` (e.g. `#[trailing_newline]`).
    pub(crate) extra_attrs: &'static str,
    /// Doc line for the bare shim.
    pub(crate) bare_desc: &'static str,
    /// Doc line for a numbered shim, wrapped around its context count.
    pub(crate) ctx_desc: (&'static str, &'static str),
}

#[allow(dead_code)]
pub(crate) const G_FAMILY: SearchFamily = SearchFamily {
    contexts: &[0, 2, 3, 5, 8, 25],
    stem: "g",
    args: "GrepArgs",
    base_args: "GrepBase",
    runner: "_grep",
    extra_attrs: "",
    bare_desc: "Case-insensitive grep-search (literal, or regex with -E), colouring matches",
    ctx_desc: ("Case-insensitive grep-search, showing ", " lines of context around each match"),
};

#[allow(dead_code)]
pub(crate) const GG_FAMILY: SearchFamily = SearchFamily {
    contexts: &[0, 2, 3, 5, 10],
    stem: "gg",
    args: "GgArgs",
    base_args: "GgBase",
    runner: "_gg",
    extra_attrs: "    #[trailing_newline]\n",
    bare_desc: "Roughly `find`+`grep`: Recursively search a directory for expression(s) — filenames, then file contents",
    ctx_desc: ("Recursive search, showing ", " lines of context around each file-content match"),
};

// --- the generated regions (rendered here, written out by `build.rs`; unused at runtime, hence
// --- `allow(dead_code)`) -------------------------------------------------------------------------

/// Capitalise the first letter, turning a style's `name` into its enum variant (`"bold"` → `"Bold"`,
/// `"underlined"` → `"Underlined"`). The recho dimensions all have single-word names.
#[allow(dead_code)]
fn cap(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The generated style region: one `pub fn` per weight × underline × colour, each forwarding a typed
/// [`BasicLook`] to `_styled_echo`. Name and doc come from the look itself.
#[allow(dead_code)]
pub(crate) fn style_matrix() -> String {
    let mut blocks = Vec::new();
    for &w in Weight::ALL {
        for &u in Underline::ALL {
            for &c in Basic::ALL {
                let look = BasicLook { weight: w, underline: u, colour: c };
                let name = look.command_name();
                let desc = look.desc();
                let literal = format!(
                    "BasicLook {{ weight: Weight::{}, underline: Underline::{}, colour: Basic::{} }}",
                    cap(w.name()),
                    cap(u.name()),
                    cap(c.name())
                );
                blocks.push(format!(
                    "    /// echo in {desc}\n    #[unprefixed]\n    #[alias(\"echo{name}\")]\n    pub fn {name}echo(args: EchoArgs) {{ _styled_echo({literal}, &args); }}"
                ));
            }
        }
    }
    blocks.join("\n\n")
}

/// The generated search-family region for `family`: a bare `pub fn` per context size, forwarding to
/// its runner.
#[allow(dead_code)]
pub(crate) fn family_matrix(family: &SearchFamily) -> String {
    family
        .contexts
        .iter()
        .map(|&ctx| {
            let name =
                if ctx == 0 { family.stem.to_string() } else { format!("{}{ctx}", family.stem) };
            let desc = if ctx == 0 {
                family.bare_desc.to_string()
            } else {
                let (pre, post) = family.ctx_desc;
                format!("{pre}{ctx}{post}")
            };
            // The bare shim takes the full args; a numbered shim takes the base (no `-C` at all)
            // and pins its context while building the full set.
            let (args_ty, call) = if ctx == 0 {
                (family.args, format!("{}(&args)", family.runner))
            } else {
                (family.base_args, format!("{}(&{} {{ base: args, context: {ctx} }})", family.runner, family.args))
            };
            format!(
                "    /// {desc}\n    #[unprefixed]\n{}    pub fn {name}(args: {args_ty}) {{ {call}; }}",
                family.extra_attrs
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}
