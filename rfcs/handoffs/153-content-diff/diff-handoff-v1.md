# `prikk diff` — handoff v1

**Order of work for 0.46.0 (the three handoffs of 2026-09-17):**
1. `153-content-diff/point-resolver-handoff-v1.md`;
2. `157-reading-a-point/tree-and-cat-handoff-v1.md`;
3. `153-content-diff/diff-handoff-v1.md` (this one).

Take them one at a time, each reported and reviewed before the next. **This round starts after `tree` and `cat` are
reviewed**, and uses the shared resolver and `tree`'s entry code.

**Live 2026-09-17.** RFC 153 (`rfcs/accepted/153-content-diff.md`) is the specification **as amended by its §7**. §7
overrides §2–§6 where they differ:
- no `untracked` section, since a new worktree file is `added` with content;
- the fresh-repository rule;
- received refs read;
- the shared resolver;
- `cat` is the bytes command.

Read both parts first.

## Stage 1 — two points (RFC 153 §6.1), then report

1. **The line-diff module.** A pure module in the store: a shortest edit script over lines, deterministic, with no
   dependency and no external tool, rendering unified hunks with **3 lines of context**.
   - A file without a trailing newline is marked the way `diff -u` marks it, so `patch` round-trips.
   - **Measure the worst case before wiring it:** a 1 MiB text file rewritten line by line, and one with every other
     line changed. **If either takes more than 1 s in a release build, stop and report** with a proposed bound. Do not
     invent a cap.
2. **The reader:** two resolved points through the shared resolver and the block-addressed anchored replay, one read
   snapshot for both.
   - **Statuses:** `added`, `deleted`, `modified`, `renamed` (declared `RenamePath` only), `mode`, `binary`. `symlink`
     stays in the schema and is unreachable (§7.4).
   - **Binary:** ids and sizes only. The prose names `prikk cat` for reading either side.
   - **Missing blob:** that entry is `unavailable`.
   - **Unsupported operation:** the whole call fails.
3. **Output:** prose and `diff-report-v1`, per §3 as amended. `from`/`to` each carry how the point was named and its
   block id; `entries[]`; `unsupported_paths[]`; **no `untracked[]`**. `--path` filters the output, and the JSON
   `entries` count follows it.

**Stage 1 controls** (§6.1), each shown failing under its perturbation:
- a fixture with one of each reachable status;
- **the renderer cannot lie:** the rendered hunks, applied to the left content, reproduce the right content byte for
  byte. Apply them with a small hunk applier in `tests/support` on every platform, **and** with `patch(1)` on Unix
  when it is present;
- `--from X --to X` gives an empty report with exit 0;
- a block id and the ref pointing at it give byte-identical output;
- a binary file shows ids and sizes only;
- an unsupported operation fails the call;
- `--path` filters both prose and `entries`;
- a received ref diffs;
- an absent ref, and a block id not held, refuse.

**Report:** `.git-exclude/review-request/diff-two-points-report-v1.md`. **Stop there.**

## Stage 2 — the worktree (RFC 153 §6.2 with §7.2 and §7.3)

- **Bare `prikk diff`** compares the current branch's tip with the worktree. `--from` alone does the same from that
  point.
- **New files are `added`, with content, and ignored paths never appear.** RFC 147 §3f's unsupported paths are named
  with `commit`'s refusal and no content.
- **A fresh repository**, whose branch is unpublished, has an empty left side: every file `commit` would author is
  `added`, with exit 0. An explicit `--from` naming an absent ref refuses.
- **Read-only:** no lock beyond the read snapshot, and no write, including while the worktree is provisional.

**Stage 2 controls:**
- **Clean worktree:** a clean worktree gives an empty report.
- **Each status once:** after an edit, a create, a delete, `chmod +x` and `prikk mv`, each shows its status once.
- **The round-trip:** `commit` then `prikk diff` is empty again, which is the RFC's central claim.
- **Fresh repository:** its bare `diff` lists every file as `added`.
- **Ignored files:** a `.prikkignore`d file never appears.
- **`worktree-status` agreement:** its "untracked" files appear here as `added` (the §7.2 wording in the guide).

