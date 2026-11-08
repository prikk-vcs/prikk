# DC-56 Commit-Index Cache — Validity Specification v1

Required by `rfcs/accepted/DC-56-COMMIT-FULL-TREE-SCAN-COMPLIANCE.md` acceptance criterion 2 and the
owner ruling of 2026-07-30: the steady-state reading of NFR-PERF-01 is conditional on this document
existing and being followed, not merely on an index existing. This is the specification; the
implementation is `crates/prikk-store/src/commit_index.rs`.

## 1. What the index is

One file, `cache_dir()/commit-index.v1` (`.prikk/cache/commit-index.v1`), holding a map from
repository-relative path to the state that path's content hash was last computed against:

| Field | Meaning |
|---|---|
| `size` | File size in bytes at last read |
| `mtime_secs`, `mtime_nanos` | Modification time at last read, full precision available from `stat`/`statat` |
| `mode` | Normalized worktree mode (`REGULAR_FILE_MODE` or `EXECUTABLE_FILE_MODE`) at last read |
| `kind` | The `BlobKind` (`Text` or `Binary`) the content hash was computed under |
| `content_hash` | The `ObjectId` that content would produce as a blob of `kind` — the same formula `EditText`/`ReplaceBinary`/`CreateFile` comparison already used before this increment |

This is per-file state sufficient to skip a read, not a path-membership list — DC-56's acceptance
criterion 3 requires exactly this; a path-only index would still force a read of every file to know
whether it changed.

## 2. The trust condition — when an entry may be used without a read

An entry for path `P` may be trusted, and `P`'s content read skipped, if and only if **all** of:

1. A stat of `P` (size, mtime, taken without opening the file) exactly equals the entry's recorded
   `size`, `mtime_secs`, and `mtime_nanos`.
2. The entry's `kind` equals the `BlobKind` the current comparison needs (`Text` for an existing
   `TextFile` node, `Binary` for an existing `BinaryFile` node).
3. **(0.57.0 step 0b, 2026-11-08) The entry's recorded `content_hash` equals the node's baseline blob.**
   An entry is trusted unread only to say "unchanged". An entry that says the file differs is a miss:
   the file is read, as it would be anyway to author the edit, re-hashed, and the entry rewritten. So a
   damaged or stale hash can never send an unchanged file to the edit planner, which was the 0.55.0
   and 0.56.0 `integrity error: EditText requested for unchanged text` (external review 046 §4). The hot
   path is unchanged: an unchanged file's entry equals its baseline blob.

Implemented as `CommitIndexEntry::matches_stat` (condition 1), plus a direct `kind` equality check
(condition 2) and the baseline comparison (condition 3) at the call site, in `node_authoring.rs`'s
`resolve_existing_file`.

**What condition 3 does not cover** is an entry that hides a real rewrite (same size and mtime, and a
recorded hash that still equals the baseline). That remains §5's accepted residual, detected by
`verify` (§6) and now reported by `doctor` (`PRIKK-DOCTOR-COMMIT-INDEX-DIVERGENCE`).

**Mode is deliberately excluded from the trust condition.** A `chmod` with no content change (e.g.
`chmod +x`) does not update `mtime` on Linux — only `ctime`, which this cache does not track. Making
mode part of the trust gate would force an unnecessary content read on every permission-only change.
Mode is instead compared directly, every commit, from the cheap stat already taken for path `P` —
never from the cache — so a permission change is always detected regardless of cache state.

If any condition fails, the path is treated exactly as if it had no cache entry at all: the file
is read, its hash recomputed, and the entry is replaced with the freshly observed `(size, mtime,
mode, kind, content_hash)`.

## 3. What invalidates an entry

- **Any stat mismatch** (size or mtime differs from the entry) — the ordinary, expected case for a
  file the user actually edited. Not an error condition; this is the mechanism working as intended.
- **A kind mismatch** — the entry was computed under a different `BlobKind` than the current
  baseline expects for that path. Can only arise if the same path was hashed as a new file under one
  interpretation and is later compared against a baseline node of the other kind; existing-node kind
  is authoritative and never reclassified (E4), so this forces a real read rather than trusting a
  hash computed under the wrong kind.
- **The path is absent from the current worktree walk** — `CommitIndex::retain_paths` drops it before
  the index is persisted, so a later, unrelated file created at the same path can never inherit a
  stale entry through a coincidental stat match.
