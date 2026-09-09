# Declared Move and Rename Authoring

`prikk mv <old> <new>` is the first prikk command that authors a `RenamePath` operation. Renames are
never inferred from content — comparing bytes to guess "this looks like that file, renamed" is not
something prikk does. A rename only ever comes from an explicit declaration.

```sh
prikk mv old-name.txt new-name.txt
```

This both performs the move on disk and records a declaration. The declaration is durable, kept
alongside the active queue, and is authored into the *next* `prikk commit` — not immediately.

## The four worktree states

`prikk mv` keys its behavior on existence only, never on content:

| Worktree state | Behavior |
|---|---|
| `<old>` exists, `<new>` does not | Perform the rename, record the declaration |
| `<old>` is gone, `<new>` exists | Record the declaration only — no bytes are touched |
| Both exist | Refused — two files exist, and which one is the tracked node is not prikk's to guess |
| Neither exists | Refused — there is nothing to move |

The second row is what makes the command useful for the ordinary flow of running a shell `mv` and
only afterward remembering to tell prikk about it:

```sh
mv old-name.txt new-name.txt   # already moved by hand
prikk mv old-name.txt new-name.txt   # records the declaration; touches no bytes
```

## Reviewing a live declaration before committing

A declaration made and forgotten becomes permanent history at the next commit. `prikk worktree-status`
shows every live declaration, in prose and in `--format json`:

```sh
prikk worktree-status
```

```text
live rename declarations: 1
  old-name.txt -> new-name.txt
```

## What a commit does with a live declaration

`prikk commit` consumes every live declaration against the worktree at commit time:

- **Confirmed** (old path absent, new path present): authors a `RenamePath`. Any simultaneous content
  or mode change on the same node is authored alongside it in the same patch.
- **Nets to deletion**: if the declared destination is itself gone before the commit — deleted, or
  excluded by `.prikkignore` — the declaration is dropped and a plain deletion is authored instead,
  never a rename to nowhere.
- **Never tracked**: if the declared source was never a sealed node (an untracked file, moved with
  `prikk mv` before its first commit), there is no node to rename.
- **Contradicted**: if the worktree disagrees with the declaration (the source path is back on disk),
  the whole commit is refused, naming the declaration. Nothing is silently dropped.

Every one of these outcomes is correct on its own — a deletion really is the honest record for a file
moved into an ignored directory, for instance. What would be wrong is doing this silently: a user who
asserted a move deserves to see what actually happened to it. So `prikk commit` names every declaration
that did not become the rename it asserted, one line each, alongside the operations it authors:

```text
$ prikk mv a.txt build/a.txt        # build/ is excluded by .prikkignore
moved a.txt -> build/a.txt
$ prikk commit -m moved
  delete-file a.txt
  declaration a.txt -> build/a.txt: destination is ignored; recorded as a deletion, not a rename
```

Renaming a node more than once before a commit collapses to the net move: `prikk mv a b` followed by
`prikk mv b c` authors exactly one `RenamePath`, `a -> c` — never two hops. A declaration that returns
to its starting point (`prikk mv a b` then `prikk mv b a`) is dropped entirely; nothing is authored —
and `prikk mv` says so immediately, since the round trip is resolved right there, not at the next
commit:

```text
$ prikk mv a.txt b.txt
moved a.txt -> b.txt
note: this declaration is authored into the next `prikk commit`; see `prikk worktree-status` to review it first
$ prikk mv b.txt a.txt
moved b.txt -> a.txt
declaration a.txt -> b.txt -> a.txt: nets to no move, dropped
```

## The commit-time hint — a suggestion about what you *didn't* declare

If you move a file with a plain shell `mv` and never tell prikk, `commit` sees an ordinary delete
and an ordinary create — nothing tells you identity could have been preserved. `commit` may print a
hint for that:

```text
$ mv old-name.txt new-name.txt      # a shell mv, not prikk mv -- never declared
$ prikk commit -m moved
  delete-file old-name.txt
  create-file new-name.txt
hint: looks like old-name.txt moved to new-name.txt; `prikk mv` would have preserved its identity
```

**The hint and a disclosure line look deliberately different, on purpose**, because they mean
opposite things — one is a fact about something you asserted, the other is a guess about something
you didn't:

```text
  declaration a.txt -> build/a.txt: destination is ignored; recorded as a deletion, not a rename
hint: looks like old-name.txt moved to new-name.txt; `prikk mv` would have preserved its identity
```

A disclosure line is indented like the operations above it, past tense, and says what *did* happen
to something you declared. A hint is unindented, conditional ("would have"), and says what *might*
have been true of something you never declared at all — the same shape as this guide's own "note:"
lines, not the operation list.

**The hint authors nothing.** It costs a line of output, never a fact in history, which is exactly
why it is allowed to be wrong. It never prompts and never blocks, and there is no flag to make it a
default — the machine suggests, you assert, always with `prikk mv` itself.

The signal is exact content equality only — never similarity scoring, and never anything cleverer.
A few cases the signal alone would get wrong are handled directly:

- **A declared move never triggers it.** `prikk mv` already authored a `RenamePath`; there is
  nothing left to suggest.
- **An ambiguous set prints no hint.** Two deleted files and two created files sharing identical
  content — which moved to which is unknowable, and a wrong guess is worse than silence.
- **A mass reorganisation prints one summary line**, not a wall of hints, once a commit's own
  unambiguous candidates pass a small threshold.

Two nodes can be swapped in one commit by chaining declarations through a temporary name:

```sh
prikk mv a.txt tmp.txt
prikk mv b.txt a.txt
prikk mv tmp.txt b.txt
prikk commit --from-worktree -m "swap a.txt and b.txt"
```

## What this does not do (yet)

- No similarity scoring in the commit-time hint above — exact content equality only, and nothing
  cleverer.
- No conflict witness for two declarations naming the same destination.
- Symlink renaming is out of scope, the same as symlink authoring generally.