**Perturbations:**
- an `untracked` bucket reinstated;
- ignore rules skipped;
- the left side of a fresh repository refused.

## Stage 3 — docs and CHANGELOG (RFC 153 §6.3)

- `docs/src/guide/diff.md`, including "untracked in `worktree-status` is `added` here";
- `commands.md` and the synopsis;
- `git-mapping.md`'s `git diff` row;
- `show.md`'s closing paragraph (the question is no longer open);
- CHANGELOG `### Added — prikk diff` with `diff-report-v1`;
- an RFC 133 memory instrument row for `diff`.

The letters to stikk and planeter are the architect's, at the 0.46.0 release.

**Report for Stages 2 and 3:** `.git-exclude/review-request/diff-worktree-report-v1.md`.

## Addendum 1 2026-09-22 — Stage 1 accepted; the cost bound ruled; Stage 2 proceeds

**Accepted** (review `diff-two-points-review-v1`): `5457a931`. Gates 14/14 (2,226 / 0 / 30 per toolchain), confirmed
by the architect on the same commit. **RFC 153 gains §6a**, which carries every ruling below.

**The four defects your own controls and gates found in your work (§4.1–§4.5) are the round's best evidence.** The
binary hint naming a movable ref, and an applier that ignored hunk headers so a header perturbation passed a
20,000-case property test, are exactly the failures a green suite hides. Tightening the instrument until the
perturbation bit is the rule working.

### 1. The cost bound (report §2, RFC 153 §6a C) — ruled, and it is Stage 2's first item

**Reproduced by the architect** on a second machine: reverse-order 32,768 lines **5.0 s** (yours 2.3 s), two files
over a 50-line alphabet **28.0 s** (yours 14.5 s). Your reading is right: the two shapes the handoff named are cheap
only because the unique-line reduction empties them, and that is not evidence about the algorithm.

**None of your four options as written.** Option 2 is closest, but the bound must be shaped to survive measurement:

1. **Count work actually done** — search steps inside the middle-snake loop — **never wall-clock, and never the
   nominal `(N+M)·D`.** Time is not reproducible across machines (5.0 s here against 2.3 s there); the nominal figure
   is wrong after your reductions, reading 4.4e10 for a case that finishes in 16 ms. A cap on it would refuse the
   required shapes.
2. **One named, documented constant.** Propose its value **with your own measurement**: the worst measured shape
   should stay well under a second on the slower machine, using your ns-per-step figure. Say how you derived it.
3. **Above the bound, stop searching and emit a valid non-minimal script** for the unresolved region (delete the
   remaining left lines, insert the remaining right ones). Applying the hunks must still reproduce the right side
   byte for byte — that promise does not bend.
4. **Tell the reader:** a per-entry `minimal` boolean in `diff-report-v1`, always present, and a note in the prose on
   that entry. A consumer must never guess whether it got the shortest script.
5. **Determinism replaces minimality above the bound** and is a control: the same two inputs give the same output on
   every run and every machine.

**Controls (each shown failing):** a shape above the bound renders non-minimally, is marked, and still applies to
give the right side; a shape below is still minimal (your property test, with its minimality assertion scoped to
under-bound inputs); determinism across runs. *Perturbations:* make the bound time-based (determinism fails); let the
fallback emit an invalid script (the applier control fails); drop the `minimal` marking (its own control fails).

### 2. The other questions

- **§6.1 — you implemented the right thing.** RFC 153 §4's `unavailable` clause is withdrawn in §6a A; the whole call
  fails, as RFC 157 §5a already ruled for `tree`. No code change.
- **§6.2 — confirmed** (§6a B): `content_id` is binary-only, from the shared emitter. A text side is its own content.
- **§6.3 — correct.** Requiring both points in Stage 1 and refusing with "not built yet" is right; Stage 2 changes it.
- **§6.4 — your readings are confirmed**, and two controls are owed in Stage 2: a multi-hop rename A → B → C (one
  `renamed` entry, A paired with C), and a node deleted and recreated under a new node id (`deleted` + `added`, never
  `renamed`). `--path` selecting a renamed entry by **either** path is confirmed and goes in the guide.
