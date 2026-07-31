#!/usr/bin/env bash
# TEST.sh — run ALL of bashrs's tests, including the live ones `cargo test` skips.
#
# Two categories, cheapest first:
#   1. offline        unit + integration (stubbed yt-dlp, fixture cookie DBs) — hermetic, fast
#   2. live-extended  one test: that the cookie store `dl --cookie-import` writes is readable by
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

run_category() {
    local name="$1"; shift
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
run_category "offline"       cargo test --no-fail-fast -- --nocapture
run_category "live-extended" cargo test --test dl_live_extended -- --ignored --test-threads=1 --nocapture

echo
echo "=== summary ==="
failed=0
for name in offline live-extended; do
    printf '  %-14s %s\n' "$name" "${RESULT[$name]}"
    [ "${RESULT[$name]}" = "FAIL" ] && failed=1
done
if ! ls "$HOME/.bashrs/user-data/browser_cookies/youtube/browser.spec" >/dev/null 2>&1; then
    echo
    echo "note: no youtube cookie store is imported, so the cookie-gated test self-skipped"
    echo "      (SKIPPED line above) — run \`dl --cookie-import youtube\` for full coverage."
fi
exit "$failed"
