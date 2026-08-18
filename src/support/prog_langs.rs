//! What each programming ecosystem writes onto disk without a human typing it — the sibling
//! of [`crate::support::package_management`], for languages instead of package managers.
//!
//! First consumer: `--lean`'s skip list ([`GENERATED_DIRS`]), printed by `_arg_lean_spec`.
//! The table carries the ecosystem and each entry's purpose so future consumers (per-language
//! tooling, project detection) can reuse it rather than re-listing.
//!
//! House rules for entries, learned the hard way:
//! - **Unambiguous or anchored.** `node_modules` means one thing; `dist` does not, so bundler
//!   output is anchored by its inner directory (`dist/assets/`) — a *source* dir named `dist`
//!   must not silently vanish.
//! - **Machine-written only.** Dot-*configs* a person typed (`.github/`, rc files) stay
//!   scanned; dot-*caches* a tool filled (`.venv/`, `.mypy_cache/`) belong here. The line is
//!   who wrote the contents, not the leading dot.
//! - **`*` is a whole path component**, nothing smaller: `target/*/release/` is any Cargo
//!   cross-compilation triple (`aarch64-…`, `wasm32-…`), where prefix guesses like `x86*`
//!   would miss half of them. No other wildcard shape exists.

/// One directory an ecosystem generates: where it appears, and what fills it.
pub struct GeneratedDir {
    /// Which toolchain writes it — a grouping label for reports.
    pub ecosystem: &'static str,
    /// The skip pattern: path components, `/`-separated; `*` matches exactly one component.
    pub pattern: &'static str,
    /// What lives inside, so a reader can judge what skipping it forgoes.
    pub purpose: &'static str,
}

/// Every machine-written directory `--lean` skips.
pub const GENERATED_DIRS: &[GeneratedDir] = &[
    // --- version control ---------------------------------------------------------------
    GeneratedDir { ecosystem: "vcs", pattern: ".git/", purpose: "git object store and packed history" },
    GeneratedDir { ecosystem: "vcs", pattern: ".hg/", purpose: "Mercurial store" },
    GeneratedDir { ecosystem: "vcs", pattern: ".svn/", purpose: "Subversion metadata and pristine copies" },
    // --- Rust / Cargo ------------------------------------------------------------------
    GeneratedDir { ecosystem: "rust", pattern: "target/release/", purpose: "optimized build artifacts" },
    GeneratedDir { ecosystem: "rust", pattern: "target/debug/", purpose: "debug build artifacts and incremental state" },
    GeneratedDir { ecosystem: "rust", pattern: "target/doc/", purpose: "rustdoc-generated HTML" },
    GeneratedDir { ecosystem: "rust", pattern: "target/*/release/", purpose: "cross-compiled artifacts, any target triple" },
    GeneratedDir { ecosystem: "rust", pattern: "target/*/debug/", purpose: "cross-compiled debug artifacts, any target triple" },
    // --- Python ------------------------------------------------------------------------
    GeneratedDir { ecosystem: "python", pattern: "__pycache__/", purpose: "compiled bytecode" },
    GeneratedDir { ecosystem: "python", pattern: ".venv/", purpose: "virtualenv: vendored packages and interpreter" },
    GeneratedDir { ecosystem: "python", pattern: ".pytest_cache/", purpose: "pytest state between runs" },
    GeneratedDir { ecosystem: "python", pattern: ".mypy_cache/", purpose: "mypy incremental type-check state" },
    GeneratedDir { ecosystem: "python", pattern: ".ruff_cache/", purpose: "ruff lint cache" },
    GeneratedDir { ecosystem: "python", pattern: ".tox/", purpose: "tox per-environment virtualenvs" },
    // --- JavaScript / bundlers ----------------------------------------------------------
    GeneratedDir { ecosystem: "js", pattern: "node_modules/", purpose: "installed npm dependencies" },
    GeneratedDir { ecosystem: "js", pattern: "dist/assets/", purpose: "bundler output (Vite-style), anchored so a source dir named dist survives" },
    GeneratedDir { ecosystem: "js", pattern: "build/static/", purpose: "bundler output (CRA-style), anchored like dist/assets" },
    GeneratedDir { ecosystem: "js", pattern: ".next/", purpose: "Next.js build cache" },
    GeneratedDir { ecosystem: "js", pattern: ".nuxt/", purpose: "Nuxt build cache" },
    GeneratedDir { ecosystem: "js", pattern: ".svelte-kit/", purpose: "SvelteKit build cache" },
    // --- JVM ---------------------------------------------------------------------------
    GeneratedDir { ecosystem: "jvm", pattern: ".gradle/", purpose: "Gradle caches and daemon state" },
    GeneratedDir { ecosystem: "jvm", pattern: "build/libs/", purpose: "Gradle jar output, anchored like dist/assets" },
    // --- general -----------------------------------------------------------------------
    GeneratedDir { ecosystem: "general", pattern: ".cache/", purpose: "tool caches by convention (webpack, uv, browsers, …)" },
];

/// The skip patterns alone, for composing into a skip list.
pub fn lean_patterns() -> impl Iterator<Item = &'static str> {
    GENERATED_DIRS.iter().map(|dir| dir.pattern)
}

/// What `_arg_lean_spec` prints: every entry, grouped by ecosystem, pattern beside purpose —
/// built from [`GENERATED_DIRS`] so the spec cannot drift from the behaviour.
pub fn spec_text() -> String {
    let width = GENERATED_DIRS.iter().map(|dir| dir.pattern.len()).max().unwrap_or(0);
    let mut out = String::from(
        "--lean skips machine-written directories: libraries, compile output, caches.\n\
         Patterns match whole path components; `*` is any single component.\n",
    );
    let mut current = "";
    for dir in GENERATED_DIRS {
        if dir.ecosystem != current {
            current = dir.ecosystem;
            out.push_str(&format!("\n{current}\n"));
        }
        out.push_str(&format!("  {:<width$}  {}\n", dir.pattern, dir.purpose));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The entry rules the module doc states, held mechanically: nothing empty, nothing
    /// duplicated, every pattern component-shaped, `*` only ever a whole component.
    #[test]
    fn every_entry_is_well_formed_and_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for dir in GENERATED_DIRS {
            assert!(!dir.purpose.is_empty(), "{} has no purpose", dir.pattern);
            assert!(seen.insert(dir.pattern), "{} is listed twice", dir.pattern);
            for part in dir.pattern.split('/').filter(|p| !p.is_empty()) {
                assert!(
                    part == "*" || !part.contains('*'),
                    "{}: `*` must be a whole component, not a prefix",
                    dir.pattern
                );
            }
        }
    }

    /// A lone `*` entry would skip every directory — the wildcard only makes sense anchored.
    #[test]
    fn wildcards_are_always_anchored_by_a_literal_component() {
        for dir in GENERATED_DIRS {
            let parts: Vec<&str> = dir.pattern.split('/').filter(|p| !p.is_empty()).collect();
            assert!(
                parts.iter().any(|part| *part != "*"),
                "{} is all wildcard",
                dir.pattern
            );
        }
    }

    #[test]
    fn the_spec_names_every_pattern_and_its_purpose() {
        let spec = spec_text();
        for dir in GENERATED_DIRS {
            assert!(spec.contains(dir.pattern), "spec is missing {}", dir.pattern);
            assert!(spec.contains(dir.purpose), "spec is missing {}'s purpose", dir.pattern);
        }
    }
}