- **§6.5 — not yours to have built.** The two-replay cost is Stage 3's RFC 133 `diff` row, as the handoff says.
- **§7 — `unsupported_paths[]` stays in the schema, always empty until Stage 2 gives it a producer**, and Stage 3's
  guide says so plainly rather than leaving a reader to wonder.

### 3. Unchanged for Stage 2

Everything in the original Stage 2 and Stage 3 lists, plus the cost bound (item 1) first, since the worktree side
inherits the same call.

**Report:** `.git-exclude/review-request/diff-worktree-report-v1.md`.

## Addendum 2 2026-09-22 — the round is accepted; one fix before the 0.46.0 cut

**Accepted** (review `diff-worktree-review-v1`): `5609efc6`. Gates 14/14 confirmed by the architect on that
commit (2,247 / 0 / 30 per toolchain), and the probes agree with the report on every status, the round trip, the
fresh repository, ignore, the three refusal shapes (byte for byte against what `commit` prints), the refused
declaration, read-only, and the bound's marking, determinism and `patch(1)` round trip. **RFC 153 gains §6b**,
which carries the five rulings of your §8.

**Two things the architect did in the record commit, so you do not:** `worktree-status.md`'s two false sentences
(§7.1 — you were right not to touch them), and `guide/diff.md`'s line about `--from` on another branch pairing
renames only where node ids agree.

**Your own perturbation discipline is what made W10 visible.** A perturbation that goes green means *either* the
control is weak *or* another layer is holding — and only reading the other layer says which. That is the
generalisation worth keeping from this round.

### 1. The one required fix — the `cat` hint can name a block that is not the side it describes

On a bare `prikk diff` whose branch has queued, unsealed commits, the left side is the tip **plus** the queue,
which **no block names**. The binary hint prints the sealed tip's block id, and running the command it prints
returns different bytes from the side the entry describes. Measured by the architect:

```text
from: heads/main (block ae705d5f…, plus 1 queued commit not yet sealed)
binary b.bin (binary: 8 bytes, id 7601f307… -> 10 bytes, id 58eca2c4…)
    prikk cat --path b.bin --ref ae705d5f…   <- the sealed tip's 6 bytes, not the 8-byte side above
```

This is the class Stage 1 already caught once (a hint naming a movable ref): **a hint must reproduce the state its
report describes, or it must not be printed.**

- **Fix:** when the left side carries queued patches (`queued_patches` greater than zero), print no
  `cat --ref <block>` line for it. Print instead one line saying the left side includes N queued commits that no
  block names, so it can be read only after `seal`. The right-side and two-point hints are unchanged.
- **Control:** a binary entry in that state prints no `--ref` line for the left side, and the line it prints
  instead names the queue. *Perturb:* print the sealed tip's block anyway — the control must go red.
- **The two sentences that promise it, in the same commit:** `guide/diff.md`'s "with the **block** each side
  resolved to", and the CHANGELOG's "(with the block each side resolved to)".

### 2. One sentence in `WORK_BOUND_STEPS`'s doc comment — the machine factor, withdrawn

The architect re-ran your timing instrument in a release build on the second machine: **2,711 ms** for the
reversal shape against your 2,730 ms, and **14,472 ms** against your 14,750 ms. The two machines are within a
couple of percent, not 2.17× apart; the Stage 1 figures (5.0 s, 28.0 s) were taken while that machine was busy,
and reading them as hardware was the architect's error. The worst shape costs **221 ms at the bound** there, so
the constant is about twice as conservative as its derivation says.

**Do not change the value.** Replace the 2.2× step of the derivation with what is now measured on both machines
(~5 ns/step, worst shape ~220 ms at the bound on each), so the next person does not re-derive from a factor that
is not there.

### 3. Nothing else

Stage 2 and Stage 3 are done, and `guide/diff.md`, `commands.md`, `git-mapping.md`, `show.md`, `ignore.md`, the
CHANGELOG and the two RFC 133 rows are accepted. Not yours, as always: `rfcs/`, `ROADMAP.md`, `MILESTONES.md`, the
letters, any push or tag.

**Report:** `.git-exclude/review-request/diff-hint-and-bound-note-report-v1.md`. This closes RFC 153; the
0.46.0 release prep follows, on the owner's word.
