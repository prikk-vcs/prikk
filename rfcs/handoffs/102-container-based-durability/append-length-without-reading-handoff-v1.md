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
