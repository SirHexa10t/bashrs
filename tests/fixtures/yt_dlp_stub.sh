#!/usr/bin/env bash
# A yt-dlp stand-in for bashrs's offline tests of the commands it builds over vidl
# (tests/dl_commands.rs). It records each invocation's argv in calls.log under BASHRS_STUB_DIR and
# succeeds without a word: what those tests look at is which yt-dlp ran and what it was handed —
# never what came back, which is vidl's to test, against its own stand-in.
#
# Bash builtins only, so it runs on the tests' HERMETIC PATH, which holds nothing but itself and
# bash.

dir="${BASHRS_STUB_DIR:?BASHRS_STUB_DIR must point at the test scratch dir}"
printf '%s\n' "$*" >> "$dir/calls.log"
exit 0
