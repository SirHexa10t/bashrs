# bashrs
.bashrc but rust instead of Bash

## Installation

Two things have to be on the system already: a Rust toolchain and `git`.

### Rust

For an up-to-date toolchain, install with **rustup**, the official toolchain manager (<https://rust-lang.org/tools/install>) — not your distribution's `rust` or
`cargo` package. rustup keeps everything under `~/.cargo` and `~/.rustup`, owned by you, and moves forward with `rustup update`.

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
. "$HOME/.cargo/env"  # one-time run that adds `~/.cargo/bin` to `PATH`
```

### bashrs

```sh
git clone https://github.com/SirHexa10t/bashrs.git
cd bashrs
./COMPILE.sh
```

Expect a few minutes on the first run. It would:
 * build from scratch
 * download the bundled tools
 * tell you how to reset your shell session to source the compiled functions (automatic after first run)
 

Re-running `COMPILE.sh` is safe, and is also how you update: `git pull && ./COMPILE.sh`.
It stores data within your home-dir (`~/.bashrs`), so the script works wherever you place this project.

### Fine control

Two flags exist for staying on proven-good versions:

| flag | effect |
|---|---|
| `--use-stable-cargo` | skip the dependency refresh; build against `Cargo.lock` exactly as committed |
| `--use-stable-carstay` | provision the tool and companion-repo versions recorded in `Carstay.toml`, rather than the latest releases |

### What it touches

Only your home directory. The binary copies itself to `~/.bashrs/`, writes
`~/.bashrs/sourcefile.sh` beside it, puts the bundled tools under `~/.bashrs/tools/`, and adds one
marked block to whichever of `~/.bashrc` and `~/.zshrc` you already have — marked so that
re-running finds that block and replaces it instead of stacking copies.

If the next shell greets you with a syntax error rather than a prompt, consult the **Troubleshooting** section. It is likely the expected bump when moving from an rc that already defines some of the same names.

## Troubleshooting

Entries are keyed by the message you actually see, so search this section for the text in front
of you.

### `syntax error near unexpected token '('`

**What you see:** At shell startup/refresh, two lines naming the sourcefile — and afterwards none of the
bashrs commands exist:

```
/home/you/.bashrs/sourcefile.sh: line 214: syntax error near unexpected token `('
/home/you/.bashrs/sourcefile.sh: line 214: `..() { cd .. "$@"; }'
```

**Why:** Bash expands aliases as it *reads* a line, and a function's name is the first word on its
line. If one of your own rc files still defines an alias by that name, the definition is rewritten
before it is parsed — with `alias ..='cd ..'` active, `..() {` is read as `cd ..() {`, which is not
valid syntax. The failure then aborts the entire sourcefile, so every command defined below that
point never comes into existence. That is why the symptom looks like "bashrs did nothing" rather
than "one command is broken".

**Fix:** The second line bash prints is the definition it choked on; the name in front of `()` is
the culprit — `..` in the example above. Remove that alias from your own rc files:

```sh
alias ..    # confirm it is an alias, and see what it expands to
grep -rnF 'alias ..=' ~/.bashrc ~/.bash_aliases ~/.profile ~/.bash_profile 2>/dev/null
```

Delete or comment out the line it reports, then open a new shell. If more than one name collides,
each will surface the same way, one per restart.

Expect this while migrating. bashrs re-implements commands that used to live in your rc, so your
old definitions are precisely the ones that collide — and clearing them out is the point.
