# RFC 147 §3b — `log` and `checkout` resolve a tag ref through the resolver that already exists

**Ruled:** `rfcs/proposed/147-refusal-visibility.md` §3b.

**HOLD: RFC 147 is in `proposed/`.** RFC-000 makes folder location lifecycle authority; do not start until
the owner moves it to `accepted/`. **If it does not move, this handoff is void.**

## 1. The change

Two call sites join the three that already use `refs::resolve_ref_tip_block`:

- **`checkout.rs:138-139`** — `let ref_state = load_ref_state(…)?; let block_id = ref_state.target_object_id;`
  Replace the direct read with the resolver, so a `RefKind::Tag` ref-state yields its tag object's
  `target_block_id`. Both `checkout` modes and `--content-path` flow through here; `patch_checkout.rs`
  has no resolution of its own — **confirm that rather than trust it.**
- **`history.rs:277-279`** — the `ObjectType::Block` assertion that renders `integrity error: history
  object … is Tag, expected Block`. Same replacement. **That `Integrity` message disappears with the fix;
  do not reclassify it separately.**

**Read the resolver's doc before using it** — it says plainly it *resolves and never validates*
(`ensure_ref_target_valid` is separate and stays separate). Checkout's own `validate_block_references`
after resolution is unchanged.

## 2. What must not change

- **`bundle export --ref tags/x`** already works through this resolver. It is the positive control:
  its output before and after must be byte-identical for the same repository.
- **`worktree-status --ref tags/x` must still refuse** with `invalid name: ref namespace is reserved`.
  That is `validate_local_branch_ref` being right on purpose: a worktree baseline is a branch. **If your
  change makes it succeed, you changed the wrong function.**
- **`log --ref heads/x` / `checkout --ref heads/x`** — every branch path, byte-identical before and after.
- **No `prikk diff`, no bare `--ref <block-id>`.** The latter is RFC 144 §4t's question, not this one.

## 3. Controls

1. **A tag at a non-tip block.** Seal twice, tag the *first* block, then: `log --ref tags/x` shows
   history from that block back (not the tip); `checkout --plan-only --ref tags/x` names that block as
   `target block:`; `checkout --patch-plan --format json --content-path <path> --ref tags/x` returns the
   content **at that block**, not at the tip. **A tag at the tip cannot distinguish the fix from the old
   behaviour on content; do not use one.**
2. **Perturb:** revert one call site to the direct read, show that site's tests fail and the other's
   still pass — two sites, two independent failures.
3. **`materialization:` vocabulary** — for a tag whose target block is unpublished-by-design or absent,
   the plan reports through the existing `CheckoutMaterialization` variants, never the mismatch error.
4. **Exit codes**: `0` on success, unchanged elsewhere. `ObjectTypeMismatch` must no longer be reachable
   from a valid tag ref — assert its absence, not only the success.
5. **Full gate set**, EXECUTION-ORDER.md §6 rule 9, verbatim; state the cross-target addendum's outcome
   from this round's own diff.
6. **Docs sweep**: any page that says checkout or log takes a branch only, or that quotes the mismatch on
   a tag, moves in the same round. `first-run.md` and `tutorial.md` were quoted-and-stale within hours
   last round; grep before you claim there is nothing.

## 4. Report

- Both call sites, before and after, quoted.
- Control 1's three outputs on the non-tip tag.
- The `bundle export` byte-identity check.
- The `worktree-status` refusal, still refusing.
