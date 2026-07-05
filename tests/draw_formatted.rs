//! Future tag-processor tests — ported from `_draw_formatted.py`, kept commented out
//! until `draw_formatted` is implemented. These exercise the tag / stack / dedup /
//! persistence / whitespace / multi-line layer that our command-nesting `recho`/`_scoped`
//! model deliberately doesn't cover (see `src/categories/autogen_styles.rs`).
//!
//! `draw_formatted(text)` rewrites `<=tag=>` markers into ANSI using a style stack: each
//! tag pushes (emitting `reset + its code` — a clean start), `<=reset=>` pops back to the
//! enclosing style, already-processed `\x1b[0m` is respected, and consecutive resets
//! collapse. Note these colors are plain (`\x1b[31m`), not bold like the `recho` family —
//! bold is its own `<=bold=>` tag.
//!
//! To revive: implement `draw_formatted`, uncomment, and wire up the `use`.

/*
const R: &str = "\x1b[0m"; // processed reset

#[test]
fn tag_processing_and_stack() {
    // default behaviour: always end with a return to default
    assert_eq!(draw_formatted(""), R.to_string());
    assert_eq!(draw_formatted("a b c d"), format!("a b c d{R}"));

    // a tag chain becomes reset + its codes (a clean start)
    assert_eq!(draw_formatted("<=bold=><=green=>AAAAA<=reset=>"), format!("{R}\x1b[1m\x1b[32mAAAAA{R}"));
    assert_eq!(draw_formatted("<=green=>AAAAA"), format!("{R}\x1b[32mAAAAA{R}"));

    // every new style gets a clean start (styles don't blend)
    assert_eq!(
        draw_formatted("<=green=> a b <=bold=> c d <=blue=> e"),
        format!("{R}\x1b[32m a b {R}\x1b[1m c d {R}\x1b[34m e{R}"),
    );

    // `<=reset=>` pops the stack back to the enclosing style
    assert_eq!(
        draw_formatted("this is <=red=>red , <=green=>green<=reset=> , regular"),
        format!("this is {R}\x1b[31mred , {R}\x1b[32mgreen{R}\x1b[31m , regular{R}"),
    );
    assert_eq!(draw_formatted("<=blue=>bla<=reset=><=red=>bli"), format!("{R}\x1b[34mbla{R}\x1b[31mbli{R}"));

    // consecutive resets collapse to one
    assert_eq!(draw_formatted("aaaa<=reset=><=reset=><=reset=>bbbb"), format!("aaaa{R}bbbb{R}"));

    // a chain of the same color collapses to one
    assert_eq!(draw_formatted("only <=green=><=green=><=green=> color"), format!("only {R}\x1b[32m color{R}"));

    // already-processed input is respected (an embedded reset from a prior run)
    assert_eq!(
        draw_formatted(&format!("aa <=bold=>AAAAA{R}BBBBB<=reset=>CCCC")),
        format!("aa {R}\x1b[1mAAAAA{R}\x1b[1mBBBBB{R}CCCC{R}"),
    );

    // tag-like non-tags pass through untouched
    assert_eq!(draw_formatted("<=notapropertag=>AAA<=reset=>"), format!("<=notapropertag=>AAA{R}"));
    assert_eq!(draw_formatted("<=notapropertag=>AAA"), format!("<=notapropertag=>AAA{R}"));
}

// Further tag-processor features to port when they land:
//   - persistence: `<^^…^^>` (survives one line) and `<^^^…^^^>` (survives to EOF)
//   - whitespace tags: `<=13WS=>` -> 13 spaces (incl. at line start, and merging with resets)
//   - multi-line: a reset is appended at the end of each non-empty line
//   - machinery unit tests: TagChain (dedup + encode), Persistence (apply/identify/strip),
//     is_valid_tag / is_valid_reset, Matcher::split_by_matches
//   - the CLI test: same output via arg, pipe, and here-string
*/
