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
