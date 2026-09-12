# RFC 102 — `prikk doctor --repair-index`: the rebuild that exists, reachable

**Scheduled:** 0.40.0, item 2, immediately after the object-store lock. **Live.**

**Why.** Concurrent object appends have been able to leave `index.container` pointing at the wrong
record since format 3 (see the locking handoff v2 — four of four twelve-process runs). Repositories
damaged before the lock lands are unverifiable today: `prikk tag` errors, `prikk verify` fails, and
**nothing a user can type reaches the repair.** `rebuild_index_from_containers` (`foundation/index.rs:559`)
already rebuilds the index from the self-describing containers, is proven by its own tests including the
crash-ordering criterion, and its doc says *"No `doctor` repair caller yet … not required by this round's
acceptance criteria."* That round is over; this one wires it.

## 1. The change

`prikk doctor [path] --repair-index`, beside `--repair-wal-tail`, following its conventions exactly:
explicit flag, reports what it did, refuses nothing it can do. It calls `rebuild_index_from_containers`
and writes the rebuilt index **atomically** — whatever the existing function returns, the on-disk
replacement must be crash-safe (write-new, fsync, rename, per RFC 102's durability contract); if the
function does not already do that, the handoff's scope includes making it so, and the report says which.

**Report what changed**: entries before, entries after, entries whose offset moved, and objects that
became readable. On a clean repository: `nothing to repair` with the counts, exit 0 — idempotent.

## 2. Controls

1. **The damaged fixture**: reproduce the collision with the pre-fix in-process barrier test's shape (two
   racing appends, shared stale length), confirm `verify` fails and `tag` errors, run `--repair-index`,
   then **`verify` clean and every object readable by id**. Assert the container bytes are byte-identical
   before and after — repair touches the index only.
2. **Crash during repair**: kill between write-new and rename → the old index is intact; after
   rename → the new one is. Reuse the crash-ordering harness the function's own tests use.
3. **Clean repository**: idempotent, reports nothing to repair, index byte-identical.
4. **`doctor` without the flag still only diagnoses** — no repair runs implicitly. Assert.
5. **Full gate set**, EXECUTION-ORDER.md §6 rule 9, verbatim; cross-target: state the outcome from this
   round's own diff (the index path is platform-neutral, but say so from the diff).
6. **Docs**: `integrity-recovery.md` gains the verb with when to use it; `troubleshooting.md` gains the
   `index entry … resolves to an envelope with computed id …` message pointing at it; CHANGELOG `### Added`.

## 3. Not in this round

- The lock itself (its own handoff).
- Any other `doctor` repair; `--repair-main-ref` stays refused as designed.
