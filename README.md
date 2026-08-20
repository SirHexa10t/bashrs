# bashrs
.bashrc but rust instead of Bash

## Troubleshooting

Entries are keyed by the message you actually see, so search this section for the text in front
of you.

### `syntax error near unexpected token '('`

**What you see.** At shell startup, two lines naming the sourcefile — and afterwards none of the
bashrs commands exist:

```
/home/you/.bashrs/sourcefile.sh: line 214: syntax error near unexpected token `('
/home/you/.bashrs/sourcefile.sh: line 214: `..() { cd .. "$@"; }'
```

**Why.** Bash expands aliases as it *reads* a line, and a function's name is the first word on its
line. If one of your own rc files still defines an alias by that name, the definition is rewritten
before it is parsed — with `alias ..='cd ..'` active, `..() {` is read as `cd ..() {`, which is not
valid syntax. The failure then aborts the entire sourcefile, so every command defined below that
point never comes into existence. That is why the symptom looks like "bashrs did nothing" rather
than "one command is broken".

**Fix.** The second line bash prints is the definition it choked on; the name in front of `()` is
the culprit — `..` in the example above. Remove that alias from your own rc files:

```sh
alias ..    # confirm it is an alias, and see what it expands to
grep -rnF 'alias ..=' ~/.bashrc ~/.bash_aliases ~/.profile ~/.bash_profile 2>/dev/null
```

Delete or comment out the line it reports, then open a new shell. If more than one name collides,
each will surface the same way, one per restart.

Expect this while migrating. bashrs re-implements commands that used to live in your rc, so your
old definitions are precisely the ones that collide — and clearing them out is the point.
