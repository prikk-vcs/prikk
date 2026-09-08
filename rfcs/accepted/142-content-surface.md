# RFC 142 — Showing what changed

**Status.** **ACCEPTED by the project owner 2026-09-08**, the same day it was opened.

**Moved to `rfcs/accepted/` on acceptance** — the trigger is design complete, not handoff issued.

**What the acceptance covers.** The whole design as written: §3's finding that a patch is
self-describing; §4's ruling that paths are resolved despite costing one replay, with **no flag to skip
it**; §5's split — **`show` ships and `diff` does not**, with both prose and `--format json` in the same
increment, scoped as RFC 138 §7.2 scoped its own; §6's refusal to synthesize a line-oriented diff over
what are content-anchored spans; and §7's refusals, including **no schema change and no path added to
node-addressed operations**.

**Handoff issued 2026-09-08:** `rfcs/handoffs/142-content-surface/show-handoff-v1.md`.

Originally opened as: **PROPOSED, 2026-09-08**, at the project owner's instruction, as item 1 of the
current order.

**Author-review independence.** The architect wrote this RFC and is also its only reviewer — the
standing gap on every architect-authored design here. Compensated at implementation review.

**Tracks.** A read surface. **No change to what is stored, sealed, or verified**, and no schema change
of any kind — §7 refuses that explicitly, because the temptation exists.

---

## 1. Why this is opened now, and it is not only because it was asked for

**Three separate threads converge on this one surface, which is why it outranked everything else on
the board:**

- **It is the only queued item that produces a user-facing change.** Everything else open — the
  coupling gate's qualified names, a second corpus profile, publication plumbing — is internal. A
  release cut today would ship binaries byte-identical to 0.35.0.
- **The held reply to the stikk project is gated on 0.36.0 shipping**, and 0.36.0 has no other
  trigger.
- **The hosting-shape RFC is gated on this.** A `prikk instaweb`-style browse view can name changed
  paths and show **no line of what changed** — the same wall the external consumer hit.

**And it is their oldest carried dependency.** stikk's letter 003 §1: `UD-09`, open since their 0.1.0,
with two designed and reviewed views (`FR-033` Compare, RFC 006 §3b Patch detail) unbuilt — *"not
deferred for effort, but because building them would mean rendering a diff we did not read."*

## 2. What exists today: nothing

Measured against the command inventory: **there is no `show` and no `diff`.** `log` prints block ids,
`status --format json` prints patch ids and resolved paths, `checkout --patch-plan` prints a plan,
`merge-evidence`/`merge-plan` are read-only analysis. **Nothing renders content.**

## 3. The finding that shapes the whole design: a patch is self-describing

**The expensive thing is not needed.** Every operation carries what it changed, in the patch itself:

| operation | what the patch carries | reads needed |
|---|---|---|
| **`EditText`** | **`old_span_text` and `replacement_text` — the actual bytes, verbatim** — plus optional line/column presentation hints | **none** |
| `CreateFile` | `path`, `blob_id`, `mode` | one blob |
| `DeleteNode` | `path`, and a preimage carrying `old_blob_id`/`old_mode` (or `old_target`) | one blob — **which may not exist; see §3a** |
| `RenamePath` | `old_path`, `new_path` | none |
| `ChangePerm` | `old_mode`, `new_mode` | none |
| `CreateSymlink` | `path`, `target` | none |
| `ReplaceBinary` | `old_blob_id`, `new_blob_id` | two blobs |

**So showing what a patch changed requires decoding the patch and, at most, reading a blob per
affected file. It requires no replay.** For the commonest case — a text edit — the before and after
bytes are *in the operation*, because the inverse-plan machinery needs them there.

**This is the opposite of what the cost analysis would have predicted**, and it is why this RFC can be
opened without waiting on RFC 136's snapshot work.

## 3a. CORRECTION 2026-09-08 — "one blob" is a blob *id*, and an id is not a promise of an object

**§3's table above was wrong, and it was mine.** It read a blob id in a payload as a blob that can be
read. For `DeleteNode` that does not hold, and the gap is not a defect anywhere — it is the design
working as ruled.

**DC-65.** Once a text node has been edited, its current content identity has no stored object: the
edit is carried as `old_span_text`/`replacement_text`, and the resulting content is reconstructed by
replay rather than written as a blob. `plan_delete`
(`crates/prikk-store/src/worktree_patch/node_authoring.rs:743`) therefore records `base.blob_id` — the
node's **correct** current content identity — and that identity is deliberately unbacked.
`current_text_for_node` (`:894`) says so in its own doc: falling through to replay-based
materialization is *"expected, not exceptional, for any node whose most recent sealed operation was an
`EditText`"*.

