# RFC 168 implementation — one recovery log, a way back, and no rename on the durable path

**Live 2026-10-06, and it is next.** RFC 168 is ACCEPTED by the owner.

## Task title and purpose

Implement `rfcs/accepted/168-a-way-back-from-every-repair.md`: §3.1–§3.4 as revised in §8, with §7 as the required
evidence.
- **D6:** every repair's removed bytes can be listed and restored, under exact conditions.
- **D5:** repository state that cannot be rebuilt no longer depends on a Windows rename for durability.

## Background and governing RFC

- **Read all of RFC 168 first,** above all §3, §6 (the residuals the owner accepted) and §8.
- **Then read the design-round reviews,** `.git-exclude/reviewed/rfc168-design-round-review-v1.md` and `-v2.md`:
  rulings R1–R8, and the lost-rename method.
- **The prototype** (`/home/nabbisen/Desktop/prikk/scratch-168/proto`) is a reference, not a patch to copy:
  - it still writes `.bytes` files;
  - it hard-codes the pointer index's container path;
  - its `verify` view was never built.

## Change scope

### U1 — the log, the commands, the writers (§3.1, §3.2)

1. **`recovery/log`** in the container frame (magic `PRECLOG1`), with the entry fields of §3.1. It is appended and
   flushed **before** the truncate, and never truncated by anything except `--recovery-clear`.
2. **Every repair writes only the log.** No `.bytes` file is written any more.
   - **Writers:** `wal.rs:350` (both callers), `refs/pointer_index.rs:514`, `doctor/repair_tails.rs:336` (ten files),
     and `foundation/index.rs:958`.
   - **The object index's lost ids:** the entry lists them, and the restore never writes them. Its plan names the
     rebuild.
3. **The meaning-file table, from source, one row per writer:** WAL → `ref-name` and the witness; pointer index → the
   ref-log container it precedes (**resolve the live slot; never hard-code `log-a`**); and each `repair_tails` file.
   - **Stop and ask** if any file's meaning cannot be captured by file identity.
4. **Commands (§3.2):**
   - `prikk doctor --recovery-list`;
   - `prikk doctor --recovery-restore <id> [--plan-only]`;
   - `prikk doctor --recovery-clear [--plan-only]`.

   **Each command:**
   - **its text** is as §3.1–§3.2 words it, including the plan's "what follows" sentence and the listing's last line;
   - **its locks** are every lock the repair it undoes takes. `--recovery-clear` takes the union of the repairs' locks;
   - **its id** is the full 16 hex characters; anything else refuses.
5. **`verify`'s log line (§3.1):** a line of its own; **`verify`'s exit status is unchanged by it.**
6. **The allowlist (RFC 168 Status):** a restore refuses any source not on the fixed list of repairable files, whatever
   the entry says.

### U2 — in-place writes, the marker's target, early creation (§3.3)

1. **One in-place writer per §3.3 row, on every platform** (no `cfg(windows)` branch):
   - **the pointer:** D1's truncate-then-append, inside the marker;
   - **`FORMAT`:** a one-byte overwrite, then flush;
   - **the witness:** overwrite from offset 0, set the length, flush; **never truncate first.** Its clear (to empty) is a
     truncate to 0;
   - **`ref-name`'s restore:** D1's writer.

   **The four caches keep `atomic_replace`:** `verified_blocks.rs:66`, `lifecycle_cache/incremental.rs:288`,
   `commit_index.rs:80` and `foundation/index.rs:920`. **Afterwards, grep: every remaining `write_file_atomically`
   caller is one of those four, or a worktree writer.** List them in the report.
2. **The marker's target line:**
   - the switch appends `target heads/<name>\n` after the sentinel; a checkout appends its ref;
   - with a malformed pointer and the marker set, `status` and every refusal name the exact command;
   - a target line without its newline is ignored.
3. **Early creation:**
   - **`init`** creates `recovery/log` and the default session's empty witness;
   - **in an existing repository,** the first command that writes creates any that are missing. **This happens at one
     call site, named in the report.**

### U3 — the worktree route (§3.4)

