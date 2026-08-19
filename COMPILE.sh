#!/usr/bin/env bash
# COMPILE.sh — build bashrs, then hand everything else to the freshly built binary itself:
# `install-stainless` provisions the non-Rust companions (bundled tools, repos, the Carstay.toml
# record), and `install-shell` installs the binary under ~/.bashrs (a self-copy), writes the
# sourcefile, and wires the rc files. Both live in Rust (src/drivers/mod.rs, src/conf/install.rs)
# where they're unit-tested. What stays in this script is only what cannot: the dependency
# refresh below has to run before the binary it would otherwise live in has been built.
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

# --- Dependency refresh ------------------------------------------------------
# `cargo update` with no arguments re-resolves the git dependencies too, one repo at a time and
# serially. Each costs at least a network round trip to conclude nothing changed — and far more
# than that wherever `api.github.com` is slow to resolve, which is the ten-seconds-per-repo case
# TROUBLESHOOTING.md documents.
#
# Cargo has no knob for this. Updating sources in parallel is an open request that has not been
# designed yet, let alone shipped (rust-lang/cargo#15934), and shallow fetches are nightly-only
# (`-Zgit=shallow-deps`). So the cheap question gets asked outside cargo instead: one
# `git ls-remote` per repo, all at once, is a single ref advertisement that transfers no objects
# and answers for the whole set in about a third of a second.
#
# Cargo is then handed an explicit list rather than a blanket instruction: every registry crate
# (so "latest compatible versions" still means exactly that) plus only the git packages whose
# tip actually moved. Naming a package restricts cargo to its own source, so an unchanged repo is
# never contacted at all.
#
# Even where nothing is wrong with the network this roughly breaks even against a bare
# `cargo update`: it asks its own questions, but spares cargo four serial ones. It repays
# properly the moment contacting a repo is expensive.
cargo_update_fresh() {
    [ -r Cargo.lock ] || { cargo update; return; }   # no lock yet: nothing to compare against

    # One record per git source: a package that belongs to it, and the source it came from.
    # A repo that is itself a workspace contributes several packages sharing one source; the
    # first is enough, since naming it updates the whole source and its siblings move with it.
    local deps
    deps=$(awk '
        /^name = /                 { gsub(/"/,""); name = $3 }
        /^source = "git\+/ {
            gsub(/"/,""); src = $3
            if (!(src in seen)) { seen[src] = 1; print name "\t" src }
        }' Cargo.lock)

    # Ask every repo whose tip can move, all at once. Which ref that is depends on how the
    # dependency was written: an explicit branch, or the remote's own HEAD when it just tracks
    # the default branch. A tag or rev pin cannot move, so it is not polled at all.
    #
    # Protocol v0 on purpose: v2 has to negotiate capabilities and then ask for refs, two round
    # trips, and it earns that back only by filtering server-side across a large ref namespace.
    # These repos have a handful of refs each, so v0's single advertisement measures faster.
    local tmp; tmp=$(mktemp -d)
    local -a pkgs=() locked=()
    local pkg src ref url i=0
    while IFS=$'\t' read -r pkg src; do
        [ -n "$src" ] || continue
        case "$src" in
            *'?branch='*) ref=${src#*\?branch=}; ref="refs/heads/${ref%%#*}" ;;
            *'?'*)        continue ;;   # tag= or rev= — pinned, nothing to ask
            *)            ref=HEAD ;;
        esac
        url=${src#git+}; url=${url%%\?*}; url=${url%%#*}
        i=$((i + 1))
        pkgs[i]=$pkg
        locked[i]=${src##*#}
        ( git -c protocol.version=0 ls-remote "$url" "$ref" 2>/dev/null \
            | cut -f1 > "$tmp/$i.sha" ) &
    done <<< "$deps"
    wait

    # Compare, and collect the packages that genuinely need cargo's attention.
    local -a stale=()
    local n remote
    for ((n = 1; n <= i; n++)); do
        remote=$(cat "$tmp/$n.sha" 2>/dev/null || true)
        if [ -z "$remote" ]; then
            # Unreachable (offline, repo gone): keep what is locked rather than fail the compile.
            printf '  ? %-16s unreachable — keeping locked %s\n' "${pkgs[n]}" "${locked[n]:0:8}" >&2
        elif [ "$remote" = "${locked[n]}" ]; then
            printf '  = %-16s %s\n' "${pkgs[n]}" "${locked[n]:0:8}"
        else
            printf '  + %-16s %s -> %s\n' "${pkgs[n]}" "${locked[n]:0:8}" "${remote:0:8}"
            stale+=("${pkgs[n]}")
        fi
    done
    rm -rf -- "$tmp"

    # Every registry crate by exact name@version — several crates appear at more than one
    # version in the graph, and a bare name would be an ambiguous spec.
    local -a specs=()
    mapfile -t specs < <(awk '
        /^name = /    { gsub(/"/,""); n = $3 }
        /^version = / { gsub(/"/,""); v = $3 }
        /^source = "registry\+/ { print "--package=" n "@" v }' Cargo.lock | sort -u)

    cargo update "${specs[@]}" ${stale[0]+"${stale[@]/#/--package=}"}
}

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
    cargo_update_fresh
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
