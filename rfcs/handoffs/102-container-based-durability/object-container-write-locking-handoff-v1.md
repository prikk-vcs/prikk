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

---

# v2 — RULED 2026-09-12, at the checkpoint: mechanism A, at the funnel, first in 0.40

**Your map is accepted whole.** Eleven rows, one funnel (`ObjectWriteSession::write_object` →
`append_object_to_container`, zero other callers), `O_APPEND` at `regular.rs:81`, four
`LockableContainer` variants with no object store among them, zero lock references in
`merge_execute.rs` / `tag_travel.rs` / `sync_negotiation/sender.rs`, one shared `index.container`.
Every claim opened at source. **The pairwise answer decides it: `commit` under `ActiveLock` and `tag
create` under nothing can both append. Extending `ActiveLock` across `import_bundle` would close one
row of eleven.**

## The measurement I ran that changes the priority

Your §5 said separate processes *"will not do it reliably"* — one run, twenty entries, zero collisions.
**I ran twelve concurrent `prikk tag create` in one repository four times on four fresh fixtures. All
four left the repository failing `verify`:**

```
error: integrity error: index entry for 51ad40b6… resolves to an envelope with computed id 95b7a194…
prikk tag        → rc 1, the same error   (the listing itself is broken)
prikk verify     → rc 1, four runs of four
succeeded per run: 3, 3, 4, 3   (the rest: lock conflict on the pointer-index lock — after their append)
```

Exactly the damage mode your §3 predicted — a wrong index offset, container bytes intact under
`O_APPEND` — but repository-wide, on the first attempt, every attempt. **And no shipped command can
repair it**: `rebuild_index_from_containers` (`index.rs:559`) has zero non-test callers and its own doc
says *"No `doctor` repair caller yet."* Two ordinary commands run concurrently leave a repository
unverifiable with no user-reachable recovery, on a path unchanged since format 3. **This is 0.40's first
item.** RFC 148 and the rest follow it.

## Ruling: (A), and where and how

1. **A fifth `LockableContainer` for the object store, acquired and released inside
   `append_object_to_container`, spanning the length read through the index append.** At the funnel, so
   all eleven rows close in one place — including rows 5, 8, 9, 10 and 11, which no caller-side change
   would reach.
2. **It is a leaf.** Nothing is acquired while it is held. The documented order gains one line — *the
   object-store lock is never held while acquiring any other* — and row 11 (RefState written under
   `RefLock` + `{RefPointerIndex, RefLog}`) nests it with no inversion possible. **Not session-scoped**:
   a lock held across an `ObjectWriteSession` would sit across `publish_ref`'s own acquisitions and need
   a real ordering rule; that is the "wrong scope in a durability path" the handoff warned about.
3. **Fail-fast, like every other lock.** The consequence you named is accepted and is the point: an
   overlapping `commit` and `tag create` now produce one visible `lock conflict` instead of a silently
   wrong index. Today they "usually both succeed" — I measured what usually means. **CHANGELOG:
   `### Changed`, stated plainly**, alongside the `### Fixed` for the defect.
4. **Cost accepted**: one lock-file create + unlink per object. A commit with N blobs pays N+1 pairs of
   syscalls against N durable writes with fsync; it is noise. If a measurement ever says otherwise, widen
   deliberately with the ordering rule written — not now.
5. **(B) and (C) refused.** (B) introduces a second lock semantics into a codebase that has exactly one.
   (C) puts a verify-and-repair loop in the hot write path and leaves the index briefly wrong by design;
   RFC 102's posture is fail-closed with explicit locks, and this is where it matters most.

## Control 1, corrected for the fix

Your in-process two-thread test at the `AppendWrite` failpoint is right for the **pre-fix** collision:
both read `L`, both block, both proceed, both record `offset = L` — deterministic. **Post-fix that
barrier cannot sit where it sits**: the second thread can no longer enter the exclusive region, and a
barrier inside it deadlocks. **Move the rendezvous before the lock acquisition**, and make the post-fix
assertion: *no two index entries share `(type, offset)`, and exactly one of the two racing appends
returns a lock-conflict error.* That is the control that can fail in both states for the right reason.

**Add the twelve-process run as an `#[ignore]`d instrument**, in the project's established shape for
measurement tests — it hit 4/4 here, and a future reader should be able to run it rather than take
either of our words.

## Also required in this round

- `concurrency-locking.md:192-205`: the "known and accepted, not fixed" paragraph becomes what now
  protects the append, and drops the wording that invites reading the risk as byte corruption.
- The **repair verb is a separate handoff** (`doctor-repair-index-handoff-v1.md`) — sequence it
  immediately after, or in parallel by another hand; repositories damaged before this lock exists need
  it regardless of this round.
