#!/usr/bin/env bash
# TEST.sh — run ALL of bashrs's tests, including the live ones `cargo test` skips.
#
# Five categories, cheapest first:
#   1. offline        unit + integration (stubbed yt-dlp, fixture cookie DBs) — hermetic, fast
#   2. macros         the proc-macro crate's own tests: the attribute contracts the compiler
#                     enforces, e.g. that every hand-written shell fragment names both bash and
#                     fish. Plain `cargo test` builds the macros but never runs their tests.
#   3. fish           the fish sourcefile through a real fish (and `fish_indent`'s check of its
#                     layout), plus fish quoting read back by fish. These also run in offline;
#                     listed apart so a fish regression has its own line, and REQUIRED — a machine
#                     without fish or fish_indent fails this category by name instead of skipping.
#   4. zsh            the bash sourcefile through a real zsh, whose .zshrc sources it too: the
#                     parts zsh does its own way (greeting at the first prompt, keybinds, `hg`'s
#                     history, its prompt left alone), aliases of the commands' names, and the
#                     contracts bash's file keeps. These also run in offline; REQUIRED, as
#                     fish's are — a machine without zsh fails this category by name.
#   5. live-extended  one test: that the cookie store `dl --cookie-import` writes is readable by
#                     the genuine yt-dlp loader. It skips-with-notice unless a store has been
#                     imported, and sends no network requests — it only reads a real session.
#
# Downloading lives in the `vidl` crate, and its live download tests moved there with it; run
# vidl's own TEST.sh for those. The live category here can still fail for environmental reasons,
# so both run regardless and the summary shows what passed where.
set -uo pipefail

command -v cargo >/dev/null 2>&1 || { printf 'ERROR: cargo not found in PATH.\n' >&2; exit 1; }
cd "$(dirname "$0")"

declare -A RESULT
ORDER=()   # every category, in the order it ran — the summary reads this, never a list of its own

run_category() {
    local name="$1"; shift
    ORDER+=("$name")
    echo
    echo "=== ${name} ==="
    if "$@"; then
        RESULT[$name]="PASS"
    else
        RESULT[$name]="FAIL"
    fi
}

# --no-fail-fast: run every test binary even when one fails — a red target must not hide the rest.
# --nocapture: several offline tests are skip-with-notice (they self-skip, printing a "SKIPPED …"
# line, when ffmpeg or a sqlite-capable python is unavailable). Without --nocapture libtest hides
# a passing test's output, so a silent skip would read as a clean pass — grep the run for
# "SKIPPED" to see exactly which, if any, opted out.
# The fish checks need fish itself — and `script` (util-linux), which gives a new fish the terminal
# its first prompt is drawn on. Not having one is a failure, named, rather than a skip: bashrs
# supports fish, and a run that could not check that support has not passed it.
fish_checks() {
    local tool status=0
    for tool in fish fish_indent script; do
        command -v "$tool" >/dev/null 2>&1 || { printf 'ERROR: %s not found in PATH — the fish checks need it.\n' "$tool" >&2; return 1; }
    done
    cargo test --no-fail-fast --lib -- --nocapture shell_quote::tests::fish \
        categories::filesystem::tests::names_are_spelled_so_fish_reads_them_back || status=1
    cargo test --no-fail-fast --test sourcefile_fish -- --nocapture || status=1
    return "$status"
}

# The zsh checks need zsh itself, and not having it is a failure, named, for the same reason:
# bashrs supports zsh — the install wires ~/.zshrc to the bash sourcefile.
zsh_checks() {
    command -v zsh >/dev/null 2>&1 || { printf 'ERROR: zsh not found in PATH — the zsh checks need it.\n' >&2; return 1; }
    cargo test --no-fail-fast --test sourcefile_zsh -- --nocapture
}

run_category "offline"       cargo test --no-fail-fast -- --nocapture
run_category "macros"        cargo test --no-fail-fast -p bashrs-macros -- --nocapture
run_category "fish"          fish_checks
run_category "zsh"           zsh_checks
run_category "live-extended" cargo test --test dl_live_extended -- --ignored --test-threads=1 --nocapture

echo
echo "=== summary ==="
failed=0
for name in "${ORDER[@]}"; do
    printf '  %-14s %s\n' "$name" "${RESULT[$name]}"
    [ "${RESULT[$name]}" = "FAIL" ] && failed=1
done
if ! ls "$HOME/.bashrs/user-data/browser_cookies/youtube/browser.spec" >/dev/null 2>&1; then
    echo
    echo "note: no youtube cookie store is imported, so the cookie-gated test self-skipped"
    echo "      (SKIPPED line above) — run \`dl --cookie-import youtube\` for full coverage."
fi
exit "$failed"
