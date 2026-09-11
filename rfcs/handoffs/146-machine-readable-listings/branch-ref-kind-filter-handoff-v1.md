# RFC 146 §8a — `branch` must stop listing tags, before `branch-list-v1` makes that a contract

**Ruled:** `rfcs/accepted/146-machine-readable-listings.md` §8a. Follows `3ccf6f69`, accepted.

**Your own incidental observation, promoted.** You noted, correctly, that `branch list` lists tag refs
too and that the missing filter predates your round. **Both true, verified here** — the shipped 0.38.0
binary prints `tags/v9 <id>` under `prikk branch`, byte-identical to your refactor's prose.

**What changed is the cost, not the fact.** In prose, a tag under `branch` is a display slip a human
notices. In `branch-list-v1` it is **structured data asserting that a tag is a branch, with a `closed`
boolean that means nothing for it.** Fixing prose later is a display change; fixing a shipped schema
later means changing what `-v1` returns or minting `-v2`. **This is unreleased today. It is the cheapest
it will ever be.**

## 1. The fix, and it is small

`tag.rs:91` already does it for tags: `if ref_state_payload.kind != RefKind::Tag { continue; }`.
`branch.rs:101` calls `list_ref_pointers()` and pushes every pointer at `:120` with **no kind check at
all**, while `branch.rs:207` already names `RefKind::Branch` when *creating* one.

**Filter the branch listing to `RefKind::Branch`, in both forms**, using the decoded ref-state the loop
already has in hand. **No new decode** — the `closed` flag comes from the same payload, so `kind` is
already there.

## 2. Answer one question the fix raises, do not assume it

**Received refs.** `branches` and `received` are separate arrays because the listing never decodes a
received ref's state. **Does a received *tag* appear under `received` today?** If `list_received_pointers`
returns every kind, the same defect exists there, and it cannot be fixed the same way without a decode
the round forbade. **Measure it, report it, and do not fix it silently either way** — if it needs a
decode, that is a scope decision for the RFC, not for the round.

## 3. Controls

1. **The agree-test for `branch` must have a tag in its fixture and assert it is absent from both forms.**
   Your existing fixture has one (`tags/v1` appears in your own JSON transcript) — turn its presence
   from an incidental into an assertion.
2. **`prikk tag` must still list it.** A filter that leaks into the wrong command is the failure this
   round could introduce.
3. **Perturb**: remove the filter, show the agree-test fail, restore it.
4. **Full gate set**, EXECUTION-ORDER.md §6 rule 9, verbatim; **state the cross-target addendum's
   outcome** — `branch.rs` counted `0` `cfg(target_os)` last round; count it again.

## 4. What this round must NOT do

- **No schema change.** `branch-list-v1` keeps its shape; it stops carrying a wrong row.
- **No prose wording change.** The tag line disappears; nothing else moves.
- **No fix to `received` without reporting first** (§2).
- **No fourth command.**
