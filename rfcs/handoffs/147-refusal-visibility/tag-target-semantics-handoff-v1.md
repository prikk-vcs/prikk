# RFC 147 §3d — `branch create --from` a tag, `tag create --target` a tag, and one `current_target_block`

**Ruled:** `rfcs/accepted/147-refusal-visibility.md` §3d. Follows `979297e9`/`d1b6be83`, accepted. **Live.**

Three items; all three came from your own census and your own offer. None is large.

## 1. `branch create heads/<n> --from tags/<t>` — resolve

Today: `object type mismatch: expected block, got tag`. **Ruled: dereference.** `--from <ref>` means
"at the block this ref names"; every read surface now does this; the operation reads the tag and writes
a new branch at that block. Use `resolve_ref_tip_block`, and keep whatever validation `--from` already
performs on a branch ref's block (`ensure_block_exists` or its equivalent) — **the resolver resolves and
never validates**, as its doc says.

## 2. `tag create <n> --target tags/<t>` — refuse, as a precondition

**Do not dereference.** The model is ref → tag object → block, one hop; a tag-of-a-tag is outside it,
and collapsing silently to the block would make "tag this tag" and "tag this block" identical in history.
Refuse with `PrikkError::Precondition`, naming what *is* accepted:

> `--target names a tag (tags/<t>); pass the block id it points at, or a branch ref`

Short is fine; **both accepted forms must be in the message.** Today's `ObjectTypeMismatch` is the wrong
class for a caller-fixable input and disappears with this.

## 3. One `current_target_block`

Two `pub(super) fn current_target_block` with near-identical bodies (`patch_replay/read.rs:21`,
`patch_inverse/read.rs:16`) and an imperative comment binding them. **Consolidate to one**, in whichever
module the other can honestly import from without inverting a dependency — check the coupling gate
(`boundary-check`) before choosing, and **report which direction you chose and why**. If both directions
create an edge the gate rejects, say so and stop; that is an RFC 131 question, not this round's.

## 4. Controls

1. **`branch create --from tags/v1` on a non-tip tag**: the new branch's tip is the tagged block, not
   the origin tip; `log --ref heads/<n>` starts there. Revert the resolver call, show failure, restore.
2. **`tag create --target tags/v1`**: exits 1 with the precondition message; **assert the old mismatch
   text is absent.** And `tag create --target <block-id>` / `--target heads/main` still succeed.
3. **Consolidation**: every test that covered either function before covers the one after — list them.
   Run `inverse-plan`, `rollback-preview`, `rollback-draft-verify`, `checkout --patch-plan`,
   `--content-path` on the non-tip tag after the move; all name the tagged block.
4. **Full gate set**, EXECUTION-ORDER.md §6 rule 9, verbatim; cross-target: state the outcome from this
   round's own diff.
5. **Docs**: `branch.md`/`tag.md` (or wherever `--from`/`--target` are documented) say what each accepts;
   changelog `### Fixed` for §1, `### Changed` for §2's new refusal.

## 5. Not in this round

- `snapshot-plan`'s `integrity error` on a by-design missing snapshot — carried in §3d, separate pass.
- Anything behind the branch-only validator.
