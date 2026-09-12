# RFC 149 — the move: `prikk-operations` above `prikk-store`

**Live 2026-09-13.** Owner rulings: the 41 items become `pub`; the notice to stikk is required but is not
a gate (no production use); `prikk-operations` is the name; the size gate is already in place. Census:
`rfc149-step-2b-report-v1.md` as reclassified in RFC 149 §5.2b (41 / 10 / 26). **Four increments, each its
own commit(s) and report; do not combine them.** Moves are separate from any content change (RFC 131 §5).

## Increment 1 — the contract, in place, before anything moves

1. **The 10 test-support items** go behind the existing `test-support` feature: `pub` under
   `#[cfg(feature = "test-support")]` re-export, as `simulate_one_seal_for_test_support` is. Rename
   `refs::write_ref_pointer_candidate` at its call sites to the function's real name; an alias is not a
   contract.
2. **The 41 become `pub`**, and `lib.rs` gains one section — `// Operations-layer contract (RFC 149 §5.2b)`
   — re-exporting exactly them, so the set is named in one place. `#[non_exhaustive]` on every enum and
   struct among them (RFC 147 ruling 2); the three enum variants counted in the census are covered by
   their enums. No other item changes visibility; the boundary gate and `size-check` stay green.
3. Root-export name diff `0.41.0` → this commit: exactly the 41 added (list them), nothing removed.
   Full gate set. Report before increment 2 starts — the architect reviews the contract before the crate
   exists.

## Increment 2 — the crate skeleton, empty

`crates/prikk-operations/`: `Cargo.toml` (workspace version and pins, depends on `prikk-store`,
`prikk-object`, `prikk-error`, whatever the surfaces need — add as the moves demand, never more),
`README.md`, `LICENSE` (as the other crates carry them), `src/lib.rs` with the crate doc stating the
layer: *operations above the store; nothing here is reachable from `prikk-store`*. Register it everywhere
a workspace member must be: root `[workspace.members]`, release-policy `MEMBERS` (size-check control 8
will fail until you do — that is the control working), `boundary.rs`'s member allowlist (RFC 139's lesson),
`policy/evidence.rs`'s `CRATE_ORDER` (RFC 141's), the release workflow's publish order (`prikk-store` →
`prikk-operations` → `prikk`), `Cargo.lock`. `cargo package --list -p prikk-operations` in the report.
Full gate set; CI green on the pushed commit before increment 3 (a new member changes every job).

## Increment 3 — one move commit per surface family

Order: the five surfaces that reach no core item first (`memory_store`, `received`,
`rfc111_seal_simulation`, `state_root`, `unlock`), then by increasing census weight, `verify` last. A
family = one top-level module and its subtree (`rollback/`, `merge/` are one each). Per commit:
- `git mv` the module directory; `crate::<core>::X` becomes `prikk_store::X` (the contract names);
  `crate::<other surface>::Y` becomes a `prikk-operations` path; tests move with their module (§5.3).
- `prikk-operations/src/lib.rs` re-exports **the same root names** the module had in `prikk-store`;
  `prikk-store/src/lib.rs` drops them. `prikk` (the CLI) switches its imports for those names to
  `prikk_operations::`. Nothing else in the CLI changes.
- Gate after every commit, including `boundary-check` (**the eight declared cycles unchanged in number
  and membership — §5.4; paste the count each time**) and `size-check` (the moved file is now under a
  different crate name in its allowlist — update the entry, not the threshold).
- The report for this increment carries a table: family, files, root names moved, gate state.

## Increment 4 — the release entry and the notice

- CHANGELOG `### Changed — breaking once: the operations layer is its own crate`, listing **every root
  export that moved** (174 of 269 today; derive the list from the diff, do not copy this number) and the
  one-line migration: add `prikk-operations`, rename `prikk_store::` → `prikk_operations::` for those
  names. The library section names the new crate's first version (the release's).
- Docs: every page that shows a `prikk_store::` path for a moved name.
- The stikk letter is the architect's; it goes when increment 3 is complete, with the final list.

## Hard stops (RFC 149 §5)

A core→surface edge appearing under the compiler; a test that cannot move with its module; a declared
cycle changing; any item beyond the 41 needing `pub`. Stop and report; do not widen.

---

# v2 — 2026-09-13: increment 3 becomes 3a and 3b (RFC 149 §6b)

Your stop was right; both blockers are ruled in `rfc149-increment-3-blockers-review-v1.md`.

## Increment 3a — the test-support surface (one commit, its own report, before any move)

1. **Five families stay in `prikk-store`**: `memory_store`, `worktree`, `state_root`, `block_state`,
   `rfc111_seal_simulation`. Nothing to do for them except leave them out of every later step; their
   root exports do not move. Movable families: 21.
2. **The surface.** `#[cfg(any(test, feature = "test-support"))] pub mod test_gates;` is **not** the
   shape — the gates themselves stay `cfg(test)`. Split: `test_gates::test_support` becomes reachable
   under the feature (its 28 functions `pub`), the gate files stay test-only. The failpoints' `fail_once`
   and `Point` move to `#[cfg(any(test, feature = "test-support"))]` with `pub` visibility on a path
   `lib.rs` can re-export; `foundation::index::remove_index_entry_for_test` and
   `foundation::container::encode_container_record_for_test` likewise. **Only what a movable test
   reaches** — derive the list by grepping the 21 families' test files for `crate::` paths that resolve
   to `cfg(test)` or `pub(crate)` items in the lower layer, and print it in the report.
