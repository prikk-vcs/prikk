# RFC 149 path B — one crate; the layer is a rule the gate enforces

**Ruled by the owner 2026-09-13** (RFC 149 §6e, §7.4). The crate cut is not made. Three increments,
each its own commit and report, in this order. The move handoff (v1–v4) is retired; nothing in it is to
be executed.

## B1 — the `LAYER` rule in `boundary-check`

1. A new category, `layer`, beside `subtree-cycle` and `hub`: **no production edge from the lower set
   into the upper set.** The upper set is listed explicitly as a constant of top-level module names —
   the twenty movable families of the v4 order (`bundle`, `compact`, `doctor`, `history`,
   `patch_checkout`, `seal_from_accepted`, `show`, `sync_negotiation`, `unlock`, `worktree_status`,
   `patch_exchange`, `tag_travel`, `recognition_claim`, `verify`, `patch_set_digest`, `rollback`,
   `merge`, `patch_inverse`, `patch_algebra`, `received`); everything else top-level is the lower set,
   with the five stayers and `checkout` named in a comment as *why* they are lower (RFC 149 §6b, §6d).
2. **An unclassified top-level module fails the gate** — the graph's top-level set must equal
   `UPPER ∪ LOWER` exactly, with `LOWER` also a listed constant, so a new module is a decision (which
   side?) and not a drift. This is the growth-direction control the theme rename names.
3. Failure names the edge (`from -> to`) and the rule. `--graph` gains `layer: {upper, lower,
   violations}` so the census can be re-read from the emission.
4. Controls: perturb by moving one module between the constants → the gate names the crossing edges
   (there are real ones both ways — e.g. `worktree.rs:12 -> checkout` if `checkout` is put upper); add
   a fake name to a constant → unclassified, fails; remove a real one → likewise. The real tree passes
   with **zero** violations — paste the count. Full gate set; the addendum does not apply (tooling).
5. EXECUTION-ORDER §6 rule 9 needs no new command — the rule lives inside `boundary-check`.

## B2 — remove `prikk-operations`

`crates/prikk-operations/` deleted; every one of the six registrations of increment 2 reverted
(`Cargo.toml` members/default-members/pin, `Cargo.lock`, `size.rs` `MEMBERS`, `boundary.rs` `PRODUCTS`,
the publication inventory's list and both procedure sequences, the two fixture lists in
`unsafe_boundary/tests.rs` and `publication/tests.rs`). `workspace_crate_order` reports eight again by
itself — assert it in the test that prints the set. `cargo package --list` per remaining crate
unchanged. Full gate set; CI green on the push (a member removal changes every job).

## B3 — revert the visibility that has no user

- The **operations-layer contract block** in `lib.rs` (46 names) is removed and every item returns to
  `pub(crate)`; the four widened `pub(crate) use` waypoints return. Root exports under default features
  return to **269** (the 0.41.0 count) — assert by diff against the tag.
- The **test-support additions of 3a and 3a.2** (49 names) are reverted: `mod test_gates` and
  `test_support` back to `cfg(test)`; the failpoints, the `_for_test` helpers, the 18 production items,
  the v1 chain, the four closure types — all back to their pre-3a visibility and configuration. The
  feature keeps exactly what it had at 0.41.0: the three names `prikk-cli` uses and
  `simulate_one_seal_for_test_support`. Feature-gated exports return to their 0.41.0 count — assert.
- **Keep**: the alias retirement (eleven call sites name the real function), every doc comment written
  for `missing_docs`, `#[non_exhaustive]` where it was added (RFC 147 ruling 2's posture costs nothing
  on a `pub(crate)` type), the size-check pins re-measured (`test_gates` leaves the graph again: 134 →
  130 nodes, 502 → 483 edges — paste before/after).
- **Remove `test_gates` from `LOWER_LAYER`** in the same commit — once `test_support` is `cfg(test)` again the module leaves the graph and the gate reports the entry stale. Expect `lower 29`.
- Full gate set; the addendum applies (`failpoints.rs`, `anchored/*` change `cfg` back).

## Not in scope

No module moves. No content change beyond the reverts named. CHANGELOG: nothing — no shipped behaviour
changes; the library section says nothing because default exports end where 0.41.0 left them.
