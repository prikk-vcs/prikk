# RFC 147 — residue: the duplicate resolver, the dangling symlink, and `#[non_exhaustive]`

**Ruled:** `rfcs/accepted/147-refusal-visibility.md` §2e (rulings 2, G3) and §3c. Follows `3fd0ed68`,
`d00d7768`, `7c9c06f3`, all accepted. **Live.**

Three small things, each found while reviewing the two rounds. None is a defect in what you shipped;
two are what you shipped not reaching as far as one sentence claimed, and one is a ruling you asked for.

## 1. The second `current_target_block`

Your Case B report said `patch_inverse` *shares* `current_target_block`. **It has its own**:
`patch_inverse/read.rs:16`, `pub(super)`, next to `patch_replay/read.rs:21`. Measured:

```
$ prikk inverse-plan --ref tags/v1      → error: object type mismatch: expected block, got tag
$ prikk rollback-preview --ref tags/v1  → same
```

Route it through `refs::resolve_ref_tip_block` as you did the other four. **Then grep `fn
current_target_block` and any other `target_object_id` read a `--ref` can reach** — there were 17 files
reading `.target_object_id` without the resolver; most are verify/sync/publication internals that handle
the target as whatever it is, but **say which you checked**, not that you checked. `merge-evidence` /
`merge-plan` refuse tags by `validate_local_branch_ref` — **deliberate, leave it.**

## 2. G3 — a dangling symlink never reaches the classifier

```
$ rm a.txt && ln -s nope.txt a.txt && prikk worktree-status
  missing a.txt — tracked file is absent from the worktree        refused paths: 0
$ prikk commit …
error: precondition not met: a.txt: worktree symlink authoring is out of scope
```

The tracked-path presence check follows the link, so a dangling symlink is `Missing` and
`authoring_refusal` is never asked. **Find the exact call** (the branch producing `Missing`, just above
`symlink_metadata` in `worktree_status.rs`) and make presence non-following: an entry exists if it has a
directory entry, whatever it points at. Then the existing shape logic classifies it and the classifier
runs. `commit`'s own walk already sees it — that is why the two disagreed.

## 3. `#[non_exhaustive]` on `WorktreeChange` — and the 0.38.0 four, judged

Add `#[non_exhaustive]` to `WorktreeChange`. It is breaking once for a struct-literal constructor;
every later field is then free. **Then judge `QueuedOperationEntry`, `MergeEvidenceDisplayOperation`,
`WorktreePatchCommitReport`, `WorktreeStatusReport`** — the four that gained fields at 0.38.0 — by the
same rule: is this a report type consumers *read* (add it) or one they legitimately *construct* (say
so). **Report each; move only the ones the rule says.** Changelog states each as breaking-once.

## 4. Controls

1. **Non-tip tag** through `inverse-plan` and `rollback-preview`: names the tagged block, exit 0; revert
   the resolver call, both fail, restore.
2. **Dangling symlink** at a tracked path: `refused paths: 1`, entry carries `[refused: …]`, and
   `commit`'s stderr equals `error: ` + the JSON `refusal` — **the same agreement test, one more
   fixture**. And the non-dangling case still passes. Perturb the presence check back to following: the
   dangling test fails alone.
3. **`#[non_exhaustive]`**: a compile-fail test (or `trybuild`-free equivalent: a doc-test that must not
   compile is not available — use a unit test constructing via the crate's own constructor path and
   assert the public API still builds) — **at minimum, show a downstream struct literal no longer
   compiles and state it in the report.**
4. **Full gate set**, EXECUTION-ORDER.md §6 rule 9, verbatim; **cross-target addendum: state the
   outcome** — `worktree_status.rs` reads `symlink_metadata` and your last round showed the addendum
   applies to this area.
5. **Docs sweep** for any page saying `inverse-plan`/`rollback-preview` take a branch only, and for
   `WorktreeChange` in the library-API changelog notes.

## 5. Not in this round

- **G1** — a contradicted declaration is not a per-path property; separate ruling if wanted.
- **G2** — baseline symlink nodes; pre-existing, out of scope until symlink authoring is.
- **No change to `merge-evidence`/`merge-plan`/`worktree-status`'s branch-only validator.**