- **The whole index file is missing, unreadable, or fails to parse** (wrong magic, wrong field count,
  a field that doesn't parse) — `CommitIndex::load` fails open to a fully empty index. This is not
  treated as a repository integrity error: the index is rebuildable and never a root of trust
  (`specs/prikk-non-functional-requirements-v1.1.md` §3, NFR-PERF-04's traceability gloss). An empty
  index costs exactly one full read per path on the next commit, then rebuilds warm from there.

## 4. What bounds rebuild frequency

**Per path, at most one content read per commit; zero when unchanged.** Across the lifetime of a
repository:

- The **first commit ever** against a repository (or the first commit after the index is deleted,
  corrupted, or otherwise falls open to empty) reads every present file's content exactly once — the
  same cost `commit` always had before this increment. This is the owner ruling's exemption:
  NFR-PERF-01 bounds **steady-state** cost, not every commit including the first.
- **Every subsequent commit**, so long as the index survives, reads only the files whose stat has
  actually changed since they were last hashed — in the common case, exactly the files the user
  edited, and nothing else. A commit that changes 1 file out of 10,000 reads 1 file, not 10,000.
- There is **no unbounded cold path**. The only way an entry stops being trusted is a real stat
  change on that specific path, or a whole-index invalidation event (deletion/corruption), both of
  which are one-time, path-scoped or index-scoped costs — never a recurring per-commit full scan
  while the index remains intact. This is the condition the RFC's §2 obligation exists to rule out:
  "a design that scans whenever the cache happens to be cold, with no bound on how often that is,
  satisfies the letter and defeats the requirement."

The metadata-only directory walk (`enumerate_worktree_files` → `list_directory`/`stat_file_state_if_exists`)
still runs every commit, at every repository size — this is unchanged by DC-56 and is not exempted
by the steady-state reading. It is `readdir` plus one `stat` per entry, never a content open or read,
and DC-59's re-run (see the benchmark report) is the empirical evidence that this remaining cost does
not reproduce the growth NFR-PERF-01 forbids.

## 5. The accepted residual risk, and why it is safe to accept

The trust condition is a **stat heuristic**, not a content guarantee. Two failure modes are possible
in principle:

- **mtime granularity.** Some filesystems record mtime at one-second resolution. Two writes to the
  same file within that window, at the same resulting size, could produce a stat that appears
  unchanged from the cache's perspective when content in fact changed twice.
- **Clock skew or a misbehaving filesystem.** A `stat` that reports a value inconsistent with the
  file's true history (backdated mtime, etc.).

In either case, `commit` would silently treat a genuinely changed file as unchanged, omitting a real
change from the resulting patch. **This is exactly the risk RFC §5 (DC-56) identifies and is the
reason the index may not be silently wrong** — acceptance criterion 6 requires this be a *detectable,
reported* condition, not an undetectable one.

**Why the design accepts the heuristic rather than eliminating it:** eliminating it entirely would
mean always reading every file's content on every commit to verify the hash — which is precisely the
full-tree read NFR-PERF-01 exists to remove. There is no way to guarantee correctness of a
skip-the-read optimization without occasionally paying the cost of *not* skipping it somewhere. The
design's answer is to move that unavoidable cost out of the hot commit path and into an explicit,
occasional, opt-in check: `prikk verify`.

## 6. Divergence detection (criterion 6)

`prikk verify` — `RepositoryVerification::commit_index_divergences` — performs the check `commit`
deliberately does not: for every path with a commit-index entry whose recorded stat still matches
the file's current stat (i.e. every entry `commit` would currently trust), it reads the file's actual
current bytes, recomputes the content hash under the entry's recorded `kind`, and compares. A
mismatch is reported as a `CommitIndexDivergence { path, recorded_hash, actual_hash }` — the index
disagreeing with the worktree, exactly per §5's requirement that this be a *reported* condition.

Entries whose stat no longer matches are **not** re-read by this check: a stat mismatch already means
the next `commit` will re-read that path itself, which is the ordinary, expected, non-divergent case
(the user edited the file and hasn't committed yet). Flagging every currently-edited worktree file as
"divergent" would be false-positive noise, not the failure mode this check exists to catch.

This check necessarily reads every file whose entry currently passes the trust condition — in a
freshly committed, otherwise-idle repository, that is most or all tracked files. This is acceptable
because `verify` carries no latency bound analogous to NFR-PERF-01; it is an explicit, occasional,
read-only operation, never part of the commit hot path.

## 7. Deletion and rebuild (criterion 7, NFR-PERF-04's evidence obligation)

Deleting `cache_dir()/commit-index.v1` and then committing must produce a result **behaviorally
identical** to committing with the index intact: the same operations, the same patch, the same
`ObjectId`s. `CommitIndex::load` falling open to empty on a missing file, and every path then being
read fresh exactly as the first commit against a repository would, is what makes this true by
construction rather than by a special case — there is no "index present" code path that differs in
outcome from the "index absent" code path, only in how much is read to get there. The evidence test
is `crates/prikk-cli/tests/dc56_commit_index.rs::deleting_the_index_does_not_change_commit_outcome`.
