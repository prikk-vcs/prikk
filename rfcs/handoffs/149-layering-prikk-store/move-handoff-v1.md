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
