# RFC 153 — `prikk diff`: two points of history, or the worktree against its baseline

**Status.** **PROPOSED 2026-09-13 by the architect**, on the owner's word (*"Approved"*) to the
assessment in `.git-exclude/reviewed/git-mapping-currency-review-2026-09-13.md`. Scheduling is the
owner's: the architect proposes it as the theme after 0.43 (RFC 136). Author-review independence: the
architect designs and will review the implementation; the gap is compensated by the controls of §6,
each of which must be shown to fail.

## 1. What exists today, and what does not

- `prikk show <block|patch>` (RFC 142) renders what **one** block or patch changed, from the patch's own
  payload: content-anchored spans, deliberately not a unified diff, and no replay for content.
  `show.md` closes with: comparing two arbitrary points *"is a deliberately separate, not-yet-open
  question"*. This RFC opens it.
- `checkout --patch-plan --format json --content-path <p>` (RFC 143) gives the content of named paths at
  a ref's tip. Ref-addressed only; a bare block id is an open row (ROADMAP §C, RFC 144 §4t).
- `worktree-status` reports the worktree's drift against the replay baseline: names and kinds, no
  content.
- Replay to a point is `replay_supported_patch_chain(layout, ref_name)` — by ref; the block-addressed
  walk (`single_parent_chain`) is internal.

So a user who wants to know *what changed between here and there* reconstructs both sides by hand.
For an agent collaborator that is the most common read after `log`.

## 2. The command

```
prikk diff [path] [--from <ref|block-id>] [--to <ref|block-id>] [--path <p>]... [--format json]
```

- **Two points**: `--from A --to B` compares the state at A with the state at B. Each is a ref name
  (`heads/…`, `tags/…`, `remotes/…`) or a bare block id; one resolver, and the same resolver makes
  `checkout --ref <block-id>` work (closing RFC 144 §4t on the way — say so in the increment).
- **Worktree**: with no `--to`, the right side is the worktree; with no `--from`, the left side is the
  current branch's tip (RFC 151's pointer). Bare `prikk diff` is therefore Git's `git diff HEAD`: the
  whole worktree against what `commit` would author against, with content.
- `--path <p>` (repeatable, root-relative, exact) filters the **output**; replay is whole-tree either
  way and the cost does not shrink — the help text says so.
- Read-only. Writes nothing, takes no lock, and holds the same read snapshot both replays use.

## 3. What it reports

Per path, in canonical path order, one status: `added`, `deleted`, `modified`, `renamed` (only a rename
the history **declares** — `RenamePath` — never one inferred by similarity), `mode` (executable bit),
`symlink` (target changed), `binary` (differs; ids and sizes, never bytes). Then, for a text file, a
**line-oriented unified rendering** of the two reconstructed contents: hunks with context, computed by
a small pure module in the store (a shortest-edit-script over lines; deterministic; no external tool,
no dependency). This does not contradict `show`'s stance: `show` refuses to *invent* line structure for
an `EditText` span because the span is the record; `diff` compares two whole contents, and lines are a
rendering of the comparison, not a claim about how the change was authored.

Worktree side only: untracked paths are **named** under `untracked`, without content (they are not in
history; Git's `diff` omits them too, and naming them avoids the "why is my file not here" question);
`.prikkignore` applies as it does for `commit`; RFC 147 §3f's unsupported paths are named with
`commit`'s refusal and no content.

`--format json` is `diff-report-v1`: `from`/`to` (each: how it was named, the resolved block id or
`"worktree"`), `entries[]` (`path`, `status`, `from`/`to` blob ids and modes where they exist, `hunks[]`
as text), `untracked[]`, `unsupported_paths[]`. Settles the format for `diff` and nothing else.

## 4. Failures — and one that is not

- A ref that does not exist, or a block id the store does not hold: `Precondition`, exit 1, naming it.
- An unsupported operation anywhere in either chain fails the whole call (RFC 140 §7b's rule, as
  `checkout` and RFC 143 already do); never a partial answer.
- Two points that resolve to the same block, or a clean worktree: exit 0, an empty report — not an
  error, and the prose says *no differences*.
- A malformed id, an unknown flag, a repeated `--format`: usage, exit 2.
- Damage the object store reports (hash or type mismatch) propagates as `Integrity`; absence of a blob
  degrades that one entry to `unavailable`, exactly `show`'s rule.

## 5. Cost, security, what is not built

**Cost:** two replays, or one replay plus the worktree scan `worktree-status` already does. Until RFC
136's anchors land that is `checkout`'s cost twice (depth^1.45); after them, O(tree + ≤64 patches)
each. The RFC 133 memory instrument gains a `diff` row at release prep like every replay surface.

**Security:** read-only; paths are rendered root-relative and never resolved on disk on the history
side; binary bytes are never printed and there is no flag that prints them; output size is bounded by
the text content of the two states, the same bound `checkout --content-path` already has. No new
trust decision: a `remotes/` ref diffs like any other, and diffing it is not adopting it.

**Not built, on purpose:** rename detection by similarity; word-level diff; `--stat`; external diff
drivers; applying a diff (diff output is a rendering, not an exchange format — bundles and sync are);
path-aware history (`log` filtering is a separate gap).

## 6. Increments and controls

1. **Two points.** The ref-or-block resolver (also wired into `checkout --ref`); the store's `diff.rs`
   reader over two replays; the line-diff module; prose and `diff-report-v1`. Controls: a fixture with
   one of each status; the rendered hunks applied by `patch(1)` to the left content reproduce the
   right content byte for byte (the renderer cannot lie); `--from X --to X` is empty; a block id and
   the ref that points at it give byte-identical output; a binary file shows ids and sizes only;
   an unsupported operation fails the call; `--path` filters output and the JSON `entries` count.
2. **The worktree.** Bare `prikk diff`, `--from` only; untracked and unsupported-path sections;
   `.prikkignore`. Controls: `prikk diff` on a clean worktree is empty; after editing, creating,
   deleting, `chmod +x` and `prikk mv`, each shows its status once; `commit --from-worktree` then
   `prikk diff` is empty again (what `diff` shows is exactly what `commit` would author).
3. **Docs and CHANGELOG.** `docs/src/guide/diff.md`, `commands.md`, `git-mapping.md`'s `git diff` row,
   `show.md`'s closing paragraph (the question is no longer open), `### Added — prikk diff`. A letter to
   stikk: their content-at-a-point dependency gains a two-point form.

Handoffs are written when the owner schedules the theme.