**Nothing downstream was broken by this, because nothing downstream dereferenced it.** The inverse
path (`crates/prikk-store/src/patch_inverse.rs:227-243`) *validates* `old_blob_id` against
replay-derived bytes via `ensure_blob_matches_node_kind`; it never reads the blob. `show` is the first
consumer to dereference a preimage blob id, and so the first to meet the case.

**RULED: this is not an authoring defect and no `worktree_patch` change is warranted.** The blob id is
right. What was wrong is the row above, which promised a read that the store never undertook to
satisfy. **The obligation is on the reader, and §6a states it.**

**The same correction applies to the `ReplaceBinary` row's "two blobs"** as a matter of form: those
ids, too, are ids. They are backed today; the reader may not assume they always will be.

## 4. The one expensive dimension, and it is already measured

**Three of the seven kinds are node-addressed** — `EditText`, `ChangePerm`, `ReplaceBinary` — and
carry a 32-byte node id rather than a path (RFC 140 §3). **Resolving those to paths at a sealed block
needs that block's lifecycle state, which means replay.**

**RFC 136 §9.3 measured exactly that**: checkout and merge-evidence both grow as **depth^1.45**, from
a full uncached chain walk, and cost tracks replay depth rather than tree size.

**So `show` is cheap in content and superlinear in path resolution** — one replay per invocation, at
the target block's own depth, not one per operation.

**RULED: resolve paths anyway, and state the cost.** A surface that shows a human a 32-byte node id
where a filename belongs has failed at the thing it exists to do. **Do not add a flag to skip
resolution** — no consumer has asked for the cheap form, and this project does not build for
hypothetical ones. If one asks, that is a later, easy addition.

## 5. RULED — two surfaces, not one, and only the first ships here

**`prikk show <block-id|patch-id>` — what this block or patch changed.** Content from the patches
themselves (§3), paths resolved once against the target's own lifecycle state (§4). **This is the
whole of the increment.**

**`prikk diff <a> <b>` — an arbitrary two-point comparison — is NOT in this RFC.** It requires
reconstructing both endpoints and comparing them: two replays and a tree comparison, which is the
cost RFC 136's Option A exists to cut and which nothing has cut yet. **Opening `show` does not commit
this project to `diff`**, and shipping `diff` on today's replay cost would be shipping a command whose
headline use — comparing two points far apart in history — is its worst case.

**This maps exactly onto the consumer's two blocked views**, which is corroboration rather than
coincidence: their Patch detail view needs `show`; their Compare view needs `diff`. **One of the two
is available now at honest cost.**

**Both `--format json` and prose, in the same increment.** The standing ruling — *if a content surface
lands, its JSON form is designed at the same time, not a release later* — applies here first. **Scoped
as RFC 138 §7.2 scoped its own**: a schema name for this command, settling the format for `show` and
nothing else. The general machine-readable surface remains unopened.

## 6. What "showing" means for a text edit, and the one thing not to invent

**Render the operation, not a reconstructed file.** For `EditText`, the patch gives `old_span_text`,
`replacement_text`, and presentation hints — that is a before/after span, and it is what actually
happened.

**Do not synthesize a unified diff with line numbers and context.** prikk's edits are
**content-anchored spans, not line ranges** — that is the whole of RFC 134 — and the presentation
hints are explicitly *"not part of algebraic identity"*. A rendering that presents spans as a
line-oriented patch would be **asserting a structure the format deliberately does not have**, and the
first user to apply it with `patch(1)` would find out. **The span is the truth; render the span.**

## 6a. RULED 2026-09-08 — an unreadable blob degrades the operation, it does not fail the command

**`prikk show` currently exits `1` on an ordinary sequence.** Create a file, edit it, delete it, seal
each: `show` on the delete block reports `error: integrity error: missing Blob <id>` and exits `1`, in
prose and in `--format json` alike. The repository is intact; the command is wrong about it.

**This is RFC 140 §7b, and that ruling stands unchanged here:** *a read may degrade, but it must say
that it degraded, in a field a machine can branch on.* A read surface that cannot render one operation
must still render the other operations, and must still exit `0`. Turning an intact repository into an
integrity error is the failure mode §7b exists to forbid.

**The shape to follow is already in the same file.** `resolve_node_path`
(`crates/prikk-store/src/show.rs:284-294`) meets an unresolvable node id and returns
`ShowPathResolution::Unresolved { node_id }` — a named variant a consumer can branch on. **Content
gets the same treatment**: an unreadable blob becomes a declared, machine-readable "unavailable"
state carrying the blob id, not a `PrikkError`.

