# RFC 144 — Two-point comparison: reopening `prikk diff`

**Status.** **PROPOSED 2026-09-08**, opened on the owner's instruction after they questioned the
refusal. **Not accepted; nothing may be implemented from it.**

**Author's note.** This RFC reopens a decision I made twice — RFC 142 §5/§7, restated in RFC 143 §7.
**It is opened because the ground under that decision moved, not because the owner pressed.** §2 says
precisely which part of my own argument no longer holds and which part survives.

---

## 1. Why this is reopened now

RFC 142 §5 refused `prikk diff <a> <b>` in these words:

> *It requires reconstructing both endpoints and comparing them: two replays and a tree comparison,
> which is the cost RFC 136's Option A exists to cut and which nothing has cut yet.*

**That was a cost argument, and RFC 143 largely answered it.**
`checkout --patch-plan --format json --content-path` reconstructs and exposes content at a point, and
its implementing round measured that path as indistinguishable from `show` at every depth tested.
**Endpoint reconstruction is no longer unbuilt or uncosted; it ships.** Two endpoints is now twice a
measured operation, not an engine nobody has written.

**The refusal as argued no longer holds. What survives is an argument §5 did not make.**

## 2. What survives, and what does not

### 2a. Does NOT survive: the cost objection

Superseded by RFC 143 (§1). **Still true and still worth carrying**: the headline use — comparing two
points far apart — remains the worst case for a replay-based implementation, and RFC 133 §5c records
that replay cost grows with depth. **A cost caveat is not a refusal.**

### 2b. Does NOT transfer: RFC 142 §6's refusal to synthesize a line diff

§6 refused inventing hunk structure for `show`, because **an `EditText` carries real content-anchored
spans and imposing line structure on them misrepresents what the patch says.**

**That argument does not reach `diff`.** A two-point delta *has no spans* — spans belong to individual
patches, not to the difference between two states. **For a two-point comparison a line-oriented
rendering is the honest representation, not a fabrication.** Recorded explicitly so this RFC is not
argued down with a rule that does not apply to it.

### 2c. SURVIVES, and is the real question: renames are not in the history

**`commit` never authors a rename.** `node_authoring.rs:12`, verbatim:

> *Out of scope (unchanged): rename inference (moves author as delete+create)*

**So `prikk diff <a> <b>` would report the commonest refactor in software as "one file deleted,
another created".** That is faithful to what the history records and **is a poor answer to the
question the user asked.** Once the command ships, that answer is our contract.

**This is the decision this RFC actually has to make**, and it may not be a decision about `diff` at
all — see §4.

### 2d. Already solved: partial replay coverage

RFC 143 §6 made the replay's coverage machine-readable, and unsupported operations error rather than
degrade. **`diff` would inherit a solved problem rather than a gap.**

## 3. What a two-point comparison would be for

**Recorded as the case to test the design against, not as justification.**

1. **What changed between two releases.** Audit and release-notes work, currently unanswerable by any
   surface we ship.
2. **What accepting this bundle would do to my repository** — the net effect of received work
   *before* an irreversible act. **The strongest case, because it is a safety question**, and it is
   the one shape `show` genuinely cannot serve: a reader would have to compose N per-patch answers
   themselves and get the composition right.
3. **The stikk project's `FR-033`** — narrowed and carried as a documented dependency rather than
   pressed. **Their own letter said this is not a blocker and they are shipping the smaller view.**

**Scenario 2 is the one to design against.** Scenario 1 is convenience; scenario 3 has a consumer who
has explicitly said they will live without it.

## 4. The question underneath, which may be the whole RFC

**If renames are not authored, no comparison surface can report them** — not `diff`, not anything
built on it. **So this RFC's central question may not be "what should `diff` show" but "should
`commit` author renames at all".**

**That is a patch-model question, not a read-surface question**, and it reaches further than this
RFC: `RenamePath` exists in the operation model and in `show`'s vocabulary, `ensure_apply_supported`
errors on it, and `node_authoring` never produces one. **The gap is in authoring, and everything
downstream inherits it.**

**RFC 143 §9 predicted this pressure exactly** — that exposing content would raise the case for
completing the replay, renames especially — and its implementing round reported feeling it. **That a
risk was predicted is not an argument that yielding to it is wrong.** It does mean the rename decision
should be made on its own merits and not smuggled in as a `diff` implementation detail.

## 4a. STRENGTHENED 2026-09-08 — the model states an invariant the authoring path does not keep

**Found while preparing the external review request, and it reframes this RFC.** Each link verified at
source:

