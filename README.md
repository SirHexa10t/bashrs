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
`~/.bashrs/sourcefile.sh` and `~/.bashrs/sourcefile.fish` beside it, puts the bundled tools under
`~/.bashrs/tools/`, and adds one marked block to whichever of `~/.bashrc` and `~/.zshrc` you
already have — marked so that re-running finds that block and leaves it alone instead of stacking
copies.

For fish, once fish has been run (so `~/.config/fish` exists), it adds a one-line
`~/.config/fish/conf.d/bashrs.fish` instead, and never edits `config.fish`. Where fish already has
completions for one of bashrs's command names (`diff`, Mercurial's `hg`), it also writes a small
file per name to `~/.bashrs/fish_completions/`, so that bashrs's completions replace fish's rather
than mixing with them.

If a bashrs command does something else when you type it, consult the **Troubleshooting** section: something of the same name, defined after bashrs's block, outranks it.

## Troubleshooting

Entries are keyed by the message you actually see, so search this section for the text in front
of you.

### A bashrs command runs something else

**What you see:** One of bashrs's commands does something else when you type it — `g foo` runs
`git foo`, say — while the rest of bashrs works.

**Why:** Something of the same name was defined *after* bashrs's block, and the last definition
wins: an alias later in your rc file, or from a plugin that loads after it — or in fish, a
function in `config.fish`, which fish reads after `conf.d`. An alias defined *before* the block
(oh-my-zsh's `g` for `git`, say), bashrs sets aside as it loads, and a fish function it replaces.
A fish abbreviation, though, expands as you type it, wherever it was defined.

**Fix:** Remove the later definition, or move it ahead of bashrs's block — in fish, into a
`conf.d` file that sorts before `bashrs.fish` — and remove or rename a fish abbreviation.
`shell_def` shows what runs, what it outranks, and what bashrs set aside, with the line that
brings it back — to keep yours over bashrs's, put that line after the block.

```sh
shell_def g    # "g is an ALIAS - the top rank, this is what runs", then the outranked FUNCTION
\g foo         # runs bashrs's `g` past an alias or abbreviation, this once
grep -rnF 'alias g=' ~/.bashrc ~/.bash_aliases ~/.zshrc 2>/dev/null                 # bash, zsh
grep -rnwE 'alias|abbr|function' ~/.config/fish 2>/dev/null | grep -w g             # fish
```

Expect this while migrating. bashrs re-implements commands that used to live in your rc, so your
old definitions are precisely the ones that collide — and clearing them out is the point.