**This binds every blob dereference in the command, not only the one that was demonstrated.** There
are four: `CreateFile`'s `blob_id`, `DeleteNode`'s file preimage, and `ReplaceBinary`'s `old_blob_id`
and `new_blob_id`. Whether each is reachable today is not the criterion — a read surface does not get
to fail hard on the ones it believes cannot happen.

## 6b. CORRECTION 2026-09-08 — §6a's requirement 4 was too broad, and the fix inverted the defect

**§6a said: if you cannot separate "unbacked by design" from "object store damaged", degrade both.
That is true of *absence* and of nothing else, and I did not say so.** The first implementation read
it as written and degraded every failure, which is a defensible reading of my words and the wrong
behaviour.

**Absence is ambiguous. An error is not.** When the object store returns `Ok(None)` there is nothing
to tell the two causes apart — DC-65's deliberately unbacked identity and a lost object look
identical, and always will. But when it returns `Err`, it is *affirmatively reporting damage*:

- `read_object_at_entry` (`crates/prikk-store/src/object_store.rs:130-135`) recomputes the envelope's
  id and returns `Integrity` when it does not match the index's claim. **That is content-hash
  verification — silent-corruption detection, the strongest integrity signal a content-addressed
  store has.**
- The same function returns `Integrity` when index and envelope disagree about object type (`:136-141`).
- `read_typed` returns `ObjectTypeMismatch`; `validate_read_schema` rejects an unreadable format;
  `BlobPayload::decode_canonical` rejects a malformed payload; and any of these may instead be an
  `Io` error from a failing disk.

**None of those is "unbacked by design." Every one of them is a repository or machine problem, and a
read surface that renders them as ordinary content-not-stored is lying about the repository** — the
exact defect §6a was written to remove, with its sign reversed.

**RULED: degrade absence, propagate error.**

- **`Ok(None)` → the declared unavailable state, exit `0`.** This is DC-65's case and §6a stands
  unchanged for it.
- **`Err(_)` → propagate. Exit `1` (RFC 121: operational failure).** A corrupt object store is an
  operational failure and `show` says so.

**The structural cause is one line and the fix is there.** `read_blob`
(`crates/prikk-store/src/show.rs`) collapses `Result<Option<ObjectEnvelope>>` into `Result<_>` with
`.ok_or_else(...)`, destroying the distinction before its caller can branch on it. **Keep the
`Option`.** The information the reader needs is already coming out of the object store; only this
function throws it away.

**This narrows what the unavailable state means, and the guide page must follow.** It no longer
covers "possibly damage" — after this ruling it means the object is absent, whose overwhelmingly
common cause is DC-65.

## 7. What this RFC does not decide, and what it refuses

- **`diff`** (§5). Its own RFC when the replay cost is addressed.
- **The general machine-readable surface.** Still unopened (RFC 138 §7.2).
- **Any schema change.** In particular, **do not add a path to node-addressed operations.** RFC 140 §7
  already refused this once for a display problem; the same temptation returns here at larger scale
  and gets the same answer.
- **Any new stored state**, snapshot, or cache. If `show` is too slow at depth, that is RFC 136's
  problem and this RFC's evidence, not a reason to invent a side table.
- **Binary content rendering.** `ReplaceBinary` reports the blob ids and sizes; showing binary bytes in
  a terminal is not a thing to design without someone asking.

## 8. Scope

**In:** `prikk show`, prose and `--format json`; content from patch payloads and blobs; path
resolution against the target's lifecycle state; the exit-code contract (RFC 121: `0` for an answer,
whether or not the thing changed anything).

**Out:** everything in §7.

## 9. Risks

**The cost is superlinear and we now know it in advance.** §4 is not a discovered surprise; it is a
measured curve inherited from a known mechanism. **This RFC's increment should report `show`'s own
cost at depth into RFC 133**, joining §5b and §5c — the third measured-but-ungated cost in the same
table.

**A cheap surface invites a wide one.** `show` answers "what did this change?" Every neighbouring
question — what does this file look like now, what changed between these two points, who changed this
line — is a different and more expensive question. **The discipline is to ship the cheap one and let
the expensive ones be asked for**, not to widen `show` until it becomes `diff` by accident.

**Related:** RFC 140 (queued-patch enumeration — the same path-resolution problem one level down, and
the precedent for how it was answered), RFC 136 §9.3 (the measured replay cost `diff` would inherit),
RFC 138 §7.2 (the surface-scoping precedent), RFC 134 (why the span is the unit, not the line),
RFC 121 (the exit-code vocabulary).
