# An object append that does not read its container — RFC 102, handoff v1

**Live 2026-09-26, and it is next**, before 0.48.0 release prep. **The owner ruled it into 0.48.0** (2026-09-26:
*"Yes. The fix should be put in 0.48.0."*). 0.48.0 is not cut without it.

**Read first:**
- `rfcs/accepted/102-container-based-durability.md`, its last section, `MEASURED and RULED 2026-09-26`;
- `.git-exclude/reviewed/measurement-budget-review-v1.md` §5, with the architect's measurements.

## The defect, in one paragraph

`foundation/index.rs::append_object_to_container` takes the new record's offset as
`read_file_if_exists(…container…)?.map_or(0, |bytes| bytes.len())`. That reads **the whole container into memory on
every object append**, only to learn its length. Two consequences follow:
- **Bytes read grow quadratically.** A first commit of 4,000 × 20 KB files reads 162 GB.
- **Peak memory tracks the container.** A commit that adds one small file peaks at about the blob container's size:
  265 MiB against 256 MiB.

Offsets are correct today. Only the cost is wrong.

## 1. What lands

1. **The offset from a stat, not a read.** Nothing about its meaning changes: it is the container's length
   immediately before the append, inside the same object-store lock hold.
   - **Preferred:** the length of **the descriptor the record is appended to**. Open it once, `fstat` it, append to
     it, and make it durable, so the length and the write name one file.
   - **Acceptable** only if that shape cannot be had on every platform: an anchored stat of the same path
     (`stat_file_state_if_exists`), under the lock, before the append. The report says which was used and why.
   - Keep every property of today's path:
     - the final component is never followed through a symlink;
     - a non-regular file is refused, and a FIFO is never opened;
     - a missing container is refused by the append, as `append_file_required` refuses it today;
     - a torn tail from an interrupted append counts in the length, exactly as the read counted it.
   - Linux, macOS **and Windows** (`fsutil/anchored/windows.rs`).
   - The funnel and its lock stay where they are. `every_object_append_goes_through_the_locked_wrapper` stays green,
     unchanged.
2. **The sweep, RFC 102 ruling 2.** Find every per-append or per-write read whose size grows with the store. Known
   candidates:
   - `FileObjectStore::write_object`, which re-decodes the whole index per write through `lookup_object_location`;
     name its production callers;
   - `wal.rs:276`'s length read;
   - anything else the sweep turns up.

   For each: its callers, and **its share of the bytes read in unit M1 after item 1**. Fix a site in this round only
   if its fix is as mechanical as item 1. Report the rest for a ruling.
3. **No format change and no change to any output.** A history written by this commit and one written by 0.47.0,
   from the same inputs, produce the same index entries (object id, type, slot, offset, length, checksum).

**Not in this round:** the index format; the lock span; `rebuild_index_from_containers`; anything under
`rfcs/accepted/158-*`.

## 2. Controls — landed, each shown red

1. **An append does not read its container.** The test must go red when the whole-container read at
   `index.rs:555` is put back. The mechanism is yours: a `cfg(test)` read counter on the container path, a
   `test-support` probe, or anything else that watches the **file** store, not a memory store.
2. **Offsets unchanged.** After a run of appends of mixed object types, every index entry's offset equals the sum of
   the preceding records' lengths in its container, and `verify` is clean. **Perturb:** offset + 1. It goes red.
3. **A torn tail.** Append a few garbage bytes to the end of a container, then append an object. The new entry's
   offset is the container's full length, torn bytes included, exactly as today. `verify` and
   `doctor --repair-index` behave as they do today. **Perturb:** take the length only up to the last complete frame.
   It goes red.
4. **Refusals unchanged.** Replace the container with a symlink, with a directory, and with a FIFO: each append is
   refused as today, and the FIFO is never opened. A missing container is refused. **Perturb:** follow the final
   component. The symlink case goes red.
5. **The race controls stay green:** `two_racing_object_appends_serialise_and_never_share_an_offset` and the held-lock
   test.