3. One block in `lib.rs`, `// Test-support surface (RFC 149 §6b)`, re-exporting exactly the list under
   the feature, beside the operations-layer contract and with the same "adding is a decision" comment.
4. **Measure**: `boundary-check --graph` before/after (nodes, edges, cycles, hubs — `test_support` now
   appears; **no new cycle and no new hub**, or stop); `size-check` green; root exports under default
   features unchanged at 318; under `--features test-support` the new count, listed; `cargo package
   --list -p prikk-store` unchanged in file count. Full gate set; the addendum applies (`failpoints.rs`
   is `cfg(target_os)`-gated).

## Increment 3b — the moves, in the reference graph's order

Recompute the order of your §1 for 21 families (the five that stay become plain `prikk_store::` paths
for their users) and put it at the top of the first 3b report. `patch_exchange` + `tag_travel` are one
commit. Per commit, as v1 §3 says, plus: the DECLARED cycle count pasted, `size-check` allowlist entries
moved with their files, and `doctor`'s two RFC 102 controls moving with `doctor`. `bundle.rs`'s
exhaustive match on `DecodedOperationKind` gains its wildcard arm in `bundle`'s own move commit.

## Not changed

Increment 4 as written; the hard stops as written. The CHANGELOG's final moved-name list is derived from
the diff — it will be shorter than letter 008's appendix by the five families' exports, and the release
note says so.

---

# v3 — 2026-09-13: increment 3a.2 before 3b (RFC 149 §6c)

One commit, one report, then 3b.

1. **The 17 of your §6** — the 15 production `pub(crate)` items and the two `_for_test` helpers — become
   `pub` at their declarations (private modules keep them unreachable) and are re-exported **only under
   `test-support`**, in the test-support block. Not the contract block. Doc comments as `missing_docs`
   demands; one sentence each saying which moving test needs it.
2. **The v1 fixture**: `plan_authored_text_span_v1` and `AuthoredTextSpanV1` move from `#[cfg(test)]` to
   `#[cfg(any(test, feature = "test-support"))]`; `publish_text_create_then_edit_block_v1` joins the
   surface. `left_anchor`/`right_anchor` untouched.
3. Same measurements as 3a: graph before/after (no new cycle, no new hub), `size-check`, package list,
   default `pub use` names unchanged, feature-gated names listed. Full gate set; the addendum applies
   if `text_span` carries `cfg` gating — say so from the diff.

Then **3b** as v2 says, with the order for 21 families at the top of its first report.

---

# v4 — 2026-09-13: increment 3b.0 derives the contract; then 3b moves 20 (RFC 149 §6d)

Your recommendation is adopted. `checkout` stays (`worktree.rs:12` is production); `merge` moves
(`block_state.rs:325` is a doc link — a code span in `merge`'s commit). 20 families; your §1 order minus
`checkout`.

## 3b.0 — derive, in a scratch worktree, no commit to `main` until the list is reviewed

1. Detached worktree. Move **all 20 families** into `prikk-operations` at once (`git mv`, path rewrite,
   both `lib.rs`); `prikk-operations` takes `prikk-store = { features = ["test-support"] }` as a
   dev-dependency. Compile-fix (`cargo check --workspace --all-targets --all-features`, then `cargo
   test --workspace --no-run`) until green, making **only** visibility, `cfg`, re-export, field, and
   constructor changes in the store — never a move back, never a content rewrite. Every change is a
   line in the report.
2. **Report three lists**, each item with module, kind, and which family needs it:
   - **Contract additions** (production, core or infrastructure) → the operations-layer contract block;
   - **Test-support additions** → the test-support block;
   - **Fields and constructors**: per contract type, which fields become `pub`, whether
     `non_exhaustive` stays (report-shaped) or goes (plain carrier, reason written), and which types
     need a `pub` constructor because the operations layer constructs them.
   Plus the surface-to-surface churn list with the family whose move retires each entry (not a
   contract; not in either block), and the final root-export counts (default and feature).
3. Then the worktree is discarded. **Nothing lands from 3b.0 except the report**; the architect reviews
   it and the owner may read it (RFC 149 §6d.5).

## 3b.1 — the contract, as one or two commits on `main`

The store side only: the contract additions, the test-support additions, the fields and constructors —
from the reviewed list, no more. Gates; graph unchanged (no module moves); default and feature export
counts as the list predicts; `cargo check` of a *second* scratch worktree with all 20 moved must now be
green with **zero** store changes — that is the control that the list was complete.

## 3b.2 — the moves, in order

As v1 §3 / v2, 20 commits (19 plus the pair), each a `git mv`, a path rewrite, two `lib.rs` edits, the
churn edits its position in the order requires, and a gate run with the cycle count pasted.
`bundle.rs`'s wildcard arm and `block_state.rs:325`'s code span in the commits that need them.
