#!/usr/bin/env bash
# COMPILE.sh — build bashrs, install it under ~/.bashrs, generate the shell
# function wrappers, and source them from the user's shell rc file(s).
#
# Safe to re-run: the binary and wrappers are overwritten in place, and the
# source block is added to each rc file only if it is not already present.
set -euo pipefail

die() { printf 'ERROR: %s\n' "$1" >&2; exit 1; }

# --- 0. Preconditions -------------------------------------------------------
[ -n "${HOME:-}" ]               || die "\$HOME is not set; cannot locate the install directory."
command -v cargo >/dev/null 2>&1 || die "cargo not found in PATH; install the Rust toolchain first."

BASHRS_HOME="$HOME/.bashrs"
BIN="$BASHRS_HOME/bashrs"
FUNCTIONS="$BASHRS_HOME/functions.sh"
# Kept literal (single-quoted): expanded when the rc file is sourced, not now.
SRC_PATH='$HOME/.bashrs/functions.sh'

# --- 1. Build ---------------------------------------------------------------
cargo build --release

# --- 2. Install the binary --------------------------------------------------
# Preserve ~/.bashrs itself: it may be a symlink (e.g. into a dotfiles repo).
# We only ever write the files inside it, never remove or replace the directory.
if [ -L "$BASHRS_HOME" ] && [ ! -d "$BASHRS_HOME" ]; then
    die "$BASHRS_HOME is a symlink to a missing target; fix or remove it, then re-run."
fi
mkdir -p "$BASHRS_HOME"                        # no-op if it already exists (dir or symlink-to-dir)
install -m 755 target/release/bashrs "$BIN"    # overwrite just the binary, keep it executable
echo "Installed $BIN"

# --- 3. Generate the wrappers from the freshly installed binary -------------
"$BIN" generate > "$FUNCTIONS"
echo "Generated $FUNCTIONS"

# --- 4. Source the wrappers from each rc file that exists -------------------
# Idempotent-by-marker: the block is bracketed by markers, so a re-run finds it
# and skips rather than appending a duplicate.
BLOCK_START="# >>> bashrs >>>"
BLOCK_END="# <<< bashrs <<<"

wire_rc() {
    local rc="$1"
    [ -e "$rc" ] || return 0                   # only touch rc files that already exist
    [ -f "$rc" ] || die "$rc exists but is not a regular file."
    [ -w "$rc" ] || die "$rc is not writable."
    if grep -qF "$BLOCK_START" "$rc"; then
        echo "Source block already present in $rc — leaving it untouched."
        return 0
    fi
    {
        printf '\n%s\n' "$BLOCK_START"
        printf '[ -r "%s" ] && . "%s"\n' "$SRC_PATH" "$SRC_PATH"
        printf '%s\n' "$BLOCK_END"
    } >> "$rc" || die "Failed to append the source block to $rc."
    echo "Added source block to $rc"
}

for rc in "$HOME/.bashrc" "$HOME/.zshrc"; do
    wire_rc "$rc"
done

if [ ! -e "$HOME/.bashrc" ] && [ ! -e "$HOME/.zshrc" ]; then
    echo "Note: neither ~/.bashrc nor ~/.zshrc exists; nothing was wired up."
fi

echo
echo "Done. Open a new shell, or run:  . \"$FUNCTIONS\""