**Find the command that writes the current branch's files over files whose bytes differ,** with the marker clear. Run it
in exactly the state P1/W1 built.
- **If no such command exists, stop and ask.** Do not add one, and do not quote a route that refuses.

### U4 — docs

- **`repository-layout.md`, `durability-recovery.md` and `troubleshooting.md`:** the log, the three commands, and the
  older files listed only. Remove *"nothing reads it back"* and *"by hand"* for new saves.
- **`platform-support.md`:**
  - the `atomic_replace` row (now only caches and worktree files);
  - the `durable_directory_entry` row, whose crash-before argument is wrong (RFC 168 §1 item 3);
  - residuals (a), (b) and (c), each by name.
- **The CHANGELOG entry.**

### U5 — timing

- **The build:** release, from the final commit, sha256 stated; on `/home`, with `stat -f -c %T` printed; inside an R1
  scope.
- **The comparison:** the repair's cost against `main` (built from `9c0c2b53` in its own target dir), at two file sizes.

## Explicit non-change scope

- **No format version change.**
- **No change to:**
  - the four caches;
  - worktree writes (they keep `atomic_replace`);
  - the classification of RFC 166 / RFC 167;
  - `release-signers.toml`.
- **Old `.bytes` files are never deleted or rewritten** by any command.

## Required tests

- **Rehearsals:** for each writer (the WAL, the pointer index, and each of the ten `repair_tails` files): repair, list,
  restore, then a byte-identical file, then `verify`'s report.
