# RFC 144 increment 2 — three follow-ups before increment 3

**Status:** increment 2 is ACCEPTED at `e72e6fc`. Nothing here reopens it. These are three items review found
*around* the change, ruled in RFC 144 §4k.1–§4k.3. **Items 1 and 2 must land before increment 3** (declared-move
capture and rename authoring); item 3 should land with them.

Small round. Do not enlarge it.

## 1. REQUIRED — prove the queued-envelope fold site

Review deleted the rename-routing block from `apply_queued_patch_envelopes` outright, left `apply_patch_ids`
untouched, and ran the full workspace suite: **zero failures across 1035 tests.** The routing is correct; nothing
holds it in place.

Add a control that drives a **two-node swap through `apply_queued_patch_envelopes`** — the unsealed-WAL path,
reached in production from `patch_replay.rs:566` — and asserts the swap resolves. It must fail if the routing
block is removed. `apply_state_effect`'s `RenamePath` arm refuses loudly, so a correctly-placed control fails
with that refusal rather than a wrong answer; say so in the test's own comment, because a reader otherwise
cannot tell the control from a test of the refusal.

**Do not** satisfy this by calling `collect_rename_run` directly. The gap is that the *fold site* is unproven,
so the control has to enter where production enters.

Report the deletion experiment back: with your control in place, remove the routing block, confirm your control
fails, restore it. A control that cannot fail is not a control — this is the third round in a row where that has
been the load-bearing question.

## 2. REQUIRED — correct increment 1's doc comment

`crates/prikk-store/src/patch_replay/apply.rs:271-279` says, present tense, that `apply_rename_batch` diverges
from `lifecycle_cache`, which "still checks occupancy against the current, already-partially-mutated path index
on every call, one operation at a time." **Increment 2 abolished that.** `rename_node_checked`, which the
paragraph names, now has no production caller — only `prikk-replay`'s own unit tests.

Rewrite the paragraph to state what is now true: both paths resolve a rename run node-before-path, from the same
algorithm, at their respective states — `apply_rename_batch` over `files`/`live_nodes`, and
`NodeLifecycleState::rename_nodes_checked_batch` over `path_to_id`/`live_by_id`. Keep the *finding* that the
sequential fold cannot survive a swap: it is why both functions exist. Change only its tense and its subject —
it describes history, not current `lifecycle_cache` behaviour.

Check the surrounding doc comments in the same sweep for the same staleness; `patch_replay.rs:8` and
`decode.rs:133` also narrate the split and may or may not still be accurate. Read them, do not assume.

## 3. REQUIRED — make `rename_nodes_checked_batch` fail-atomic

`crates/prikk-replay/src/node_lifecycle/mutation.rs`. Phase 1 clears every source from `path_to_id` before Phase
2 can still fail on occupancy, so an `Err` leaves the receiver half-mutated. No live defect — the one production
caller discards state on error — but this is `pub` on a published type, and `_checked` promises otherwise.

Resolve without mutating: build the batch's own source-path set in Phase 0, then check each destination as
"occupied by a node **outside this batch**" rather than vacating first to make the check true. Phases 0 and the
new check must complete before any mutation; commit only after every failure mode is behind you.

The observable behaviour on success must not change, and the collision message must stay verbatim — it is
`rename_node`'s own string and `rename_convergence` pins the genuine-collision shape on it.

**Leave `patch_replay::apply::apply_rename_batch` alone.** It has the same non-atomicity, but it is `pub(super)`,
its state is replay-local, and changing it would move code review has just cleared. If you find that argument
wrong, say so in the report rather than acting on it.

## 4. Gates and report

Full ten-gate set per `rfcs/EXECUTION-ORDER.md` §6 rule 9, run against your final commit, not an earlier one.

The three block roots in `block_state/tests/root_stability.rs` must stay byte-identical. Item 3 touches the
rename primitive; if any root moves, stop and report rather than re-recording the values.

Report to `.git-exclude/review-request/`. Include the item 1 deletion experiment, and say explicitly whether
items 2's sweep found staleness beyond the paragraph named here.