1. **The model requires node identity to survive rename.**
   `crates/prikk-store/src/node/node_id_gen.rs`, module doc, verbatim: *"A `NodeId` is an opaque
   32-byte **stable** node identity, minted once at node creation and thereafter immutable: it **must
   survive rename**, edit, chmod, and binary replacement, and it is part of the text `span_id`
   preimage."*
2. **Span identity is derived from node identity.** `text_span.rs::compute_span_id_v2` folds
   `node_id.as_bytes()` into the hash preimage directly after the `PRIKK-TEXT-SPAN-v2` tag.
3. **Authoring resolves the baseline by path** — `node_authoring.rs` keys it
   `BTreeMap<String, BaselineFile>`.
4. **`commit` never authors a rename** — `node_authoring.rs:12`: *"rename inference (moves author as
   delete+create)"*.
5. **A fresh node draws a new 256-bit id** from the OS CSPRNG.

**Therefore a move mints a new node id and severs the old one, and because node id is in the span
preimage, every span identity on that file changes. Edits before and after a move share no lineage
the model can relate.**

**This changes what this RFC is.** `prikk diff` reporting delete+create is not a rendering choice we
could make differently — **it is the faithful report of a history in which the rename did not
happen.** The output is a symptom; §4's question is the disease.

**Stated against ourselves:** this is not a defect someone introduced. `RenamePath` is in the
operation model, `show` carries a `rename-path` label, and `ensure_apply_supported` errors on it as
*"node-addressed apply pending a rename authoring path"*. **The gap is deliberate and deferred.** The
open question is whether the deferral is still right — **and whether "the model contradicts itself" is
an overstatement of an ordinary unimplemented feature.** We reached that framing quickly, which is
when we are least reliable, and the external review request says so explicitly.

## 4b. CORRECTED 2026-09-08 — §4a was overstated, and the external review was right to say so

**§4a claimed the model contradicts itself. It does not, and the inflation was costing us.**

**The external architect's correction, which I accept:** the invariant in `node_id_gen.rs` is a property
of the `NodeId` **under the operation set** — *when a `RenamePath` is applied, the id is preserved*.
**That claim is true.** What is missing is not in the primitive and not in the operation. It is in
**authoring policy**: `commit` chooses to represent a move as delete+create rather than emitting the
`RenamePath` the model already defines.

**Verified at source rather than accepted:** `rename_node_checked`
(`prikk-replay/src/node_lifecycle/mutation.rs:145`) looks the node up by `node_id`, validates the
stated old path against the live one, then moves the path and keeps the id. **The operation preserves
identity by construction.**

**The honest finding, restated:**

> **prikk defines `RenamePath` and guarantees it preserves node identity, but `commit` never emits it,
> so real-world moves are recorded as delete+create. This is an undecided authoring policy, not a
> defect in the model or the id.**

**Why the correction matters and is not pedantry:** "the model contradicts itself" puts the `NodeId`
primitive and the `span_id` preimage on the table as suspect. **They are not. Nothing about the id draw
or the span hash needs to change.** The decision is contained entirely to authoring — a far smaller and
safer decision. **§4a distrusted its own framing because it was reached quickly; that instinct was
right and the framing was wrong.**

## 4c. The reframing, which is sharper than this RFC's own §4

**From the external review, and adopted:**

> **On what evidence may authoring ever assert that two nodes are one — and the answer is: only a
> declaration, never an inference.**

**Q2 and Q5 are one decision, not two.** Preserving identity across a move *is* the assertion "these
two are the same node". If that assertion can be minted from a similarity score, **it is a
provenance-laundering primitive**: file B inherits file A's identity and lineage. The mitigation is the
rule above.

- **A declared move** (`prikk mv`, or equivalent capture) is a truth a human asserted. **Author
  `RenamePath`; preserve the id.**
- **An inferred move** (content similarity) is *evidence, not identity*. **It must never silently mint
  an identity-preserving operation** — at most it proposes, and a human confirms.

**"Detect nothing, require declaration" is therefore defensible — on one condition**, which the review
states and this RFC adopts: **it is honest only if the declaration path exists and is ergonomic.** It
becomes abdication the moment there is no way to declare a move and the user is blamed for the
delete+create. **So the work this implies is not a detector. It is a capture path plus the authoring
that turns a declaration into a `RenamePath`.**

## 4d. VERIFIED — prikk already has the primitive Pijul built a dual-vertex model to get

**Both external notes hang their merge argument on a structural claim they could not check from
outside. I checked it.**