## 3. Measurement — units and budgets, up front

Release builds only. **This commit's** binary against **0.47.0's** (the downloaded asset, or a release build of
`21895f46`), with sha256 and `opt_level` for each. Each unit runs under the watcher, which records per-step elapsed
time and the boot id, and **stops itself at twice its budget**. Nothing of the architect's runs meanwhile.

| unit | what | samples | budget (stop at ×2) |
|---|---|---|---:|
| **M1** | bytes read (`rchar`, the parent's `/proc/self/io` around the reaped child), read syscalls and wall time of a **first commit** of 1,000 / 2,000 / 4,000 files × 20 KB, and of 8,000 files × 40 B | one per point per binary, alternating | **15 min** |
| **M2** | peak RSS (`getrusage`) of a commit that adds **one small file** to a repository whose blob container is 8 / 64 / 256 MiB (4 MiB random files) | three per point per binary, alternating | **10 min** |

The architect's probes are a starting point, not the instrument: `/home/nabbisen/.pgtmp/arch-seal/commit_io_scale.py`,
`container_rss_probe.py`, and `commit_fd_probe.py`, which names the files a running commit holds open. Copy them;
don't edit them in place. They read the fixed test seeds beside them.

**The acceptance figures:**
- **M1:** bytes read are **linear** in N: at 4,000 × 20 KB, a small multiple of the worktree plus the containers'
  final size, against 162 GB today. Any term still growing faster than linearly is named, with its share and its
  source site (item 2).
- **M2:** peak RSS of the one-small-file commit **does not follow the container**. At 256 MiB it stays within 10 % of
  the 8 MiB case.

**Not run in this round:** the RFC 133 release-gate profile and the full sweep. 0.48.0 prep runs the profile. This fix
will lower its absolute figures, and prep has to explain any move in the ratio against 0.47.0's re-baselined 1.916×.
Also not run: corpus-depth histories.

## 4. Docs and CHANGELOG

- **CHANGELOG** (`### Fixed`): every object write read its whole object container to learn its length. A commit's
  memory therefore grew with everything the repository stored, and a first commit or import of many files read
  quadratic bytes. Give the release figures from M1 and M2.
- **Any page that states commit memory or import cost** (`current-state.md` first) says what changed, with M2's
  figure.

## 5. The report

`.git-exclude/review-request/append-length-without-reading-report-v1.md`:
- the gates on the exact final commit;
- each control with the perturbation that turned it red;
- the sweep's table: site, callers, M1 share, fixed or reported;
- M1 and M2 with boot id, load at start and binary hashes;
- anything in this handoff that is not true at source.

**Windows:** local gates compile Windows but run none of its tests. The architect pushes the fix and reads the
**Windows mutation test suite** by name. The round closes only when that job is green.

## Addendum 1 — 2026-09-26: S-2 kept with a required bound, site C ruled in, and the class change accepted

Report: `.git-exclude/review-request/append-length-without-reading-report-v1.md`; review
`.git-exclude/reviewed/append-length-without-reading-review-v1.md`. **Item 1 is accepted as delivered.** The handoff's
account of M2 was the architect's error: the one-small-file commit's peak came from its **reads** (site B) as well as
the append. You found that and said so.

**Not pushed yet.** Nothing of this round goes to `origin` until item 1 below lands, because S-2 as committed aborts
`prikk verify` on a damaged container.

1. **Required: bound the ranged read (S-2 is kept, on this condition).** `read_file_range_if_exists` allocates
   `vec![0_u8; len]` **before** reading. `len` is the frame header's `body_len`, a `u64` taken from disk. The architect
   measured it on a release build of `6edb17b5`:
   - an unsealed repository, its one blob's header `body_len` set to 2⁶², then 2⁴⁰;
   - `prikk verify` **aborts**: `memory allocation of 4611686018427387954 bytes failed`, exit 134;
   - 0.47.0's asset on the same repository exits 0, with no abort.

   The whole-file read that S-2 replaced was bounded by the file's real size. The positioned read must be too. **Both of
   these land:**
   - **(a) In every reader** (Posix, Windows, path-only): never allocate more than the file holds from `offset`. Clamp
     `len` to the length from `fstat` (or the Windows handle's metadata) minus `offset` before allocating. Confirm the
     path-only reader's `take(len).read_to_end` reserves nothing from `len`, and say so.
   - **(b) In `read_object_envelope_at`:** the frame length the header claims must equal the index entry's `length`
     (both are header plus body, as `append_object_to_container` writes them). A mismatch is an `Integrity` error naming
     the object id and the container's offset, returned **before** the frame read.

   **Controls, each shown red:**
   - the reader with `len = usize::MAX / 2` on a 10-byte file returns the 10 bytes;
   - an object read whose header claims 2⁶² returns an `Integrity` error, and so does one whose header disagrees with
     its index length;
   - **the CLI control:** a **sealed** repository with a blob header claiming 2⁶² makes `prikk verify` report the
     damage and exit non-zero, with no abort. State what 0.47.0 reports on the same repository.

   A perturbation that removes the clamp may abort the test binary. That is acceptable as red if the report shows the
   suite failing. Run such a case in a child process if you prefer.
2. **Site C: option (A), in this round.**
   - `replay_index_tail_with_extent` reads only `[start_offset, current length)` with the bounded positioned read, and
     decodes it with the offsets it reports shifted by `start_offset`. Every result must equal today's
     `decode_index_records(&whole, start)`: entries, `trailing_partial_bytes`, `record_outcomes` offsets and `extent`.
   - **A file shorter than `start_offset`** (repaired or truncated under the session) takes exactly the path it takes
     today. Show that path with a control.
   - The decision stays stat-then-decode, and RFC 111's decode counter counts a tail decode as it does now.
   - **Option (B), trusting the session's own append length, is refused**: it re-opens the B1 question RFC 111 closed.
   - **Controls:**
     - equality with the whole-file decode over a run of appends, **including a torn index tail and a truncated file**;
       perturb the shift by one and it goes red;
     - the pinned-digest control `a_fixed_run_of_appends_writes_the_same_index_and_containers_as_before` stays green;
     - the read tally shows a session's own write reading only the new index bytes; put the whole read back and it
       goes red.

   **Scope, as the architect reads the owner's ruling:** *"The fix should be put in 0.48.0"* covers the defect RFC 102
   ruled on 2026-09-26, that is, per-write reads that grow with the store. Site C is its second instance, found by that
   ruling's own sweep. **Authorized by the owner 2026-09-26** (*"Authorized."*, answering that reading): site C is in
   this round and in 0.48.0.
3. **The refusal-class change (report §7.b): accepted.** Every failure exits 1 whatever its class. The case is a
   damaged repository, the append is still refused before anything is written or indexed, a FIFO is still never
   blocked on, and the new messages name their cause. Nothing to change. Name the two new message classes in the
   CHANGELOG entry's last sentence.
4. **Sites D, E and F: no change.** Each is bounded by something other than the store (D by a ref publication's few
   writes, E by the patch queue, F by tracked files). The report's table is the record.

**Measurement, units and budgets up front:** re-run **M1** (budget 5 min) and **M2** (budget 5 min) on the final
commit against the same 0.47.0 binary, alternating, as before. **Acceptance:**
- **M1 is now linear:** ×2 ±25 % per doubling across 1,000 / 2,000 / 4,000 × 20 KB; at 4,000 files, bytes read at most
  3 × (worktree + final containers); the 8,000 × 40 B point far below today's 4,257 MB.
- **M2 unchanged:** within 10 %.

**Docs:** the CHANGELOG entry drops the remaining index re-read sentence if item 2 lands, and gives the new M1 figures
and the `verify` robustness in one line.

Gates on the exact final commit. Report: `.git-exclude/review-request/append-length-without-reading-report-v2.md`. Then
the architect pushes the whole round, and it closes on a green Windows mutation suite.
