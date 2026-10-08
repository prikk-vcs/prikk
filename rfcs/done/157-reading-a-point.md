# RFC 157 — Reading a point of history: a tree listing, and a file's bytes

**Status.** **ACCEPTED by the project owner 2026-09-17** (*"Yes. Both accepted. Write them."*). **The architect's
reading, stated so it can be corrected:** §2–§7 are accepted as written, including the verb names `tree` and `cat`,
the full-mode `mode` field (not planeter's `420`), `content_id` for binary files only, and the refusal of binary content
to a terminal. Handoff: `rfcs/handoffs/157-reading-a-point/tree-and-cat-handoff-v1.md`, after the shared resolver
(`rfcs/handoffs/153-content-diff/point-resolver-handoff-v1.md`).

*History:* **PROPOSED 2026-09-17 by the architect** for **0.46.0 "comparing"**, beside RFC 153 (`prikk diff`). It is the
design owed to planeter's read-path letters of 2026-09-16:
- A1, a machine-readable tree listing;
- A2, a file's bytes, text and binary.

planeter sent the JSON it would consume (`.git-exclude/upstream/planeter/receive/2026-09-16-reply-read-gaps-json-shapes.md`),
and those shapes are input here. prikk owns the final shape. **Scheduling and acceptance are the owner's.**
Author-review independence: the architect designs and will review; §7's controls compensate, each shown to fail.

## 1. What exists, measured on the released 0.45.0 asset

- **`checkout --patch-plan --ref <ref>`** replays a ref to its tip.
  - Prose lists `file: <path>` per result file.
  - `--format json` without `--content-path` has no listing at all, so a consumer cannot enumerate a tree
    machine-readably.
- **`checkout --patch-plan --format json --content-path <p>`** returns, per named path:
  - `mode` as the full file mode (`33188`, `33261`);
  - text as `{"kind": "text", "text": …}`, reconstructed by replay;
  - a non-UTF-8 file as `{"kind": "binary", "blob_id": …, "size": …}`, **never its bytes**;
  - an absent path under `not_found`.

  Whether a file is text follows UTF-8 validity: bytes `\0\1\2` are `text`.
- **A binary file's bytes cannot be read at all.** `show <blob-id>` refuses, and `show` on the block prints
  `<binary blob …, N bytes>`.
- **Edited text has no stored object** (DC-65). Its content exists only by replay, so "bytes by blob id" cannot serve
  text; "bytes by path at a point" can serve both.
- **A point is a ref only.** `--ref <block-id>` is an open row (RFC 144 §4t), and RFC 153 §2 closes it with one
  resolver.
- **Replay is anchored at checkpoints since 0.43.0** (RFC 136): O(tree + at most 63 patches), not O(depth).

## 2. One point resolver, shared with RFC 153

Every command in this RFC and in RFC 153 names a point as a **ref** (`heads/…`, `tags/…`, `remotes/…`) or a **bare
block id** (64 lowercase hex), through **one** function. It extends 0.45.0's `require_existing_ref`, and never becomes
a second resolver:
- an absent ref: `Precondition`, "ref <name> does not exist in this repository" (0.45.0's wording);
- a block id the store does not hold: `Precondition`, "block <id> is not in this repository";
- a received ref is **read**, like `log` and `merge-plan`. Reading is not adopting.
- The JSON echoes how the point was named, and the resolved `target_block_id`, so a consumer can cache by the block.

## 3. A1 — `prikk tree`

```
prikk tree [path] [--ref <ref|block-id>] [--prefix <p>] [--format json]
```

- **Lists the present leaf paths at the point** in canonical path order: files, and symlinks once replay supports
  them. **No directory entries.** prikk has paths, not trees, and planeter builds directories from segments.
- **Without `--ref`**, the current branch's tip, as `log` does. A fresh repository's unpublished branch lists nothing,
  with exit 0, the implicit-branch rule of the refusal sweep. An explicit absent `--ref` refuses.
- **`--prefix <p>`** filters by whole path components: `src/` matches `src/main.rs`, never `srcx/a`. The replay is
  whole-tree either way, and the help text says so.
- **`--format json`** is `tree-listing-v1`:

  ```json
  { "schema_version": "tree-listing-v1",
    "point": "heads/main", "target_block_id": "…", "prefix": "src/",
    "entries": [
      { "path": "src/main.rs", "kind": "file", "encoding": "text", "mode": 33188, "size": 1834 },
      { "path": "src/big.bin", "kind": "file", "encoding": "binary", "mode": 33188, "size": 240128,
        "content_id": "…" } ] }
  ```

  - `kind` is `file` or `symlink`, planeter's axis.
  - `encoding` is `text` or `binary`, checkout's axis. It is the same classification, so A1 and A2 agree.
  - `mode` is the full file mode, as checkout prints it. **Not planeter's `420`**: one number per repository, across
    every report.
  - `size` is **exact**, in bytes of the content at the point, reconstructed for text.
  - **`content_id` is present only for a binary file**: its stored blob id. It is absent for text, even when a
    checkpoint happens to store that content, so the listing never changes with snapshot cadence. A consumer uses
    it as a cache hint, never as a retrieval key.
- **Cost:** one anchored replay plus the text reconstruction `size` needs. The increment measures it against
  `checkout --patch-materialize` at depth, and the RFC 133 instrument gains a `tree` row. **If exact text sizes
  cost materially more than the replay itself, stop and report**; do not quietly approximate.
- **Not built:** server-side pagination or a cursor. planeter pages client-side in canonical order and bounds by
  prefix; the flat-directory case is recorded, not scheduled.

## 4. A2 — `prikk cat`

```
prikk cat --path <p> [--ref <ref|block-id>] [--output <file>] [--max-bytes <N>] [--format json]
```

**Bytes.** Without `--format json`, it writes **the file's bytes at the point**, text and binary alike, to stdout or
to `--output`:
- **All or nothing.** The content is fully resolved before one byte is written. A failure anywhere writes nothing.
- **`--max-bytes <N>`:** content larger than N refuses with `Precondition`, naming the size and the bound, and writes
  nothing, not even a partial file.
  - A binary blob's size is known from the store, before its bytes are read. **Amended 2026-09-22 (§4a): that is
    not what today's replay does.**
  - Text is compared after reconstruction; replay already holds it.
- **`--output <file>`:**
  - refuses an existing file, unless `--force` (the `bundle export` convention);
  - refuses a path inside `.prikk/`;
  - is written to a temporary sibling and renamed into place, so a killed process leaves no partial file.
- **A terminal is protected.** Binary content refuses to write to a stdout that is a terminal, naming `--output`, and
  writes nothing. This is the one security rule new to this RFC: history from an untrusted source must not be able
  to drive a terminal by being `cat`-ed. Text goes to a terminal as-is, as `checkout` already materializes it.
- A path absent at the point: `Precondition`, "path <p> does not exist at <point>". A directory prefix is not a file,
  and is refused the same way.

**Metadata.** `--format json` writes **no bytes**, only `path-content-v1`:
`{schema_version, point, target_block_id, path, kind, encoding, mode, size, content_id?}`, the same fields and rules
as one `tree-listing-v1` entry. A consumer decides between inline render and download, and shows a size, before
fetching.

**What `cat` is not:**
- It is not a second content surface. `checkout --content-path` stays as it is, for text in a plan.
- RFC 153 §5's "binary bytes are never printed" stays true **of `diff`**. `cat` is the one command whose job is
  bytes, it is bounded, and it refuses a terminal for binary.
- **Not built:** bytes by blob id (planeter will not depend on it; a candidate); ranges; streaming without the full
  content in memory. The bound is `--max-bytes`, and the replay's own memory is already O(tree text).

### 4a. Amended 2026-09-22 — what `--max-bytes` bounds, and what it does not

The `cat` round measured (`cat-report-v1.md` §2) that the shared replay materializes every file's content, binary
blobs included, while building its manifest. So by the time `cat` compares a size against `--max-bytes`, the bytes
are already in memory. **§4's "known from the store, before its bytes are read" does not describe today's replay.**

**Ruled, and this is what the flag promises:**
- `--max-bytes` is **exact and all-or-nothing for what is written**: when it refuses, nothing reaches stdout and no
  file is created or replaced. That is the guarantee §4 and §6 are about, and it is held by its controls.
- It is **not a bound on peak memory**. Peak memory is the replay's own O(tree) cost, which
  `checkout --patch-plan` already pays and this RFC does not change.
- The documentation says both, so a caller bounding a hostile input knows which guarantee it has.

**Making it a memory bound is RFC 158's Stage B** (streaming: chunk-wise reads, with the bound cutting before the
content is resolved). It is not a `cat` change, and it must not be done by teaching replay to skip a file's content —
the change §5a declined.

## 5. Failures

- The resolver's refusals (§2).
- An unsupported operation anywhere in the chain fails the whole call (RFC 140 §7b). Never a partial listing.
- Damage the store reports propagates as `Integrity`.
- A missing blob for a binary entry: **the whole call fails** (§5a). `cat` refuses.

### 5a. Amended 2026-09-21 — `unavailable` was unreachable, and the whole call fails

§5 first said `tree` marks such an entry `unavailable`, "the `show` rule". **That was the architect's error**, measured
by the dev team in `tree-report-v1.md` §5.1 and accepted:
- replay reads every file's blob (`patch_replay/apply.rs` → `read_blob_bytes_with_kind`), so a missing blob fails the
  replay before any entry exists to mark;
- a history naming an unstored blob **cannot be sealed** in the first place (`lifecycle replay: blob <id> required for
  a state effect is missing`);
- post-seal damage surfaces as the index's own `Integrity`, and `checkout --patch-plan` fails the same way.

**`show`'s rule does not transfer.** `show` renders one patch's own operand and can say `<unavailable blob …>` because
it never replays a tree; `tree` and `cat` report a whole state, and a state with a hole is not a state.

**Ruled:** a missing or non-recomputing blob **fails the whole call**, as an unsupported operation already does. No
entry-level `unavailable`, and no partial listing. A per-entry "unavailable" would have to be a replay-level state
shared by `checkout`, `bundle export` and `merge`, which is recorded as a candidate in the ROADMAP and is not this
RFC's.
- Usage errors (unknown flag, repeated `--format`, malformed block id) exit 2.

## 6. Security

- Read-only; no lock beyond the read snapshot replay uses; nothing adopted or trusted.
- Paths are rendered root-relative and never resolved on disk, except `--output`, which is the user's own path.
- `--max-bytes` gives a caller an all-or-nothing bound on untrusted content.
- Binary content never reaches a terminal (§4).
- No new format, no new object type, no new persisted state.

## 7. Increments and controls

**The resolver** (§2) comes first and is shared with RFC 153. It is one increment, reviewed once.

1. **`tree`.** Controls:
   - a fixture with text, binary, an executable and nested paths lists each once, in canonical order, with exact
     sizes (compared with materialized files) and `content_id` only on the binary;
   - `--prefix src/` excludes `srcx/`;
   - a block id and the ref pointing at it give byte-identical JSON;
   - a received ref lists;
   - an absent ref refuses, and the implicit fresh branch lists nothing with exit 0;
   - an unsupported operation fails the call;
   - the listing at a checkpoint and one block after it agree on unchanged entries: cadence does not show;
   - the RFC 133 `tree` row is recorded.
2. **`cat`.** Controls:
   - text and binary bytes equal the materialized file, byte for byte;
   - `--max-bytes` one below the size refuses with **no file and no stdout bytes**, and at exactly the size it
     succeeds;
   - `--output` over an existing file refuses without `--force`;
   - a process killed during `--output` leaves no file (failpoint);
   - binary to a pseudo-terminal refuses, and text to it is written (Unix; stated where Windows differs);
   - `--format json` writes no bytes and agrees with the `tree` entry;
   - an absent path and a directory prefix refuse.

   Perturb each guard: the bound checked after writing, the terminal check skipped, a non-atomic output.
3. **Docs and CHANGELOG:**
   - `docs/src/guide/tree-and-cat.md`, `commands.md`;
   - `git-mapping.md`'s `git ls-tree` and `git show <rev>:<path>` rows;
   - `### Added` for each command.

   The consumer letters for 0.46.0 carry the schemas: planeter's A1/A2, and stikk's content-at-a-point.

Handoffs are written when the owner accepts this RFC.