```
NodeLifecycleState {
    live_by_id:  BTreeMap<NodeId, LiveNode>,   // primary index
    path_to_id:  BTreeMap<RepoPath, NodeId>,   // secondary
}
LiveNode { path, kind, content }               // path is an attribute of the node
```

Its own doc: *"Live-node uniqueness is structural: `live_by_id` is keyed by `node_id`."*

**Replay keys on identity; path is a mutable attribute and a secondary lookup.** Per the external
prior-art note, that is precisely Pijul's split — an identity vertex and a name vertex — which Pijul
pays a whole model for **in order to make renames commute with edits.** **prikk already has it.**

**Consequence: the merge property is structural, not hoped.** A declared rename would be an operation
the merge simply carries, and an `EditText` — node-addressed — follows the moved node with no path
heuristic.

## 4e. The prior-art lesson, attributed

**From the external architect's note** (their sources and their reading; recorded here as their
finding, not as this project's independent verification):

- **Both Darcs and Pijul model moves explicitly and never infer them**, and the reason is merge, not
  display: **an inferred rename cannot participate in a sound merge** — it is invisible to commutation
  and re-guessed, possibly differently, by every reader.
- **Darcs' scar:** putting conflict *reasoning* inside the patch algebra made merges exponential in
  conflict size; conflictors helped and recursive cases still blow up. **The lesson: keep rename an
  ordinary explicit operation on a stable identity, and keep detection and resolution out of the
  merge core.**
- **Pijul's limit, which is not ours to inherit:** it keeps conflicts *in* the data structure and never
  loses information — a conflict-tolerant model. **prikk's claim is authored, verifiable history where
  a human resolves and the resolution is recorded.** Take Pijul's primitive; leave its conflict model.

**Open, and worth its own check:** Pijul's "inode" vertex is an internal identity minted from a change
hash — **not** the OS `st_ino` this RFC's Q2 rejected as non-portable. **The naming collides with the
exact thing we ruled out and they are opposites.**

## 4f. RULED-SHAPED — the `diff` question splits, and the split is the answer

**Adopting the external review's Q3 answer, which is better than either of this RFC's own options.**

**Decline a generic `prikk diff`.** Owning it means owning what "difference" means forever — across
renames, permissions, binaries, symlinks and merges the replay does not walk. **That is an unbounded
contract and "clean over rich" declines it.** A consumer holding RFC 143's content-at-a-point for two
points can compose the generic comparison correctly.

**Own exactly one case: what accepting this bundle would do to this repository.** §3's scenario 2.
**It is not a convenience comparison; it is a safety surface consulted before an irreversible act**,
and a consumer who composes a safety answer wrongly emits a confident wrong answer at the worst
moment.

**And it is probably not `diff`-shaped.** It is anchored at HEAD — *what does this do to me* — not
*what differs between two arbitrary points*. **Naming it `diff` would be the tail wagging the dog.**

**Publish a recipe for the rest**, so declining the general command is a documented position rather
than a silence.

## 5. What must be ruled before anything is built

1. **Renames** (§2c/§4) — report delete+create honestly, infer renames at comparison time, or author
   them. **Each has a different blast radius and the third is not a read-surface change.**
2. **State-level or content-level, or both.** Which paths differ is cheap and composes from what
   ships; content per path is the expensive half and RFC 143 already exposes it per path.
3. **Whether `diff` is a command at all**, or whether the honest answer is that RFC 143's surface
   plus a documented recipe is enough. **A consumer who can already fetch content at two points may
   not need us to own the comparison.**
4. **What it must never claim.** RFC 142 §6's discipline applies in spirit even where §2b's specific
   argument does not: **the output must not imply structure the repository does not record.**

## 6. What this RFC does NOT do

**It does not accept `prikk diff`.** RFC 142 §5's split stands until this RFC is ruled. **It does not
authorize rename authoring**, which is §4's question and is larger than this document.

**And it does not treat the requesting consumer as a reason.** stikk asked, narrowed, and said plainly
it was not a blocker. **This is reopened because the architect's own stated reason expired**, which is
the only good reason to reverse a refusal.

## 7. Risks

- **Reversing a twice-stated refusal costs credibility if done casually.** §2 exists so the reversal
  is traceable to a specific superseded premise rather than to persistence.
- **§4 could swallow this RFC.** If the answer is "author renames", that is a patch-model change with
  its own schema, authoring, replay and conflict consequences. **This RFC must be allowed to conclude
  "the real work is elsewhere" rather than forcing a `diff` shape onto a rename problem.**
- **A comparison surface is a permanent contract** in a way `show` is not: `show` reports what a patch
  says, `diff` asserts what two states mean relative to each other.