- **Controls, each shown red with its check removed:**
  - the source written since;
  - a same-length change to the prefix;
  - a meaning file changed (R2's WAL-at-offset-0 example);
  - a damaged region in the log, with the entry after it still listed;
  - a source outside the allowlist;
  - a wrong-length id.
- **A torn-write failpoint at each §3.3 site,** and what every reader then does.
- **Lost-rename runs** (complete the operation and the steps after it, then put the old bytes back, or remove the new
  name):
  - the witness's first creation in a repository from 0.48.0;
  - `ref-name`'s restore.
- **The marker's target:** a torn pointer under a set marker; the refusal's exact command; that command run; then
  `status` clean.
- **Text tests** for the three commands and `verify`'s log line.
- **An old-repository test:** a 0.48.0 repository (`/home/nabbisen/.pgtmp/prikk-0.48.0-5e50a661`) with an old `.bytes`
  file. The first 0.49.0 write creates the log; the listing shows the old file as older; nothing is deleted.

## Prohibited shortcuts

- A `cfg(windows)` branch for the in-place writers.
- Claiming Windows durability from a Linux run. Say "from source".
- Text-matching printed paths (`support::assert_same_path`).
- Modelling a lost rename as a crash before it.
- A restore without the allowlist.
- Quoting a route in docs or a refusal that was not run.

## Compatibility and security constraints

- **A 0.48.0 repository needs no migration,** and older binaries ignore `recovery/log`. Every in-place write keeps its
  file's bytes exactly as today.
- **A restore and a clear are writers:** the repairs' locks, the exact file and offset, the allowlist, and a plan first.
- **The log may hold user content.** `--recovery-clear` is the way to remove it, and the docs say so.

## Known risks

- **A torn pointer on Linux, where today's rename never tore.** The marker's target line is what keeps the refusal
  clear. Test it.
- **The witness's in-place write is a new torn shape.** Row 8 must rebuild it, never treat it as cleared.
- **U3 may find no route,** which is a stop, not a workaround.
- **More tests on Windows CI.** Estimate the Windows job's time. If it would exceed 50 minutes, stop and ask.

## Required evidence and review request

- **Per unit:** the real start and end, with `date` printed in the session log at each boundary.
- **The tables:** the meaning files, the remaining `write_file_atomically` callers, and the early-creation call site.
- **Gates:** `scripts/gates.py`'s summary on the final commit, and the primary tree clean.
- **Report:** `.git-exclude/review-request/rfc168-implementation-report-v1.md`.

| unit | what | budget (stop at ×2) |
|---|---|---:|
| U1 | the log, the commands, the writers, the rehearsals | 240 min |
| U2 | in-place writes, the marker's target, early creation, failpoints | 180 min |
| U3 | the worktree route | 45 min |
| U4 | docs | 60 min |
| U5 | timing | 30 min |

## Addendum 1 — 2026-10-06: fix round (review `rfc168-implementation-review-v1`)

**Corrections Required.** The core holds. Fix the items below. **Item 9 (amendment A1's mechanism) waits** until the owner has read A1; the architect marks it live here.

1. **F1 — `verify` and the log:**
   - a log that cannot be read prints one line, *"recovery log: cannot be read (<reason>)"*;
   - **`verify`'s exit status is the same as without the log,** in prose and in JSON;
   - **tests:** the log is a directory; the log is a symlink.
2. **F2 — the plan tells the truth:**
   - **`--plan-only` exits as the real run would.** If a condition fails: *"plan only -- this restore would be refused"*, exit 1, and no "After this" sentence.
   - **Each row states the fact found:**
     - `ok  the source is 680 bytes, the length the repair left`, or `no  the source is 690 bytes; the repair left 680`;
     - `ok  active/default/witness is unchanged since the repair`, or `no  active/default/witness has changed since the repair`.
   - **After a written restore,** the sentence reads *"The file now holds…"*.
3. **F3 — a torn tail is only a prefix of one frame** (RFC 160 F3): a short header, or a sound magic whose body runs past the end of the file.
   - **A complete frame at the end that fails its checksum is damage,** listed and printed by `verify`.
   - **Tests:** both shapes; and the reproduced case (one byte flipped in the only entry) must now report damage.
4. **F4 — the budget bounds the work:**
   - charge each candidate its header plus its claimed body (clamped to the bytes remaining) **before** hashing, as `sound_frame_after_partial_budgeted` does;
   - hash in place, with no copy;
   - **test:** a damaged entry whose payload holds many in-range `PRECLOG1` headers keeps the bytes hashed within 8× the input. **Control:** remove the charge, and the test goes red.
5. **F5 — the locks of restore and clear** are the union of every repair's locks: the object-store lock, and the active lock of each session a repair can cut. List the set from source.
6. **F6 — `--plan-only` writes nothing:** no `ensure_write_state` on any plan-only path, including `--discard-damaged-commits --plan-only` and `--restore-queue-target --plan-only`.
   - **Test:** a repository with no log and no witness, then each plan-only command; both files still absent.
7. **F8 — meaning paths:** a restore recomputes them from the table. An entry whose list differs refuses, naming why.
8. **F9 and F12 — messages and the version field:**
   - the allowlist refusal names the source, and says a restore does not write that file;
   - ids are compared ignoring case;
   - `--recovery-clear` lists the entries it removes (id, source, size), in the plan and in the run;
   - **the header's version is read.** A newer version lists as *"written by a newer prikk; this version cannot read it"*. It is not damage.
   - **Creating the log and the witness tolerates `AlreadyExists`,** so a concurrent first write does not fail.
9. **A1's mechanism — LIVE 2026-10-06** (the owner approved A1, RFC 168 §9). Fix `troubleshooting.md:347-348` to match what lands:
   - replace entries (the witness rewrite in `--discard-damaged-commits` and `--repair-tails` row 8);
   - run ids;
   - a run-level restore in reverse order, every condition checked first;
   - **an interrupted restore finishes when run again:** a step whose file already holds its result counts as done;
   - **a run overlapped by a later run** refuses, naming *"restore run <later id> first"*;
   - **tests:**
     - a discard, then its restore, gives the WAL **and** the witness byte-identical to before the discard;
     - a restore interrupted after its first step (failpoint), then run again, completes;
     - an overlapped run names the later run;
     - a two-file `--repair-tails` run, restored by one id;
     - a run whose middle step fails its condition, and nothing is written.
10. **Docs and the code comment:**
    - **`platform-support.md`'s `durable_directory_entry` row:** the marker argument holds only for a crash *before* the marker clear. A power loss after a completed operation is residual (a);
    - **`troubleshooting.md`:**
      - "never gone" becomes "kept until `prikk doctor --recovery-clear`";
      - drop "by hand" for log entries;
      - add the residual-(a) route as ruled: move the files `status` names out of the worktree, then `prikk checkout --patch-materialize --ref <current branch>`. **Run it before quoting it;**
    - **the CHANGELOG:** do not count the WAL and the pointer index twice against "the ten";
    - **`anchored.rs`:** the displaced doc comment goes back on `truncate_existing_file_required`.
11. **Tests:**
    - **P1 becomes a regression test:** a failpoint at the marker clear leaves the new pointer in place;
    - **the snapshot-checkout route:** an interrupted `checkout --snapshot-materialize` names a command that, when run, finishes it.
12. **U5:** the timing, with the 1-minute load recorded per sample. If the load stays above 4, report the load and stop.
13. **`scripts/gates.py`'s full summary** on the final commit, and a Windows CI time estimate.

**Not in this round** (carried to the 0.49.0 release-prep triage):
- the stale `active.lock` left by a sync failure at its own creation;
- the stranded `ref-name`, which `verify` does not report as an item.

**Prohibited:**
- a `cfg(windows)` branch;
- weakening a condition to pass a test;
- quoting a route not run;
- starting item 9 before it is marked live.

| unit | what | budget (stop at ×2) |
|---|---|---:|
| A | items 1–8, 10–11 | 180 min |
| B | item 12 | 30 min |
| C | item 9 (when live) | 180 min |

**Report:** `.git-exclude/review-request/rfc168-implementation-report-v2.md`, with `date` at each unit's start and end.

**Fix round ACCEPTED 2026-10-06 with conditions** (review `rfc168-implementation-review-v2`). `bcebf76f` is pushed with that review's record.
- **Still owed before the 0.49.0 cut:** item 9 (A1, once the owner has read it), item 12 (the timing, on a quiet machine), and item 14 below.

14. **(live now) A frame's version is trusted only after its checksum holds.**
    - `checksum_of` hashes the header's own version, not the constant;
    - `classify_at` calls a frame `Newer` only when that checksum holds; a failing frame is damage.
    - **Test:** flip each version byte of a sound entry, and it reads as damage, with the entries after it still listed. **Control:** go back to the constant, and the test goes red.
    - **Budget:** 30 min. **Report:** `.git-exclude/review-request/rfc168-implementation-report-v3.md` (with item 9, if it is live by then).

**Item 14 ACCEPTED 2026-10-06** (review `rfc168-implementation-review-v3`, `51b17edc`). **Still owed before the 0.49.0 cut:** item 9 (A1, after the owner's reading) and item 12 (the timing, on a quiet machine).

**2026-10-06: item 9 is LIVE** (the owner approved A1). **Item 12's method changes:** the load rule ("above 4, stop") cannot be met on this machine, which other projects share. Measure under load, but so that load cancels:
- **The binaries:** release builds of `main` (`9c0c2b53`, its own target dir) and of the final commit;
- **the samples:** at each of two WAL sizes, 15 interleaved pairs (main, then new, alternating which goes first), each on a fresh fixture made outside the timed region, with the 1-minute load recorded per sample;
- **the report:** each side's median, the median of the per-pair ratios, and their min–max spread;
- **the reading:** if the median ratio is at most 1.10, or its excess lies inside the spread, the log costs nothing visible, and item 12 is done. Otherwise **stop and report:** a visible cost needs a quiet machine to size it.

**Report for items 9 and 12:** `.git-exclude/review-request/rfc168-implementation-report-v4.md`, with `date` at each unit's start and end.

| unit | what | budget (stop at ×2) |
|---|---|---:|
| C | item 9 (A1) | 180 min |
| D | item 12 (timing, interleaved) | 45 min |

**Items 9 and 12 ACCEPTED 2026-10-07** (review `rfc168-implementation-review-v4`, `d4fd0068`). **RFC 168 is delivered,** on condition that CI is green on every job of its push. If a job goes red, this handoff reopens as a fix round.

**RFC 168 CLOSED 2026-10-07:** CI `37487680419` on `b7108c91` (A1 included) is green on all 16 jobs (Windows mutation 34m11s, macOS 10m30s). The delivery's condition is met.
