# RFC 142 — `prikk show`: what a block or patch changed

**RFC:** `rfcs/accepted/142-content-surface.md` — **accepted in full 2026-09-08.** §4's ruling (resolve
paths, no opt-out flag), §5's split (`show` ships, `diff` does not) and §6's span rendering are settled
input.
**Base:** `main` at `c27aad4`.

**§2 is the good news and §3 is where the work is.** The content you need is already in the patch; the
paths are not, and getting them needs one narrow new function rather than widened internals.

---

## 1. What to build

**`prikk show <block-id|patch-id> [--format json]`** — what that block or patch changed: each
operation, its kind, the path(s) it affects, and the content it changed.

**Both forms in this increment.** Prose and `--format json`, per the standing ruling that a content
surface's machine-readable form is designed at the same time, not a release later. Schema name in the
established idiom (`verify-report-v1`, `trust-list-v1`, `status-report-v1`), naming this command and
**settling the format for `show` and nothing else**.

**Exit code `0` whichever way it resolves** (RFC 121). A block that changed nothing is an answer.

## 2. The content is already in the patch — no replay

**Verified at `crates/prikk-object/src/payload/patch/operations.rs`:**

| operation | what it carries | reads |
|---|---|---|
| **`EditText`** | **`old_span_text` and `replacement_text` — the bytes, verbatim** — plus `presentation_hint_line`/`_column` | **none** |
| `CreateFile` | `path`, `blob_id`, `mode` | one blob |
| `DeleteNode` | `path`, preimage (`old_blob_id`/`old_mode`, or `old_target`) | one blob |
| `RenamePath` | `old_path`, `new_path` | none |
| `ChangePerm` | `old_mode`, `new_mode` | none |
| `CreateSymlink` | `path`, `target` | none |
| `ReplaceBinary` | `old_blob_id`, `new_blob_id` | two blobs |

**`BlockPayload.patch_ids: Vec<ObjectId>`** gives block → patches directly. So `show <block>` is: read
the block, read its patches, decode operations. **No lineage walk for content.**

## 3. Paths — one replay, and a new narrow function you will have to add

**Three kinds are node-addressed** (`EditText`, `ChangePerm`, `ReplaceBinary`) and carry no path.
Resolving them at a sealed block needs that block's lifecycle state.

**The pattern already exists**, at `crates/prikk-store/src/merge_evidence.rs:49-51`:

```rust
let object_store = ObjectReadSnapshot::open(layout)?;
let horizon = lineage_horizon(&object_store, block_id)?;
let replay  = replay_derived_state(&object_store, block_id, horizon)?;
```

**But you cannot call it from the CLI as it stands, and I checked rather than assuming:**

- `lineage_horizon` is a **private `fn` inside `merge_evidence.rs`** — not `pub(crate)`.
- `replay_derived_state` is `pub(crate)` in `prikk-store`.
- `ReplayDerivedLifecycleState`'s fields (`baseline_block_id`, `state`) are **private**.

**So add ONE narrow public function in `prikk-store` that returns already-summarized public types** —
the shape RFC 140 used for `enumerate_queued_patches`, which kept `resolve_folded_worktree_baseline`,
`NodeLifecycleState` and `DecodedPatchOperation` at their existing visibility. **Do not widen
`replay_derived_state`, `NodeLifecycleState`, or `ReplayDerivedLifecycleState`.**

**RFC 131 landed the crate's first 27 `pub(in crate::…)` two days ago.** Widening internals now would
work directly against that; say in your report which boundary you chose and why.

**Resolve once per invocation, not once per operation.** One replay for the whole `show`.

**An unresolved node id is reported, never fatal** — RFC 140 §4's ruling applies unchanged. The
reachable case: a block that edits a node *and deletes it* in the same block. Report the operation with
its node id, marked unresolved, and keep going.

## 4. Render the span, not a synthetic diff

**RFC 142 §6, and this is the instruction most likely to be violated with good intentions.**

