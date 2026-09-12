# RFC 102 / DC-33 — object-container writes are unprotected against each other; measure, then close

**Scheduled:** 0.40.0 plan item 3, **authorized by the project owner 2026-09-12**. Moved up from the
remaining-work inventory on correctness grounds. **Live.**

**The gap, in the concurrency reference's own words** (`docs/src/reference/concurrency-locking.md:192-205`):
*"two different concurrent unprotected appends into the same container computing offsets against the
same stale length … Known and accepted, not fixed here … The mechanism that would close it (a dedicated
object-container lock, or extending `import_bundle` to hold `ActiveLock` across its object writes) is out
of scope for this page to design; it is deferred, tracked follow-up scope."*

Content-addressed idempotency covers two writers writing the *same* object. It does not cover two
writers appending *different* objects to one container from the same stale length. `import_bundle`
already holds `ActiveLock` for its validate-then-record section (`bundle.rs:776`) and the received-index
container lock (`:798`) — **its object writes are outside both.**

## 1. Measure first — do not choose the mechanism before the map exists

**Every path that appends to an object container, and what lock it holds while doing so.** At minimum:
`commit` (worktree patch authoring), `seal`, `import_bundle`, `merge`, `rollback-draft`, `tag create`,
`branch create`, `sync`'s accept path. For each: the lock(s) held during the object append, in order,
quoted from source with line numbers. **A table, not a paragraph.**

Then the pairwise question the page raises: **which two of those can run concurrently against the same
container with neither holding a lock the other respects?** If the answer is "only `import_bundle` vs
anything", the fix is the page's second option. If `commit` vs `import_bundle` — or any two mutators —
can both append unprotected, a dedicated object-container lock is the only honest fix and holding
`ActiveLock` longer in one caller would paper over it.

## 2. Then close it

Propose the mechanism the table supports, **in the report, before implementing it** — this is one
round with a checkpoint, because the wrong lock scope in a durability path is worse than the gap. The
architect will rule within the round. Whichever mechanism:

- **Lock ordering is documented and asserted.** `concurrency-locking.md` already states an order;
  a new lock joins it explicitly, and a test drives the two-lock acquisition in both callers to show no
  inversion.
- **The page's "known and accepted, not fixed" paragraph is rewritten** to say what now protects the
  append, in the same round.

## 3. Controls

1. **A racing-writer test**: two processes appending different objects to the same container
   concurrently, with the fix → both objects present and readable, container verifies; **with the fix
   reverted → the test fails** (corruption, a lost object, or a `verify` failure — assert which).
   This is a control that must be able to fail; show it failing before showing it passing.
2. **No new lock inversion**: every existing lock-order test still passes; the new order is asserted.
3. **`bundle import`'s refused-import invariant** (`bundle.rs:41`: a refused import leaves nothing
   behind) still holds — it was the reason `ActiveLock` was introduced there.
4. **Full gate set**, EXECUTION-ORDER.md §6 rule 9, verbatim; cross-target: state the outcome from this
   round's own diff — locks are platform-sensitive and `fsutil` carries `cfg(target_os)`.

## 4. Not in this round

- No change to what any command does; only what it holds while doing it.
- No garbage collection, no compaction change.
