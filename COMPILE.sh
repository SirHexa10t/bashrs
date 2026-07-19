#!/usr/bin/env bash
# COMPILE.sh — build bashrs, then hand everything else to the freshly built binary itself:
# `install-stainless` provisions the non-Rust companions (bundled tools, repos, the Carstay.toml
# record), and `install-shell` installs the binary under ~/.bashrs (a self-copy), writes the
# sourcefile, and wires the rc files. Both live in Rust (src/drivers/mod.rs, src/conf/install.rs)
# where they're unit-tested — this script is just build + two calls.
#
# Safe to re-run: the binary is overwritten in place, and the shell integration is idempotent.
#
# Flags — the "stay on known-good versions" pair (either, or both):
#   --use-stable-cargo     skip `cargo update`; build --locked against the current Cargo.lock
#                          (the committed lock is the stable record: `git checkout -- Cargo.lock`
#                          first if a previous update already moved it)
#   --use-stable-carstay   provision the tool/repo versions RECORDED in Carstay.toml instead of
#                          the latest releases (`git log Carstay.toml` names known-good sets)
set -euo pipefail

die() { printf 'ERROR: %s\n' "$1" >&2; exit 1; }

# --- 0. Preconditions and flags ----------------------------------------------
[ -n "${HOME:-}" ]               || die "\$HOME is not set; cannot locate the install directory."
command -v cargo >/dev/null 2>&1 || die "cargo not found in PATH; install the Rust toolchain first."

# Run from the script's own directory (the project root), so cargo and the target/ paths below
# resolve no matter where this was invoked from.
cd -- "$(dirname -- "$0")" || die "could not enter the script's own directory."

# The freshly built binary — every step below runs from it (installing it under ~/.bashrs is
# its own job: `install-shell` self-copies).
BIN="./target/release/bashrs"

USE_STABLE_CARGO=0
SYNC_ARGS=()
for arg in "$@"; do
    case "$arg" in
        --use-stable-cargo)   USE_STABLE_CARGO=1 ;;
        --use-stable-carstay) SYNC_ARGS+=("$arg") ;;
        *) die "unknown argument: $arg (accepted: --use-stable-cargo, --use-stable-carstay)" ;;
    esac
done

# --- 1. Update dependencies (unless pinned), then build ----------------------
if [ "$USE_STABLE_CARGO" = 1 ]; then
    echo "--use-stable-cargo: keeping Cargo.lock exactly as it is (no cargo update); building --locked"
    cargo build --release --locked
else
    echo "cargo update: refreshing ALL crates to their latest compatible versions"
    # "Compatible" means within each Cargo.toml requirement's semver range — a requirement like
    # "0.8" is caret semantics (>=0.8, <0.9), so a MAJOR release is never taken automatically.
    # Crossing one is a deliberate act: run
    #     cargo upgrade --incompatible        (from `cargo install cargo-edit`)
    # which rewrites the requirements in Cargo.toml itself — then review, build, and test.
    LOCK_BEFORE=$(cksum Cargo.lock 2>/dev/null || true)
    cargo update
    if [ "$(cksum Cargo.lock 2>/dev/null || true)" != "$LOCK_BEFORE" ]; then
        printf '\033[1;34m%s\n%s\033[0m\n' \
            "New crate versions were locked. If this build misbehaves, revert to the last stable set:" \
            "    git checkout -- Cargo.lock && ./COMPILE.sh --use-stable-cargo   (or: bashrs_compile --use-stable-cargo)"
    fi
    cargo build --release
fi

# --- 2. Provision the non-Rust companions ------------------------------------
# Run straight from target/ (nothing is installed yet on a first run). Clones/updates the repos
# under ~/.bashrs/stainless_comfy so install-shell can alias them and read their --help live,
# keeps the self-contained tools (ffmpeg, python, …) under ~/.bashrs/tools on their latest
# published builds, and records the provisioned versions in Carstay.toml. Best-effort: a failure
# (offline, …) must not abort the compile.
#
# With --use-stable-carstay, the versions RECORDED in Carstay.toml are provisioned instead of
# the latest releases — the recover-a-broken-upstream path, --use-stable-cargo's twin.
"$BIN" install-stainless ${SYNC_ARGS[@]+"${SYNC_ARGS[@]}"} || echo "WARNING: install-stainless failed; companion aliases may be stale or missing." >&2

# --- 3. Install the binary + shell integration --------------------------------
# The binary installs ITSELF under ~/.bashrs (guarding the dir, unlink-then-copy so a running
# old copy survives), writes the sourcefile, and wires each existing rc file to source it
# (idempotent-by-marker) — one tested implementation, src/conf/install.rs.
"$BIN" install-shell