`EditText` gives you a before-span and an after-span. **Render that.** Do **not** synthesize a unified
diff with line numbers, hunks and context.

prikk's edits are **content-anchored spans, not line ranges** — that is the whole of RFC 134 — and the
presentation hints are explicitly *"not part of algebraic identity"*. A line-oriented rendering
**asserts a structure the format does not have**, and the first user to pipe it into `patch(1)` finds
out. The hints may inform display; they may not become the unit.

## 5. Out of scope

- **`diff <a> <b>`.** RFC 142 §5 refuses it here: two replays plus a tree comparison, inheriting the
  **depth^1.45** cost RFC 136 §9.3 measured. Not this increment, and building "half of it" is worse
  than not starting.
- **Any schema change**, and in particular **do not add a path to node-addressed operations**. RFC 140
  §7 refused this once for a display problem; the same temptation returns here at larger scale.
- **Any new stored state, snapshot or cache.** If `show` is slow at depth, that is evidence for
  RFC 136, not a reason to invent a side table.
- **Binary content rendering.** Report blob ids and sizes for `ReplaceBinary`; nobody has asked to see
  binary bytes in a terminal.
- **`--format json` on any further command.**

## 6. Controls

1. **A text edit shows its before and after.** Edit a committed file, seal, `show` the block: the
   operation reports the old and new span text. **Write this first** — it is the whole point.
2. **A node-addressed operation resolves to a path.** Same case: the entry names the file, not a node
   id. This is RFC 140's control 1 one level up, and it fails the same way if resolution is skipped.
3. **An unresolved node id does not fail the command.** Edit and delete the same node in one block;
   exit `0`, every operation reported, that one marked unresolved.
4. **A mixed block.** Create, edit, delete, rename in one block — each renders with its own content and
   path(s); the rename reports both endpoints.
5. **`show` on a patch id and on a block id both work**, and a block's output is the union of its
   patches in order.
6. **Nothing is a synthetic diff** (§4). Assert the output contains no hunk header (`@@`) and no
   line-number gutter.
7. **JSON parses and carries its `schema_version`.** This crate has no `serde_json` — mirror
   `rfc138_trust_read_surface.rs`'s hand-written checker, as RFC 138 and RFC 140 both did.
8. **Exit `0` for a block that changed nothing**, and `2` for a malformed or missing id.

**Each control seen to fail before it passes**, with the perturbation reported.

## 7. Cost

**Measure and report `show`'s own wall clock at depth** — shallow and deep — the way RFC 140's round
measured enumeration. **It goes into RFC 133**, joining §5b (quadratic seal) and §5c (checkout and
merge at depth^1.45) as the third measured-but-ungated cost.

**You already know the shape**: content is O(operations), path resolution is one replay at the target's
depth. **Report the number, not the expectation.**

## 8. Gates

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

**`boundary-check` carries the RFC 130 coupling gate, and RFC 131 just reshaped the module graph.** A
new `prikk-store` function reaching across modules can move an edge or a hub. Run it early; if it
fires, the entry needs a reason **and** a `what_would_remove_it`, and a **new cycle is a finding to
report rather than a declaration to write quickly** (RFC 131 §6b.1).

**Docs are a gate.** `docs/src/reference/commands.md` is a declared document and must gain the `show`
line; check rule (A)/(B) in `crates/prikk-cli/src/commands/tests.rs` if unsure.

## 9. This is a user-facing surface, so it needs a `CHANGELOG.md` entry

**Under `## Unreleased`.** Name what it does and what it does not: `show` renders what a block or patch
changed; **there is no `diff`**, and saying so prevents the obvious wrong inference.

**This is also the entry that makes the next release non-empty** — nothing user-facing has landed since
0.35.0.

## 10. Reporting

`.git-exclude/review-request/`. Include:

- **which boundary you added in `prikk-store`**, and why that rather than widening existing internals
  (§3);
- **the cost numbers** from §7;
- the per-control perturbations;
- **anything that made you want to synthesize a line diff** (§4) — that pressure is worth recording
  even when resisted, because the next person will feel it too.
