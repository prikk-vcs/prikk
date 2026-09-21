# RFC 153 — `prikk diff`: two points of history, or the worktree against its baseline

**Status.** **ACCEPTED by the project owner 2026-09-17** (*"Yes. Both accepted. Write them."*), as amended by §7, for
**0.46.0 "comparing"**. **The architect's reading, stated so it can be corrected:** the direction of §2–§6 is
accepted together with every §7 amendment, including dropping the `untracked` section and the fresh-repository rule. The
resolver is shared with RFC 157 and is handed off first. Handoffs:
`rfcs/handoffs/153-content-diff/point-resolver-handoff-v1.md`, then `diff-handoff-v1.md`.

*History:* **PROPOSED 2026-09-13 by the architect**, on the owner's word (*"Approved"*) to the
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

## 6a. Amended 2026-09-22 — the cost bound, and a missing blob

**A. A missing or damaged blob fails the whole call** (the `diff` Stage 1 report §6.1). §4's "absence of a blob
degrades that one entry to `unavailable`" is **withdrawn**: it is the same clause RFC 157 §5a already withdrew for
`tree`, for the same measured reason, and `diff` shares that replay and that entry emitter. A per-entry state would
have to be a replay-level change shared by every reader, which is a ROADMAP candidate and not this RFC's.

**B. `content_id` is binary-only**, as RFC 157's shared emitter gives it. §3's "blob ids where they exist" is read
that way: a text side carries its size and mode, not an id, because its content is the diff itself. Changing that
would change `tree`'s schema too, and is not a `diff` option.

**C. The line diff gets a deterministic work bound.** Measured (`diff-two-points-report-v1.md` §2, reproduced by the
architect on a second machine): the two shapes this RFC's increment named finish in 8–16 ms, but two ordinary shapes
do not — 32,768 shared lines in reverse order took 2.3 s and 5.0 s on the two machines, and two files drawn from a
50-line alphabet took 14.5 s and 28.0 s. The cost is O((N+M)·D) and those inputs are not exotic.

**Ruled:**
1. **The bound counts work actually done** — search steps inside the middle-snake loop — not a wall-clock budget and
   not the nominal `(N+M)·D`. Time is not reproducible across machines, and the nominal figure is wrong after the
   prefix/suffix and unique-line reductions: it reads 4.4e10 for a case that finishes in 16 ms.
2. **One named, documented constant**, calibrated so the worst case stays well under a second on the slower machine
   measured (about 2.3 ns per step there). Its value is the increment's to propose with its own measurement.
3. **Above the bound the search stops and emits a valid, non-minimal script** for the region it had not resolved
   (delete the remaining left lines, insert the remaining right ones). **Correctness is unchanged**: applying the
   hunks still reproduces the right side byte for byte, which is what the renderer promises.
4. **The reader is told.** `diff-report-v1` carries a per-entry `minimal` boolean, and the prose says so on that
   entry. A consumer must never have to guess whether it received the shortest script.
5. **Determinism is the property that replaces minimality** when the bound engages: the same two inputs give the
   same output on every machine and every run. That is a control, not a hope.
6. **§6's "shortest edit script" wording** now reads: shortest within a documented work bound, and a valid script
   beyond it.

## 7. Amended 2026-09-17, for 0.46.0 — what 0.43.0 to 0.45.0 changed under this design

Proposed again for **0.46.0 "comparing"**, beside RFC 157 (a tree listing and a file's bytes at a point), which
shares its resolver. Acceptance and scheduling remain the owner's. Checked against the released 0.45.0 asset.

1. **One resolver, extended rather than added (§2).** 0.45.0 shipped `require_existing_ref`, the refusal sweep's
   shared absent-ref check. The ref-or-block resolver **extends that function**, and RFC 157 uses the same one.
   - An absent ref refuses with 0.45.0's wording, "ref <name> does not exist in this repository".
   - A block id the store does not hold refuses with "block <id> is not in this repository".
   - A received ref is **read**, as `log` and `merge-plan` read one since 0.45.0.
2. **No `untracked` section (§3). This corrects a contradiction in this RFC.** prikk has no index: `commit` authors
   every worktree file `.prikkignore` does not exclude. A new file is therefore **`added`, with its content**, which
   is exactly what §2's "what `commit` would author" means. §6.2's control ("commit, then diff is empty") holds only
   this way.
   - `untracked[]` is dropped from `diff-report-v1`.
   - Ignored paths are not shown.
   - `worktree-status` calls these same new files "untracked" (its scan, `guide/ignore.md`). `diff`'s guide page
     says that an untracked file there is an `added` entry here, because `commit` authors it (measured on 0.45.0:
     a new file is authored by a plain `commit -m`).
   - RFC 147 §3f's unsupported paths keep their section.
3. **A fresh repository (§2).** With no `--from`, the left side is the current branch's tip. When that branch is
   unpublished (a fresh repository), the left side is the **empty state**: bare `prikk diff` shows every file
   `commit` would author as `added`, with exit 0. That is the implicit-branch rule of the refusal sweep. An
   explicit `--from` naming an absent ref refuses.
4. **Statuses that history can now produce (§3).**
   - **`renamed`** arises across merges since 0.45.0 merges renames.
   - **`symlink` remains unreachable from history:** replay does not apply symlink operations, so a chain holding
     one fails the call (§4). On the worktree side a symlink is an unsupported path. The status stays in the
     schema for when replay supports symlinks.
5. **Cost (§5).** Anchors landed in 0.43.0: each side is O(tree + at most 63 patches), not depth^1.45. The RFC 133
   instrument gains a `diff` row.
6. **Bytes (§5).** "Binary bytes are never printed" stays true **of `diff`**. RFC 157's `prikk cat` is the one
   bounded command whose job is bytes. `diff` shows a binary entry's ids and sizes, and names `cat` as the way to
   read either side.

**Increments are unchanged**, except that the resolver is its own first increment, shared with RFC 157. §6.2's
controls gain: a fresh repository's bare `diff` lists every file as `added`, and an ignored file never appears.
