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

---

# v3 — 2026-09-12, one round: the repair holds the lock it exists to complement

**`2ec9aa40` reviewed** (`.git-exclude/reviewed/rfc102-doctor-repair-index-review-v1.md`): the verb
repaired the four repositories the pre-lock binary really damaged, and a fully garbage index besides.
Not accepted for one reason.

## 1. Required: `repair_object_index` runs under `LockableContainer::ObjectStore`

The repair reads the index, scans the containers, and renames a rebuilt index over the old one — with
no lock. Measured: forty rounds of `doctor --repair-index` launched beside `tag create` left `verify`
failing (`ref tags/race40 names missing RefState …`); two writers' entries were discarded by installs of
a stale scan. A repair is a writer to `index.container` and takes the same lock every other writer now
takes.

- Acquire in `doctor::repair_object_index` — the surface layer, exactly where `append_object_under_lock`
  sits and for the same reason (`foundation` must not reach `crate::lock`). Hold it from before the
  first read until `repair_index_from_containers` has returned. Acquire nothing else: it stays a leaf.
- Consequences to state in the docstring and `integrity-recovery.md`: a writer arriving during a
  repair is refused with `lock conflict`; a repair arriving during a write is refused the same way; a
  stale `objects.lock` refuses the repair with the message that names `prikk unlock`.
- Controls: (a) the forty-round race as an ordinary test or the instrument's shape — assert `verify`
  clean and every concurrent `tag create` either succeeded or reported `container:object-store`;
  (b) deterministic — hold `ObjectStore` from the test, call `repair_object_index`, assert
  `LockConflict` and the index byte-identical. Perturb (b) by removing the acquisition.
- The bypass guard's rule extends: `repair_index_from_containers` may be named in production only at
  its definition, its import, and inside `repair_object_index` under the lock. Add it to
  `every_object_append_goes_through_the_locked_wrapper` or a sibling of the same shape.

## 2. Also in this round

- `#[non_exhaustive]` on `IndexRepairReport` (RFC 147 ruling 2's reasoning; it is a report struct that
  will gain fields).
- Two stale doc lines from before the set comparison: `already_correct`'s field doc ("byte-identical")
  and the function doc ("rebuilt bytes are compared") — say "equal as a set of entries".
- CHANGELOG `### Added` gains one sentence: the repair takes the object-store lock, so it refuses
  rather than racing a concurrent writer.

## 3. Optional, while you are in `object_store/tests.rs`

A deterministic companion to control 1: hold `ObjectStore` from the test thread, call `write_object`,
assert `LockConflict`. The racing test passed 150 of 150 under 2× CPU oversubscription here, so this is
insurance against a fast filesystem, not a fix.

## 4. Gates

The full set, verbatim; the cross-target addendum applies to this round's diff by inheritance (`index.rs`
carries platform `cfg`); say so from the diff and run both targets.
