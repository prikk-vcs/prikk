# RFC 142 follow-up — `show` must degrade on an unreadable blob, not fail the command

**RFC:** `rfcs/done/142-content-surface.md` — **§3a and §6a are new, added 2026-09-08**, and both
are corrections issued *with* your round's acceptance. **Read them first; §3a corrects my table, not
your code.**
**Base:** your own `0a52411`, which is **accepted and held unpushed** pending this. Commit on top.
**Round 1 review:** `.git-exclude/reviewed/142-content-surface-show-review-v1.md`.

---

## 1. What is accepted, and what is corrected

**Accepted:** the boundary, the controls, the cost table, the documentation, all ten gates re-run
independently against `0a52411`. The refusal to widen `lineage_horizon` was right and is recorded as
such.

**Corrected — and the error is mine, not yours.** You reported a *"pre-existing gap in
`worktree_patch::node_authoring::plan_delete`"*. **There is no gap.** `plan_delete` records
`base.blob_id`, which is the node's *correct* current content identity; after an `EditText` that
identity is deliberately unbacked (DC-65), and `current_text_for_node`'s own doc says the replay
fallback is *"expected, not exceptional"*. `patch_inverse.rs:227-243` never dereferenced it — it
validates against replay-derived bytes — so `show` is simply the first consumer to try to read it.

**Do not touch `worktree_patch`.** RFC 142 §3's table promised a read the store never undertook to
satisfy; §3a now says so.

## 2. The defect, which is `show`'s

```
$ prikk show <block containing DeleteNode of a previously EditText'd node>
error: integrity error: missing Blob 80a48fcf…
prose exit=1 · json exit=1
```

Create a file, seal; edit it, seal; delete it, seal. **The repository is intact and the command says
it is not.** Every other operation in that block goes unrendered because one blob was unreadable.

**Site:** `crates/prikk-store/src/show.rs:296-300` (`read_blob`), reached from `show_blob_content`.

## 3. RULED — §6a, and it is RFC 140 §7b applied unchanged

> a read may degrade, but it must say that it degraded, in a field a machine can branch on

**Requirements:**

1. **An unreadable blob degrades that operation. The command still renders every other operation and
   still exits `0`.**
2. **The degradation is a named, machine-branchable state carrying the blob id** — not a message
   string, not an omitted field, not an empty content value a consumer cannot distinguish from real
   empty content.
3. **All four dereference sites are bound**, not only the demonstrated one: `CreateFile`'s `blob_id`,
   `DeleteNode`'s file preimage, `ReplaceBinary`'s `old_blob_id` and `new_blob_id`.
   **Reachability-today is not the criterion.** A read surface does not fail hard on the cases it
   believes cannot happen; that belief is exactly what was wrong this round.
4. **A genuinely corrupt repository must remain distinguishable from this.** If you cannot separate
   "unbacked by design" from "object store damaged" at this layer, **say so and degrade both** — but
   say it in the report, and say what a `doctor`-level surface would need in order to tell them apart.
   Do not invent a distinction the data does not carry.

**The shape is already in your own file.** `resolve_node_path` (`:284-294`) returns
`ShowPathResolution::Unresolved { node_id }`. Content gets the same treatment. **Follow the local
precedent rather than inventing a second idiom** — two degradation shapes in one command is worse than
either.

## 4. Not ruled — yours to decide with the code in hand

- **Whether `ReplaceBinary` should read those blobs at all.** It reads two full blobs solely for
  `declared_size`, in a command that has ruled it will never render binary content. If the size can be
  had without the read, or is worth dropping when it cannot, **decide and say why.** Offered as
  material, not as a ruling.
- **Where the variant lives** — a new `ShowBlobContent` variant, or wrapping at the preimage/content
  level. Whichever reads cleaner at both renderers.

## 5. Controls

Each seen to fail before it passes.

1. **The exact sequence in §2 exits `0`** and renders the block's other operations. **This is control
   3 of round 1, rebuilt as originally specified.** Round 1 replaced it with a `ChangePerm` fixture
   that avoids the blob path — **a control rebuilt to avoid the thing it tests has stopped being a
   control.** This one must use `EditText`-then-`DeleteNode`.
2. **JSON exposes the degraded state as a field a consumer can branch on** without string matching,
   and a consumer cannot confuse it with successfully-read empty content. Assert both.
3. **Each of the four sites degrades**, driven directly at the `prikk-store` level where constructing
   the CLI-level case is impractical. A site with no test is a site that will regress.
4. **A block whose blobs are all readable is byte-identical to round 1's output.** The fix adds a
   state; it must not perturb the ordinary path.
5. **`show` still fails loudly where it should** — a nonexistent object id still exits `1`, a malformed
   one still exits `2` (RFC 121). **Degrading a missing blob must not soften the target-not-found
   path**, which is a different failure and stays a failure.

## 6. Documentation and CHANGELOG

- **`docs/src/guide/show.md` must state what a reader sees when content is unavailable**, and that it
  is not corruption. A user meeting this without warning will file a bug against their own repository.
- **`CHANGELOG.md`**: fold into the existing `## Unreleased` `show` entry rather than adding a second.
  `show` has not shipped; there is no fixed behaviour to describe a change against.

## 7. Gates

The full set, verbatim from `rfcs/EXECUTION-ORDER.md` §6 rule 9:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
- `cargo test --workspace --locked`
- `cargo +1.85.0 test --workspace --locked`
- `cargo +1.85.0 check --workspace --all-targets --locked`
- `git diff --check`
- `cargo audit --no-fetch`
- `RUSTDOCFLAGS="-D rustdoc::private_intra_doc_links" cargo doc --workspace --no-deps`
- release-policy `check`, `boundary-check`, `reference-check`

## 8. Reporting

`.git-exclude/review-request/`. Include:

- **the four sites and how each degrades**, and any you judged unreachable — with why, and why you
  bound it anyway;
- **your answer on §4's `ReplaceBinary` reads**;
- **whether "unbacked by design" and "store damaged" are separable here** (§3 requirement 4);
- **the cost note**: the round-1 table is now RFC 133 §5d — **filed by me, not by you.** `rfcs/` is
  architect-only, so a report saying a number was "filed into RFC 133" claims an action you cannot
  take. Write "to be filed by the architect".
