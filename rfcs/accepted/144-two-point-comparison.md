# RFC 144 — Two-point comparison: reopening `prikk diff`

**Status.** **ACCEPTED by the project owner 2026-09-09**, after three rounds of external review.

**What the acceptance covers, stated because a bare acceptance is scope-ambiguous:**

- **`prikk diff` as a general command is declined, not deferred** (§4f). A recipe over RFC 143's
  surfaces is published instead.
- **The pre-acceptance bundle-impact preview is owned** (§4f), and is **separable from everything else
  here** (§4g.3).
- **Identity-preserving rename authoring follows only from a declaration, never from an inference**
  (§4c) — the rule that makes the operation safe rather than a lineage-forgery primitive.
- **Provenance rides in the signature, not the operation** (§4h.2). **No `Patch` schema change.**
- **Trust policy is never an input to replay** (§4h.3). Provenance is a read-time annotation; facts are
  shared, judgements are local.
- **The honesty invariant** (§4i.1) binds every read surface built after renames are authored, and is
  to be enforced structurally rather than by review.
- **`patch_replay` must learn to apply `RenamePath` before any rename is authored** (§4h.7) — a gate,
  not a follow-up.

**What acceptance does not settle:** the wording and shape of the capture path itself, and the
thirteenth conflict witness's own field set. **Both belong to their implementing increments.**

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

## 4g. FOUND 2026-09-08 — three things that change the size of the decision

**Verified at source while preparing this RFC for acceptance.**

### 4g.1 Two replay paths disagree about `RenamePath`, so authoring one today would break checkout

- **`lifecycle_cache/replay/effect.rs:67` handles it** — calls `rename_node_checked`, preserving the
  id. This is what `show` and lifecycle state use.
- **`patch_replay/decode.rs:145` refuses it** — `unsupported_operation("RenamePath (node-addressed
  apply pending a rename authoring path)")`. **This is the path `checkout --patch-plan`,
  `--patch-materialize`, and therefore RFC 143's content-at-a-point all use.**

**So the moment `commit` authors a rename, every checkout and every content-at-a-point read over that
history fails.** The work is not "teach `commit` to emit it" — **it is that plus teaching the
materialization path to apply it.** §5's list understated the scope and this corrects it.

**The refusal is loud, not silent**, so nothing would be quietly wrong — but the feature is
strictly larger than the authoring change it looks like.

### 4g.2 `RenamePath` carries no provenance field — and it may not need one

Its fields are `node_id`, `old_path`, `new_path`. **Nothing records whether the assertion was declared
or inferred**, which §4c's rule says must be distinguishable.

**But the rule can be satisfied without a schema change.** If authoring never mints an inferred
rename, **every `RenamePath` in history is declared by construction** — the guarantee comes from the
authoring rule, not from a per-operation field. Recording "declared" on every operation when no other
kind exists is noise.

**The cost is deferred, not avoided:** if a later version ever mints inferred renames, it must add the
field then, and absence would have to mean *"authored in an era that only declared"*.

**QUESTIONED 2026-09-08 — this reasoning has a hole and it may be wrong.** *Absence means declared*
holds only if **every writer follows the rule**, and **prikk accepts patches authored elsewhere**:
`patch_exchange::accept_exchange_artifact` takes an artifact from another repository. Today that is
moot because nothing anywhere authors a rename — **but the moment prikk authors them, the format
permits any writer to**, and a `RenamePath` crossing that boundary carries no evidence of how it was
minted. **The guarantee is not enforceable across implementations; it is a hope with a good shape.**

**Three candidate answers, none ruled**, put to the external architect whose own system carries a
`Stated`/`Derived` distinction for exactly this reason: a per-operation field (a schema change); lean
on the **signer already recorded** and make it a question of which authors you trust rather than a new
field; or **refuse on receipt** any `RenamePath` whose provenance cannot be established — fail-closed,
at the cost of making renames unexchangeable.

**Recommendation, pending that answer: no schema change now.** A `Patch` schema bump is a release-compatibility event
(0.31.0 is the precedent — the first release older builds could not read) and this does not earn one.

### 4g.3 The bundle-impact preview is separable, and cheaper than §4f implies

**It does not depend on the rename decision at all.** It is *bundle against HEAD* — one replay of the
current state plus applying the bundle's own patches — **not two arbitrary points.** A bundle
containing delete+create for a move previews delete+create: honest, and limited in exactly the way
today's history is.

**So the two halves of this RFC can be accepted separately.** The preview could be authorized and
built while the rename question stays open, and it would not need revisiting when the rename question
is settled.

## 4h. RESOLVED 2026-09-08 — provenance rides in the signature, not the operation

**Second external review round. The exchange-boundary hole §4g.2 raised is answered, and the answer is
cheaper than either candidate this RFC proposed.**

### 4h.1 It is not the same problem as the reviewer's own system, and the difference is our advantage

Their system stamps a per-operation `Stated`/`Derived` marker because it **decodes a bag of facts from
an unaccountable source** — one import mixes a recorded rename and an inferred one under the same
"author", and **there is no one to hold accountable.**

**prikk infers nothing and receives signed artifacts.** The question is not *"did I infer this"* — we
did not — but *"did the author who signed this patch have standing to assert this rename, and can we
hold them to it?"* **That is a trust question about an assertion crossing a boundary, not an epistemic
question about a derivation.**

### 4h.2 RULED-SHAPED — a per-operation provenance field would be forgeable, and is therefore wrong

**Within one signed patch, provenance is uniform**: every `RenamePath` was minted by one author under
one regime, so a per-operation marker is redundant. **Worse, it is forgeable** — a marker the asserter
fills in themselves proves nothing across a trust boundary, because a hostile writer simply stamps
`declared` on an inferred rename.

**The only unforgeable provenance is who signed the patch.** So the record that a rename is a real
declaration is **a trusted author's signature over the patch, not a boolean on the operation.**

**Therefore: no `Patch` schema change** — §4g.2's conclusion survives, but for a sound reason instead
of a hopeful one. **This also corrects the first external round's own advice**, which said *"the
operation must record which it was"*; the reviewer withdrew the placement, not the principle.

### 4h.3 Demote, do not reject — CORRECTED: the demotion cannot touch history

**The external answer was "degrade the `RenamePath` to delete+create on receipt". The intent is right
and the mechanism is impossible here.** Three checks, verified at source:

1. **The patch is signed** — rewriting an operation inside it breaks the author signature.
2. **The patch is content-addressed** — rewriting changes its object id, so the block's patch list no
   longer refers to it.
3. **Replay is bound into the block.** `derive_next_state_root(reader, parent, patch_ids)` derives a
   Merkle root from parent state plus patches, and `BlockPayload.state_merkle_root` carries it
   **inside the signed block**; `verify` re-derives and compares.

**So a repository that replayed a `RenamePath` as delete+create would derive a state root disagreeing
with the block's own, and its `verify` would fail.** Two repositories with different trust policies
would compute different histories from identical bytes.

**RULED: trust policy must never be an input to replay.** Replay is a pure function of the patch set,
and that determinism is what makes history verifiable at all.

**What survives — and it is the whole intent: provenance is a read-time annotation, not a history
mutation.**

- The patch records `RenamePath`. **Signature, patch id, state root and replay all hold, identically
  everywhere.**
- **Whether a reader believes the identity claim is a function of (signer, that reader's trust
  policy)** — computed on read, never stored, never fed to replay.
- **Facts are shared; judgements are local.**

**And the placement question answers itself from this project's own precedent.** RFC 140 §7b and RFC
142 §6b already rule that *a read may degrade, but must say so in a field a machine can branch on*.
**So the annotation belongs on the read surface in a branchable field, not in a side record** — a
consumer asking *what changed here* is told, in the same document, that this rename's identity
assertion comes from a signer outside their trust policy. **They cannot miss it, because it is in the
answer rather than beside it.**

### 4h.3a The original external framing, kept for the record

**A receiving repository that does not trust a signer to assert identity should neither refuse the
patch nor accept the claim.** It should **degrade the `RenamePath` to its honest floor — delete +
create** — keeping the content change (always true), dropping the identity assertion (unverifiable
across the boundary), and **recording that the demotion happened.**

**Refusing loses the content; accepting forges lineage; demoting keeps the content and refuses only
the forgeable claim.** Renames stay exchangeable and forged lineage stays impossible. **It is
fail-closed applied to the claim rather than to the patch.**

### 4h.4 VERIFIED — this probably needs no new machinery

`patch_exchange::accept_exchange_artifact` **already** verifies the AUTHOR signature against recorded
material and **already** loads the maintainer trust policy, verifying claim and tag signatures against
it. **The trust decision exists at the boundary today.**

**So the provenance question may collapse entirely into the acceptance decision already being made:**
accept the patch → its renames are declarations by a trusted author; do not trust the signer for
identity → demote. **Open, and ours to rule: whether "trusted to contribute" and "trusted to assert
identity" should ever be separate grants.** Start from the assumption they are the same; split only if
a real threat forces it.

### 4h.5 The residual cost, and the fix that is not coercion

**Declaration-only cannot punish (§4g's resolution), but one cost survives:** a user who *would* have
declared, and did not know the path existed or forgot, gets delete+create **sealed permanently**. That
is not a regression — it is an **irreversible, commit-time-invisible opportunity cost.**

**RULED-SHAPED: the fix is discoverability, not a mandate.** When `commit` sees a delete+create that is
a strong move candidate, it may **hint** — *"looks like you moved X to Y; `prikk mv` would preserve its
identity"* — **without authoring anything.** The machine suggests; the human asserts. **This stays
inside "declare, never infer", and it closes the one blind spot §4g's measurement honestly admitted:**
the solo developer moving files in an IDE without thinking now gets a prompt at commit time.

### 4h.6 Q10 answered, and it splits into two cases with different homes

**Verified at source, and the answer is sharper than "add a witness".**

`facts.rs:105` shows `RenamePath` **already inserts its destination into `newly_occupied`** — the set
`SamePathCreate` fires on (`classify.rs:96`).

- **Rename onto an occupied path: already covered.** Two operations whose `newly_occupied` intersect
  raise `SamePathCreate` today. **The machinery is wired for renames already; only the label
  misdescribes it**, since nothing was created.
- **Both sides rename one node to different paths: nothing fires.** The destinations are disjoint, so
  `SamePathCreate` does not trigger. **One node, two paths, no witness.**

**And the taxonomy shows why the thirteenth is principled rather than additive.** It already carries
per-attribute mismatch witnesses — `KindMismatch`, `ModeMismatch`, `BlobMismatch` — for disagreements
about one node's attributes. **`path` is the one mutable attribute with no mismatch witness, precisely
because the operation that mutates it was never authored.** The thirteenth completes a family rather
than extending one.

### 4h.7 Q11 is a prerequisite, and the fix dissolves both hazards

**Materialization has the merge-ordering hazard's analogue**: a rename cycle (`a→b`, `b→a`) applied
sequentially by path clobbers; a rename onto an occupied path silently overwrites.

**Applied by node, both vanish.** Resolve the final `node → path` projection and materialize that: a
swap is two nodes exchanging a path attribute, with no sequence to get wrong, and a collision becomes a
structural event the `path_to_id` index detects.

**So the fix is not "apply renames in the right order" — it is "teach `patch_replay` what
`lifecycle_cache` already knows: resolve node before path."** And it **gates** rename authoring rather
than following it: authoring first would seal history `checkout` cannot materialize.

## 4i. CLOSED 2026-09-09 — external review concluded, and one invariant survives it

**Third and final external round. Q7, Q8, Q9, Q10, Q12 and Q13 are all settled**, and the reviewer
declined to manufacture further doubt — recorded because a review that ends by saying *"I think this
is solved"* is worth more than one that finds something to keep itself busy.

### 4i.1 RULED — the honesty invariant, and it binds every future surface

**The one obligation that outlives this RFC:**

> **Any surface that presents a `RenamePath` must keep the asserting signer recoverable *in the same
> answer*. The shared object carries the evidence (the signature); the belief is local; the two must
> never be separated at the point of display.**

**Why the object is honest without it, and what it guards.** A signed `RenamePath(a→b)` asserts *"signer
S asserts a→b"* — which is **true**, whether or not anyone believes S. **Belief was never the object's
to carry**, so the patch does not lie; it would lie only if it claimed *"a→b is so"*, and it never did.

**The one way this system could produce a lying object is a read surface that renders a rename as a
bare fact while suppressing who asserted it** — laundering a claim into a truth. **The invariant
forbids exactly that and nothing else.** It passes the naive-reader test *because the signer is in the
object*: even a reader computing no trust annotation still renders *"S asserts a→b"*.

**RULED — make it a property, not a rule.** A written obligation decays; this project's own precedent
is to make correctness structural (RFC 130's coupling gate, RFC 127's changelog gate, RFC 108 §3c's
shared classifier that made parity *a property rather than a decision*). **So the rename and its
asserting signer should be one value in the read types, not two fields a surface may render
separately** — if a rename-bearing report cannot be *constructed* without its signer, no future surface
can drop it, and the invariant is enforced by the compiler rather than by a reviewer's memory.

### 4i.2 Q10 refined — only one of the two cases needs the thirteenth

**The external verdict ratifies the thirteenth and narrows the work**, applying this RFC's own
resolution test rather than taxonomy:

- **Both sides rename one node to two disjoint destinations — needs the thirteenth.** Nothing fires
  today, so there is no resolution path at all. Its resolution is *"choose which destination wins **for
  this node**."*
- **Rename onto an occupied path — `SamePathCreate` passes and should not change.** Its resolution,
  *"two claimants for one path, choose one"*, is already correct. **Only the label is imprecise**, since
  nothing was created. **Relabelling is optional cosmetics and may be deferred** — it is an
  external-interface change with no resolution benefit.

**And the pair is a genuine dual**, which is the strongest evidence the thirteenth is not
`SamePathCreate` in a rename hat: **both-sides is "one node, two paths — pick the path"; occupied-path
is "two nodes, one path — pick the node."** Different resolution shapes, so different witnesses.

### 4i.3 Q12 and Q13 answered

**Q12 — the read-time annotation satisfies the honesty rule**, and more cleanly than demotion would
have, because it never had to mutate anything to stay honest.

**Q13 — the separable-grant concern dissolves rather than hides.** The mechanism concern is gone with
the mutation. The underlying question — *is trusting S's content the same as trusting S's identity
assertions?* — relocates to the reader's own local policy, **and cannot bite shared state, because
replay is policy-independent.** A concern load-bearing only on local display and never on shared
history is dissolved. **Better than the reviewer's own mind-changer imagined:** a separable grant is
never *needed*, yet the distinction is always *available* to any reader who wants it.

## 4j. CORRECTED 2026-09-09 — the handoff's "reference implementation" was false, and the gap reaches sealing

**Increment 1's handoff said: *"teach `patch_replay` what `lifecycle_cache` already knows"* and *"there
is a reference implementation in this repository."* **That was wrong for the one case the increment
exists to handle.**

**Verified at source.** `rename_node` (`prikk-replay/src/node_lifecycle/mutation.rs:93-101`) rejects a
target path occupied by another live node. Applied **sequentially** — which is what
`apply_state_effect` does, one operation at a time — **a two-node swap fails**: node A moving to
`b.txt` while node B is still live there raises
`Integrity("rename target path b.txt is occupied by another live node")`.

**And the gap is not confined to a read path.** `apply_candidate_patches` is, in its own words, *"the
same `apply_state_effect` fold every other replay path uses"*, and
`derive_next_state_root_for_candidate` is the **seal-time** derivation
(`seal_from_accepted.rs:208`; also `rfc111_seal_simulation.rs:82`, `merge_execute.rs:166`).

> **A rename cycle cannot be sealed into history today, and increment 1 does not change that.** It
> teaches the *read* path to materialize one; the *write* path still cannot produce a block whose
> state root represents it.

**Found by the implementing round empirically — their first fixture called `derive_next_state_root` the
way existing fixtures do, and it failed with the identical error from a different call site.**

### 4j.1 RULED — the seal path gets the same resolution, in its own increment, before authoring

**Increment 3 must not land first.** Authoring `RenamePath` while the seal path cannot represent a
cycle would produce exactly the failure increment 1 was built to prevent, one layer up: history that
cannot be sealed rather than history that cannot be read.

**Constraint on that increment, and it is the part that needs care:** for every patch set that derives
a state root successfully today, **it must still derive the same root.** Turning an error into a value
is safe — nothing depends on a derivation that currently fails. **Turning a success into an error is
not**, and a naive port of increment 1's validation would do exactly that. See §4j.2.

### 4j.2 RULED — a chained rename within one patch is malformed, and both paths must say so

**Increment 1 removes one divergence and introduces another, in the opposite direction.** Its Phase 0
validates every operation's `old_path` against the **pre-run** state, so a *chained* rename — `A→B`
then `B→C` for one node in one patch — is rejected. **The sequential seal path accepts it**, because
each operation sees the previous one's result.

**So today a chained rename would seal successfully and then fail to check out.**

**RULED: the chained rename is the malformed one, and `patch_replay` is right to reject it.** A patch
describes one set of changes; renaming a node twice within it means the intermediate path existed in no
sealed state. **Its only honest meaning is the net move `A→C`.**

**Therefore the follow-on increment aligns `derive_next_state_root` to `patch_replay`'s validation, not
the reverse.** That is a success→error flip, and it is admissible **only because it is unreachable in
authored history** — nothing emits `RenamePath` — and because the case represents a state no sealed
block ever had. **Increment 3 must never author one.**

### 4j.3 The batch scope, accepted as ruled

Resolution is over **maximal runs of consecutive `RenamePath` operations within one patch** — not
across patch boundaries, not across interleaved non-rename operations. **The reasoning is sound and
recorded:** a patch is the atomic signed unit, so a genuine simultaneous swap can only be authored
within one; deferring all of a patch's renames regardless of interleaving would reorder execution
against `op_seq`.

**Accepted limitation:** a *non-adjacent* interleaved cycle (`rename A`, unrelated edit, `rename B`)
is not resolved. **No control requires it and no authoring path can produce it**, but it is a real
edge and is recorded rather than discovered later.

## 4k. DELIVERED 2026-09-09 — increment 2 converged both paths, and the fold topology was two sites not three

Increment 2 is **accepted** at `e72e6fc`. `NodeLifecycleState::rename_nodes_checked_batch` now carries the
node-before-path resolution at the node-primary state, and both fold sites route consecutive `RenamePath`
runs through `effect::collect_rename_run` / `apply_rename_run` instead of applying them one at a time.
`apply_state_effect`'s `RenamePath` arm refuses, as a routing-bug defence rather than a statement that
renames are unsupported. The seal path and the read path now agree on all five shapes — plain rename,
two-node swap, chained rename, genuine collision, rename-free — with zero disagreements.

**The handoff's §3 was wrong about the topology, and the correction is recorded here rather than left in a
report.** It named *three* fold sites, listing `replay_chain_with_appended_patches (~:467)` as one. There
are **two**: `apply_patch_ids` and `apply_queued_patch_envelopes`. Line 467 is inside the latter's own
missing-blob retry branch, guarded by a let-else to `EditText`, so it is the same fold reached twice, not a
second rule. `replay_chain_with_appended_patches` contains no operation loop at all — it calls
`apply_patch_ids` twice and inherits the resolution. Verified at source during review.

**A third *entry point* exists and needs no routing of its own.** The incremental lifecycle cache
(`try_incremental_step` → `apply_one_block`) derives state without passing through either fold site
directly; both `apply_one_block` and `apply_one_block_with_text_cache` funnel into `apply_patch_ids`, so
they inherit the run resolution transitively. Recorded because "how many places implement this rule" is the
question §3 existed to answer, and the answer is now one implementation, two fold sites, three entry points.

**No sealed state root moved.** The constraint on this increment was that every patch set deriving a root
today must still derive the same root. Confirmed independently at review by transplanting the round's own
`root_stability.rs` onto the *parent* commit and running it there: all three roots pass pre-change, so the
recorded values genuinely predate the work rather than having been captured from its output.

### 4k.1 REQUIRED before increment 3 — the queued-envelope fold site is asserted, not demonstrated

Removing the rename routing from `apply_queued_patch_envelopes` entirely, leaving `apply_patch_ids` intact,
**passes the full 1035-test suite with zero failures**. The routing is present and correct; nothing proves
it stays that way. Because `apply_state_effect` refuses a lone `RenamePath` loudly, any test driving a
rename through the queued path would have failed instantly — none does.

That site is the unsealed-WAL path, production-reachable from `patch_replay.rs:566`, and it is where
multi-commit queuing lives. It is harmless today only because no authoring surface emits `RenamePath`.
**Increment 3 is the increment that ends that**, so the coverage must exist before it lands: a control that
drives a two-node swap through `apply_queued_patch_envelopes` and fails if the routing is removed.

### 4k.2 REQUIRED — increment 1's doc comment now denies the convergence increment 2 established

`patch_replay/apply.rs:271-279` states in the present tense that `apply_rename_batch` *diverges* from
`lifecycle_cache`, which "applies renames one operation at a time" and "does not survive a literal swap."
That was true when increment 1 shipped and is false now. `rename_node_checked`, which the paragraph names,
has **no production caller left** — only `prikk-replay`'s own unit tests. `patch_replay` was correctly
untouched by increment 2, so this is a consequence of the change rather than a fault within it, but a
convergence asserted in code and denied in the comment beside it will not survive the next contributor.

### 4k.3 RECORDED — `rename_nodes_checked_batch` is not fail-atomic, and neither is its twin

Phase 1 clears every source from `path_to_id` before Phase 2 can still fail on occupancy, so an `Err` leaves
the receiver half-mutated. **Not a regression**: increment 1's `apply_rename_batch` has the identical shape,
and increment 2 was asked to mirror it. Not a live defect either — the single production caller sits under a
replay discarded on error, and `try_incremental_step` clones the cached state before stepping.

The asymmetry is exposure: increment 1's version is `pub(super)`, this one is `pub` on a published
`prikk-replay` type. A method named `_checked` that half-applies on failure will mislead a future caller.
Ruled: **make it atomic** — check occupancy against "occupied by a node outside this batch" using the
batch's own source set, without vacating first — rather than documenting the hazard and keeping it.

## 4l. CLOSED 2026-09-09 — the three follow-ups are delivered, and §4j.2's mechanism was misdescribed twice

All three items ruled in §4k.1–§4k.3 are delivered at `bb2e932` and reviewed. **Increment 3 is unblocked.**

**§4k.1 — the queued fold site is now held.** A control enters through `apply_queued_patch_envelopes`
itself, building `WalRecord`s in the shape `patch_replay.rs:566` supplies, and resolves a two-node swap.
Re-verified at review by repeating the deletion experiment that found the gap: deleting the routing block
now fails exactly one test, where against `e72e6fc` it failed none. Increment 2's routing is byte-identical
to `e72e6fc` — nothing was disturbed to add the coverage.

**§4k.2 — increment 1's doc comment is corrected**, and `patch_replay/apply.rs` is doc-comment-only in the
diff, so the cleared algorithm is untouched. The sweep beyond the two named locations found no further
staleness and said so explicitly.

**§4k.3 — `rename_nodes_checked_batch` is fail-atomic.** Phases 0–2 are read-only, Phase 3 cannot fail, and
occupancy is checked against the batch's own source set rather than by vacating first. A control asserts
whole-state equality against a clone after a collision; review perturbed Phase 1 back to mutate-first and
confirmed the control fails on exactly that property. One recorded consequence: occupancy is now checked
before index consistency, so a state triggering both reports the other error. Both are `Integrity`, both
reject rather than accept, and both require an already-corrupt index — accept/reject convergence with
`patch_replay` is untouched.

### 4l.1 CORRECTED — the chained rename is rejected by the `old_path` check, not the dedup

Increment 2's report and the increment 2 review both stated that a chained rename is rejected by
`sources_seen.insert(node_id)` in Phase 0. **It is not.** The `old_path`-against-pre-batch-state check sits
*before* the dedup in the same loop: for a real chain (`A: a→b`, then `A: b→c`), the second pair asserts
`old_path = "b"` while node A's pre-batch live path is still `"a"`, so the mismatch fires and the dedup is
never reached. The dedup catches a narrower shape — the same node named twice with the same true `old_path`
and two destinations (`A: a→b`, `A: a→c`) — where both pairs pass the `old_path` check.

Found by the implementing team while writing the test for it: their first version asserted the stated
mechanism and failed. The finding is theirs, not the reviewer's, and it corrects the reviewer.

**The ruling does not move.** The outcome was and is correct — a chained rename is rejected, both paths
agree, and `rename_convergence`'s `chained-rename` shape remains `false/false`. **§4j.2's own text was never
wrong**: it describes the rule as validating `old_path` against the pre-run state, which is exactly the
check that fires. The imprecision lived in two reports and one review, not in anything ruled — recorded
here so the RFC carries the mechanism as well as the rule, since the two tests that now pin it are split
along precisely this line.

## 4m. DESIGNED 2026-09-09 — the bundle-impact preview, and the gap it actually fills

§4f ruled the *shape* — own exactly one comparison, anchored at HEAD, and do not call it `diff`. §4g.3
ruled it separable. Neither designed it. This section does, and **the design changed once the existing
surface was read rather than assumed.**

### 4m.1 The gap, stated precisely

The exchange path today is four steps, and the safety question falls in a hole between the first two:

| Step | What it does | Writes? |
|---|---|---|
| `bundle verify --input <f>` | Checks a bundle offline. **Needs no repository** | Nothing |
| `bundle import --input <f>` | Records it as an untrusted received pointer | **Writes objects into the store** |
| `trust maintainer add` | The trust decision | Trust state |
| `merge --into REF --from REF` | Seals the merge | **The irreversible act** |

**`merge-plan` already exists and is already read-only** — but it takes block ids or refs, so it can only
answer *after* the bundle's objects are in the store. **So today, to learn what a bundle would do to you,
you must first let its objects into your object store.**

That is the gap. `bundle verify` answers *is this bundle internally sound*; nothing answers *what would
this do to **my** repository* without first admitting it. **The preview is the missing middle step**: it
needs the repository (there is nothing to compare against otherwise) and it **writes nothing**.

**This is a narrower and better-founded claim than §4f's own framing.** §4f called the preview a surface
consulted "before an irreversible act", implying `merge`. The act it must precede is earlier: **admitting
foreign objects into the store.**

### 4m.2 RULED — the command

**`prikk bundle preview --input <file>`.**

A `bundle` subcommand, not a top-level command: it sits beside `export` / `import` / `verify`, takes the
same `--input <file>` those already take, and reads in the same vocabulary. §4f's instruction not to call
it `diff` is satisfied without inventing a noun — and `rollback-preview` already establishes `preview` as
this project's word for a non-mutating look at what an operation would do.

**Hard requirement: it writes nothing.** No objects, no refs, no received pointer, no trust state, no
cache. A safety surface that mutates the thing it is protecting is not one. This must be asserted by a
control, not by inspection — the round must prove the object store and every ref are byte-identical
before and after.

**It requires a repository** (unlike `bundle verify`), because the whole question is *against my HEAD*.

### 4m.3 RULED — what it computes, and the reader it needs

**One replay of current state, plus the bundle's own patches applied in memory** (§4g.3). The
implementation crux: the bundle is self-contained, so its objects must be readable **without being
written**. That means a composed, read-only reader — the repository's object store **overlaid with the
bundle's own object set held in memory** — and replay run against that.

**Reported effects, at node granularity**, being exactly what the model can honestly say today:
paths created, deleted, edited, renamed, and permission-changed, with counts and the affected paths.

**Three answers that are not effects, and are the point of the surface:**

1. **Does it apply at all?** A bundle whose history does not connect to this repository's is a
   legitimate, common answer — **report it, do not error.**
2. **Would it conflict?** Conflicts are a *result* of a successful preview, not a failure of it.
3. **Who sealed it?** Report the sealing identity, and **state in the same breath that this is not a
   trust decision** — `import` already says exactly that about recorded author key material, and a
   preview that implies trust it has not established would be worse than one that says nothing.

### 4m.4 RULED — exit codes, output, and the honesty limit

**Exit codes follow RFC 121 unchanged: `0` the preview was produced · `1` operational failure · `2` usage
error.** **A bundle that would conflict, or that does not connect, exits `0`** — the command was asked
what would happen and it answered. Conflict is not the command failing.

**Therefore the answer must be machine-branchable in the output**, per RFC 140 §7b / RFC 142 §6b's
lineage: a caller must be able to distinguish *applies cleanly*, *applies with conflicts*, and *does not
connect* from a field, never by parsing prose or inferring from an exit code. `--format json` is
required, not optional, for the same reason.

**Degradation follows RFC 142 §6b: degrade absence, propagate error.** A blob the preview cannot read is
reported as unavailable in a named field; a malformed bundle is an error.

**The honesty limit is stated in the output, not only in the docs.** §4g.3: *"A bundle containing
delete+create for a move previews delete+create."* Until rename authoring lands (§4k, order item 4), the
preview cannot show a move as a move, and **it must say so** rather than let a reader infer that a
delete-plus-create was what the author meant.

### 4m.5 Scope — one increment, and what it must not become

**In:** the subcommand, the composed read-only reader, the node-granularity effect report, the three
answers of §4m.3, `--format json`, and the write-nothing control.

**Out, explicitly:** content-level diffing of any kind — that is RFC 143's job composed by the caller,
and §4f declined it deliberately; any trust evaluation; any change to `import`, `merge`, or `merge-plan`;
and any prompt-to-import convenience. **The preview tells you; it does not offer to act.**

## 4n. DELIVERED 2026-09-09 — `prikk bundle preview` is built, and one state it cannot answer

**Accepted at `b814716a`.** `prikk bundle preview --input <file> [--ref REF] [--format json]` answers
§4m.3's three questions — connectivity, conflict, and sealer — against a bundle whose objects are never
admitted to the store.

**The write-nothing guarantee is proven at the subject level.** The round's own perturbation demonstrated
that its digest is sensitive to a write; review additionally made `preview_bundle` *itself* write a cache
file and confirmed `bundle_preview_writes_nothing` fails on it. The digest is a full recursive walk over
path **and** bytes, so cache writes and stray refs are in scope, not just objects.

**The reader already existed.** §4m.3 specified "the object store overlaid with the bundle's own object
set held in memory"; `BundleAndLocalReader` was already exactly that, used by `verify_bundle` with
`local: None`. Reused rather than rebuilt.

**Two limits, both disclaimed in the output rather than only in the docs:** conflict detection is a naive
apply and cannot see two individually-successful changes that fail to compose under real 3-way
resolution; and an ambiguous lowest common ancestor degrades to `Undetermined` rather than guessing. A
`rename_note` field carries §4g.3's delete+create honesty limit, and there is deliberately **no `Renamed`
effect variant** — a variant that can never be produced would be worse than its absence.

### 4n.1 REQUIRED before any release ships this command — the unsealed repository

**`preview_bundle` returns `Err("ref heads/main is not published")` on a freshly-initialized repository**,
verified empirically. `bundle.rs:445-447` has no branch for an absent local ref, and no fixture covers the
case — every test starts from a repo that has already sealed.

**That state is reachable and it is the important one.** The tutorial's sequence is `prikk init` →
`commit` → `seal`; nothing is published until the first seal. **So the one moment a user most needs to ask
what a bundle would do — they have just initialized, someone handed them a bundle, and they have nothing
of their own yet — is the one moment the command returns exit 1 instead of an answer.**

This is a gap against §4m.3 rule 1 (*does not connect is a legitimate answer; report it, do not error*),
of which an empty repository is the limiting case, rather than a defect in what was built.

**RULED: add a fifth `BundlePreviewConnectivity` state; do not overload `FastForward`.** With no local
history the bundle's whole history is new, which makes `FastForward` tempting — but that variant asserts
*your history is a prefix of the bundle's*, and a repository with no history has no prefix. In a field
whose entire purpose is to be machine-branchable, that would be a false value. A distinct state costs one
variant and says the true thing.

### 4n.2 RECORDED — the cycle fix cost 39 `pub(crate)` widenings

Building the module as `patch_replay::preview` created `patch_replay -> merge_evidence` while
`merge_evidence` already depended on `patch_replay` — a real cycle, caught by RFC 130's own SCC gate and
confirmed with `boundary-check`. **Relocating to `bundle/preview.rs` was the right fix** (`bundle` already
depended on `patch_replay` one-way), and it was chosen over widening the gate or suppressing the check.

It required widening **39** items to `pub(crate)` — `mod apply;`, `mod read;`, `apply_operation_sequence`,
`single_parent_chain`, `read_block`, `read_patch`, `load_snapshot_files`, `ReplayLiveNode` and its fields
among them.

**RFC 131 is already delivered** (`544cc6c`, `4acd7e8`, `971e664`): 125 → 100 top-level entries, 69 → 52
modules, and the **first 27 `pub(in crate::…)` narrowings**. So this is not a tension with future work —
**this round widened 39 items while the completed round narrowed 27.** They are different items and do
not cancel, but the direction is opposite and the magnitude larger, and **nothing measures the net**: the
coupling gate binds cycles, not visibility, so no gate would have said this happened.

Recorded because RFC 131 §3's remaining target is already foreclosed by its own §6a pending the
coupling-gate item, and a visibility budget that only ever moves outward between narrowing rounds is the
thing that would make that target unreachable for good.

### 4n.3 CLOSED 2026-09-09 — the unsealed repository answers, and the release blocker is lifted

Delivered at `a6d70665`. `preview_bundle` branches on `Option<ObjectId>` instead of erroring: an
unpublished local ref routes to `preview::preview_new_repository_impact`, which replays the bundle's own
chain against an empty starting state and reports every path as `Created` with `AppliesCleanly` — "all of
it arrives." Exit `0`, JSON `connectivity: "no-local-history"`, distinct from all four existing labels
(`bundle.rs:297-301`).

**§4n.1's ruling was followed, not reinterpreted**: `FastForward` was not overloaded, and `preview_impact`
is untouched — the branch is taken at the call site, before any `ObjectId`-shaped value is required.

**Verified at review by perturbing the new branch alone** to write a cache file: only
`no_local_history_reports_the_whole_bundle_as_created` failed. The guarantee holds on the new path, which
is where a guarantee most often silently stops applying.

**The neighbour question is settled, at the type rather than the call site.**
`RefStore::read_current_ref_state_id` (`refs.rs:336-348`) returns `Ok(None)` whenever no pointer entry
exists. A ref that never existed and a ref not yet published are **not two states this store can
distinguish** — they are one, and both now route to `NoLocalHistory`.

**Recorded for whoever adds a sixth branch:** the round's report claims the pre-existing
`bundle_preview_writes_nothing` test "covers every path through `preview_bundle`." **It does not** — it
builds a sealed fixture and takes the `Some(...)` branch; under the perturbation above it passed while
the new test failed. The new path is covered only because the new test carries its own before/after
digest. **A claim that an existing control covers a new path must be established by a perturbation that
travels that path**, or a future branch will ship with the guarantee quietly not applying to it and every
test still green.

**`prikk bundle preview` is complete.** Nothing in §4m or §4n remains open.

## 4o. DESIGNED 2026-09-09 — increment 3, in four pieces, of which one is handed off

§3 says "`prikk mv`, **or equivalent capture**" and §4h.5 mentions `prikk mv --record` only to set it
aside. **The policy is ruled thoroughly and the surface is not designed at all** — the same state
`bundle preview` was in at §4f before §4m. This section designs it.

**Increment 3 as scoped bundles four separable pieces**, and only the first is on the critical path.
All four are designed here; **piece 1 is handed off alone**, because it is the round that makes several
latent things live at once and should be small enough to review closely.

### 4o.1 RULED — `prikk mv <old> <new>`, and it both moves and declares

**The command performs the rename on disk *and* records the declaration.** Not one or the other.

A declaration that does not move the file leaves the worktree and the declaration disagreeing, and the
user finding out at `commit`. A move that does not declare is the shell `mv` we already have. **`git mv`
does both and the shape is right for the same reason.**

**But the realistic flow is against us, and §7's own note says so:** *"a user runs `mv` in a shell and
**then** commits, by which point the declaration was never made."* So `prikk mv` must accept the
already-moved case:

| Worktree state | Behaviour |
|---|---|
| `<old>` exists, `<new>` does not | Perform the rename, record the declaration |
| `<old>` gone, `<new>` exists | **Record the declaration only**, touch no bytes — the user already moved it in a shell |
| Both exist | **Refuse.** Two files, and which one is the node is not ours to guess |
| Neither exists | **Refuse.** Nothing to move |

**The second row is the one that makes this usable at all**, and it is why the command is not merely a
wrapper around `rename(2)`. **It must not compare content to decide** — that would be inference, and
§4g's declaration-only ruling forbids the machine deciding what moved.

### 4o.2 RULED — the declaration is durable, cleared on consumption, and collapses to the net move

`prikk mv` and `prikk commit` are separate invocations, so the declaration must survive between them.
It lives in the repository's own state alongside the queue, is written durably, and is **cleared when
the commit that consumes it is queued** — not when it is sealed. A declaration is an intent about the
*next* patch, and a queued patch has already captured it.

**Chained declarations collapse to the net move, and §4j.2 already ruled why.** `prikk mv a b` then
`prikk mv b c` before a single commit must author **`a→c`**, one operation, not two.

> §4j.2, verbatim: *"renaming a node twice within it means the intermediate path existed in no sealed
> state. **Its only honest meaning is the net move `A→C`.**"*

That is not a new decision; it is the same rule the seal path and `patch_replay` already enforce,
applied one layer earlier so a malformed patch is never authored in the first place. **Authoring must
not emit what replay would reject.**

**Two corollaries fall out of the same rule and must be handled, not discovered:**

- **`prikk mv a b` then `prikk mv b a`** nets to no move. **Drop the declaration entirely** — do not
  author `a→a`, which asserts a move that did not happen.
- **A declared move whose destination is then deleted** nets to a deletion. **Drop the declaration and
  author the plain `DeleteNode`** — the intermediate path existed in no sealed state, exactly as above.

**If the worktree contradicts a live declaration at commit time — the declared destination is gone and
the source is back — refuse the commit with a message naming the declaration.** Do not silently drop it:
the user asserted something, and the machine discarding an assertion quietly is the failure mode §4g's
whole resolution exists to prevent.

### 4o.3 RULED — `worktree-status` shows live declarations, because an invisible assertion is a trap

A declaration made and forgotten is authored into permanent history at the next commit. **It must be
visible before then**, in the command whose job is *what will this commit do*, and in its `--format
json` as a machine-branchable field.

### 4o.4 Piece 2 — the commit-time hint (§4h.5), non-authoring

When `commit` sees a delete+create pair that is a strong move candidate, it **may** print
*"looks like you moved X to Y; `prikk mv` would preserve its identity"* — and **authors nothing**.

**The heuristic is allowed to be wrong** because it costs a line of output, not a fact in history. Its
one hard constraint: **it must never author, never prompt, and never block.** The machine suggests; the
human asserts. A hint that becomes a default is the coercion §4h.5 refused.

**Scope note:** exact-content match is the obvious candidate signal and is sufficient. Similarity
scoring is explicitly out — it buys a better hint at the cost of a threshold nobody can defend.

### 4o.5 Piece 3 — the thirteenth conflict witness (§4i.2)

**One node, two disjoint destinations, both sides.** Resolution: *choose which destination wins for this
node*. Distinct from `SamePathCreate`, which stays as it is — §4i.2 established the pair is a genuine
dual: *"both-sides is one node, two paths — pick the path; occupied-path is two nodes, one path — pick
the node."*

**Relabelling `SamePathCreate` remains optional cosmetics and stays deferred**, per §4i.2.

### 4o.6 Piece 4 — §4i.1's honesty invariant, enforced structurally

**Rename and asserting signer become one value in the read types**, so a rename-bearing report cannot be
*constructed* without its signer and no future surface can drop it.

**This is the piece with reach beyond rename**, since it changes read types other surfaces already use —
`show`, `bundle preview`, `checkout --patch-plan --format json` all render operations. It is designed
here and **must not ride with piece 1**: bundling a type change across four render surfaces into the
round that first authors renames would make both harder to review.

### 4o.7 Sequencing, and what piece 1 makes live

**Piece 1 first and alone.** Pieces 2-4 follow in any order; piece 4 wants its own round.

**Piece 1 is the first thing in prikk that ever emits a `RenamePath`**, and that flips several latent
things to live in one commit:

- **`checkout --patch-plan --format json` fails outright on a `RenamePath`** — documented, and we told
  stikk so in `send/004`. Their Compare view will start meeting it.
- **The queued-envelope fold site's rename routing becomes load-bearing** rather than precautionary —
  which is exactly why §4k.1 gated increment 3 on proving it.
- **`rename_nodes_checked_batch`'s fail-atomicity starts mattering** on real input.

None of these is a reason to delay. They are the reason to keep piece 1 small.

## 4p. DELIVERED 2026-09-09 — prikk authors renames, and §4o.2's refusal rule was narrower than its own reason

**Piece 1 accepted at `9a1b7dc2`. This is the first sealed prikk history that can contain a
`RenamePath`** — increments 1 and 2 built the replay and seal paths for an operation nothing could yet
produce; this round produces it. `prikk mv <old> <new>`, the four worktree states of §4o.1, the durable
declaration store of §4o.2 with net-move collapse, and `worktree-status` disclosure per §4o.3.

**Six controls, end to end through the compiled binary**, including the two-node swap that increments 1
and 2 existed for and that nothing could reach until now. The round's perturbation — removing the
round-trip drop guard — was re-run at review and fails exactly one control, the right one.

### 4p.1 The finding this round produced: a path-keyed cache is invalid for a rename destination

Comparing a rename's destination against baseline through `resolve_existing_file` consults
`commit_index`, **a cache keyed by path and valid on the premise "this path's content is unchanged since
the entry was recorded."** A rename breaks that premise in a way no other caller does: `new_path` just
changed **which node occupies it**, so a prior entry at that key describes the path's *former* occupant.
Observed as a false content mismatch that sent a provably-unchanged rename into `plan_edit_text`.

**Only a swap can reach it** — a lone rename's destination is never a path the index already holds a
stale entry for. Fixed by never consulting the cache for a rename destination, while still refreshing
the entry afterward. **Found by the control that only became runnable in this round.**

### 4p.2 CORRECTED — §4o.2 required refusal for one case; three others discard the assertion just as quietly

§4o.2 required refusing the contradiction case (destination gone, source back), justified as:
*"a machine discarding a human's assertion quietly is the exact failure §4g exists to prevent."*
**The rule was narrower than the reason.** Three further resolutions discard the assertion silently, and
the round implemented them exactly as specified. Verified at review against the built binary:

```
$ prikk mv a.txt build/a.txt        # build/ is ignore-matched
moved a.txt -> build/a.txt
$ prikk commit -m moved
  delete-file a.txt                 # no mention of the declaration at all
```

**The outcome is correct and must not change.** Moving a tracked file into an ignored path removes it
from the tracked set; a deletion is the honest record and refusing it would be wrong. **The silence is
the defect.**

**RULED: disclosure, not refusal.** Every declaration resolving to anything other than the asserted
rename — vacuous round trip, nets-to-deletion, `old_path` never a tracked baseline node,
ignore-matched destination — **must be named on the commit that consumes it, with what it became**, one
line each. **Refusal stays reserved for the contradiction case §4o.2 already names.**

### 4p.3 The three judgment calls accepted as implemented

- **"Last stated intent wins"** when a second `mv` names an `old_path` with a distinct live declaration.
  **Correct**: it matches the declaration model exactly — the user's latest assertion is the one they
  mean, and both a merge and a refusal would be the machine second-guessing an assertion.
- **A declared destination occupied by another untouched tracked node is refused at authoring time**,
  best-effort, with replay authoritative. **Correct layering**: a fail-closed authoring check that
  neither duplicates `rename_nodes_checked_batch` nor pretends to replace it, and is framed as
  best-effort rather than complete.
- **Raising these rather than interpolating them silently is why they could be ruled on.** Two were
  right as built; two (§4p.2) were right in outcome and wrong in disclosure.

### 4p.4 CLOSED 2026-09-09 — disclosure delivered, and it belongs at the point of resolution

Delivered at `cabe2487`, output only — every outcome unchanged, the `continue` sites intact with a
disclosure push added before them.

```
  delete-file a.txt
  declaration a.txt -> build/a.txt: destination is ignored; recorded as a deletion, not a rename
  declaration a.txt -> b.txt: destination is gone; recorded as a deletion, not a rename
  declaration x.txt -> y.txt: source was never a tracked node; there was no node to rename
  declaration a.txt -> b.txt -> a.txt: nets to no move, dropped
```

**Every line says what happened and stops** — no advice, no "did you mean", no warning tone. That was
the easiest thing to drift from and it did not drift; §4o.4's hint stayed out, though "destination is
ignored" sits one sentence away from it.

**CORRECTED — the disclosure belongs at the point of resolution, not uniformly at commit.** §4p.2's
handoff said "on the commit that consumes it." **For the round trip there is nothing to consume**: the
chain-collapse drops it when the second `mv` runs, so no commit ever sees it. That one disclosure lives
on `prikk mv`'s own output, and the round **flagged the deviation rather than silently reinterpreting the
ruling.** My wording was wrong; a disclosure can only live where the resolution happens.

**Deleted and ignored are distinguished by a filesystem check, not by the ignore-filtered map.** A path
absent from that map means either *gone* or *present but excluded* — the map is the wrong witness for
this question by construction, and reusing it would have produced a confidently wrong message.
`symlink_metadata` against the real disk path decides.

**The perturbation's scope proved a property beyond its failure.** Suppressing commit-side emission fails
exactly the three commit-side disclosure tests while the round-trip control keeps passing — demonstrating
the two disclosure sites are **independently held**, so breaking one cannot be masked by the other.

## 4q. DELIVERED 2026-09-09 — the commit-time hint, and "authors nothing" made a compiler guarantee

**Piece 2 accepted at `25c45734`.** A shell `mv` followed by `commit` now names the command that would
have preserved identity — closing §4h.5's *"irreversible, commit-time-invisible opportunity cost"*
without prompting, defaulting or blocking.

```
  declaration a.txt -> build/a.txt: destination is ignored; recorded as a deletion, not a rename
hint: looks like a.txt moved to b.txt; `prikk mv` would have preserved its identity
```

**Separated on three axes, not one:** indentation (the disclosure sits with the operation lines; the hint
does not), prefix (`declaration …` versus `hint:`, matching the existing `note:` convention), and grammar
(past-tense fact versus conditional suggestion). **A reader can tell what prikk recorded from what prikk
wondered**, which was the requirement.

**`plan_move_hints` takes `planned: &[PlannedOp]` — a shared slice.** "Authors nothing" is therefore
enforced by the borrow checker, not by the test suite: the function *cannot* mutate the operation list.
**That is §4i.1's own principle applied elsewhere** — make it a property, not a rule. Review's
perturbation had to be installed at the call site, because the signature makes it unreachable from
inside, and it fails exactly the two controls that inspect authored operations.

**No prompt, no flag, no default, no `--assume-moves`.** §4h.5's prohibition held, and this was the round
that could most easily have drifted into it once a working heuristic existed.

### 4q.1 CORRECTED — the ambiguity rule does not cover empty files, and the handoff assumed it did

The piece-2 handoff said zero-byte content was *"probably covered by the ambiguity rule, but check that
the rule actually covers it rather than assuming."* **It is not.** The ambiguity check fires only on
*more than one* candidate per side, so **two unrelated empty files — one deleted, one created — are 1:1**
and would have produced a confident, false hint inviting the user to assert a move that never happened.

Both empty blob ids are excluded explicitly. Verified at review by removing the exclusion: exactly the
dedicated test fails while the ambiguity control still passes, proving the two rules are independent and
the exclusion load-bearing. The finding is recorded in the code, not only in a report.

**The signal remains exact content equality alone** — `blob_id` equality, sound because the canonical
encoding is a pure function of `(kind, bytes)`. Similarity scoring stays refused.

### 4q.2 A note for whoever reads the declared-move assertion next

Declared moves are kept out of the heuristic by a `debug_assert!` plus a CLI-level control.
**`debug_assert!` compiles out in release**, so the release-build guarantee is the control, not the
assertion. Adequate — the behaviour is covered either way — but the assertion is not a release-time check
and should not be read as one.

## 4r. DELIVERED 2026-09-09 — the thirteenth witness exists, and a gate one layer up hides it

**Piece 3 accepted at `0e04164a`.** Label **`rename-destination-conflict`**, chosen once and now an
external interface. Two renames of one node to disjoint destinations classify, with both destinations
recoverable through the real report path.

**`Action::RenamePath` gained its destination**, which the handoff established up front as the enabling
change: it carried only `node_id`, so the node→destination pairing was unrecoverable and the witness
could not be computed.

**`SamePathCreate` untouched** — extracted into `classify_same_path_create`, byte-identical logic.

### 4r.1 REQUIRED — the witness cannot be produced by `prikk merge-evidence`, and that is shared machinery

`check_confluence` (`commutation.rs:44-58`) calls `ensure_flat_sequence` on both sides and **returns
early** before the pairwise loop where classification lives; `ensure_flat_sequence` reports `Unknown` for
any operation whose `deferred_reason` is `Some`, which for `Action::RenamePath` is **unconditional**.
**So any real call touching a rename reports `Unknown { RenameDeferred }` and never classifies.**

Found by the round while building its own control 4, verified independently at review, and documented in
the patch-algebra reference and the changelog rather than left in a report.

**Correctly out of scope there**: the fix restructures ordering in shared machinery **every** witness kind
flows through, which is the class of change piece 3 was scoped to exclude. **Handed off separately, with
the constraint that it must not become rename-specific** — a special case for `RenameDeferred` that leaves
symlink deferral pre-empting classification closes the reported gap and leaves the real one.

### 4r.2 CORRECTED — reachability, twice, and both corrections were the round's

**The handoff called control 2 a regression guard. It is new reachability.** Before this round a rename
onto an occupied path classified as `Unknown { RenameDeferred }`, not `SamePathCreate`, because
`deferred_reason` fires unconditionally and was checked first. **§4i.2's "`SamePathCreate` passes and
should not change" describes the *resolution*, not *reachability***, and this round is what establishes
reachability at all.

**And the completeness sweep is weaker than its name.**
`every_conflict_witness_kind_reaches_the_report_item` passed immediately, with no report-item work: it
constructs a synthetic witness for every kind and checks the mapping passes it through — **kind-agnostic
by construction**, so it proves the mapping is total and **can never catch a kind real classification does
not produce**. The round read it rather than reporting a pass. **That distinction is worth more than the
feature it was asked about.**

### 4r.3 RULED — the same-destination case stays `SamePathCreate`

Two sides renaming one node to the **identical** destination intersect on `newly_occupied`, so
`SamePathCreate` fires and its resolution *pick one* is exactly as correct as for a create-vs-create
collision. Nothing was created, so the label is imprecise **in the same way §4i.2 already ruled optional
cosmetics for the dual — deferred on the same grounds**: an external-interface change with no resolution
benefit.

### 4r.4 CLOSED 2026-09-09 — the witness is reachable, and the obvious fix was the wrong one

Delivered at `88e61bdd`. **`prikk merge-evidence` now produces `rename-destination-conflict`**, and
`same-path-create` for a rename operand, through `analyze_merge_evidence` itself.

**`ensure_flat_sequence` was conflating two questions** — *does this sequence have a genuine
replay/evidence/prefix-dependency problem* and *can the oracle attempt this operation alone*. Split into
`FlatSequenceCheck { hard, deferred }`: **`hard` still pre-empts everything at the same point in the same
order**; only `deferred` moved past the pairwise loop, consulted as a fallback.

**The naive fix — run the pairwise loop first — is wrong, and the round found out by checking rather than
by shipping.** It breaks
`confluence::sequence_internal_dependency_is_deferred_before_cross_pair_checks`, a case with **no rename
in it at all**: a genuine same-node ordering dependency coexisting with a cross-pair that resolves to a
different `Unknown` reason, which would have been reported instead. **Verified independently at review**
by running the naive version: exactly that test fails, and only that test.

**That is the risk §4r.1's handoff was written around** — *"the risk is not the feature but the twelve
other kinds this shared ordering moves underneath"* — met and avoided.

**Not rename-specific**: nothing in `commutation.rs` branches on which deferred reason it is, and the
symlink case is covered by its own control. A fix special-casing `RenameDeferred` would have closed the
reported gap and left the real one.

**A prediction in the handoff was wrong and the round said so.** It predicted that disabling the deferred
fallback would report `Confluent`; it reports `EvidenceFailure`, because the code then falls through to
the full replay proof and hits the oracle's own refusal to replay a deferred operation. Wrong either way,
caught either way — and **stating that the observed mode differed from the predicted one** is what
distinguishes a perturbation that was run from one that was described.

**Regression evidence is tied to existing commitments, not hand-captured**: each of the six sweep
scenarios asserts the literal value that kind's own pre-existing dedicated test already asserts. 1061 →
1066 tests, none of the pre-existing 1061 modified.

**Increment 3 now has one piece left: §4o.6, the structural honesty invariant.**

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

## 6. What this RFC does NOT do — CORRECTED 2026-09-08

**This section predated §4b-§4g and contradicted them. Restated.**

**It still does not authorize any implementation.** Nothing may be built from this document.

**What acceptance would settle** — the positions §4c-§4f adopt after external review:

- **`prikk diff` as a general command is declined**, not deferred (§4f).
- **The pre-acceptance bundle-impact preview is the one case worth owning** (§4f), and §4g.3 shows it
  is separable from everything else here.
- **Identity-preserving rename authoring follows only from a declaration, never from inference**
  (§4c), and that rule is what makes the operation safe rather than a lineage-forgery primitive.

**What acceptance would NOT settle, and what still needs its own decision:**

- **Whether to build the declared-move capture path at all**, and what it looks like when the user
  has already moved the file outside the tool (§7's open risk).
- **Teaching `patch_replay` to apply `RenamePath`** (§4g.1) — a prerequisite nobody had costed.
- **The recipe** that documents composing a generic comparison from RFC 143's surfaces.

**And it does not treat the requesting consumer as a reason.** stikk asked, narrowed, and said plainly
it was not a blocker. **This is reopened because the architect's own stated reason expired**, which is
the only good reason to reverse a refusal.

## 7. Risks

- **Reversing a twice-stated refusal costs credibility if done casually.** §2 exists so the reversal
  is traceable to a specific superseded premise rather than to persistence.
- **§4 could swallow this RFC.** If the answer is "author renames", that is a patch-model change with
  its own schema, authoring, replay and conflict consequences. **This RFC must be allowed to conclude
  "the real work is elsewhere" rather than forcing a `diff` shape onto a rename problem.**
- **RESOLVED 2026-09-08 — the ergonomics risk below was overstated, and the reason is structural.**
  **Declaration-only cannot punish anyone, because the undeclared path *is* today's behaviour.** A
  user who declares a move gets identity preserved; a user who does not gets delete+create — **exactly
  what they get now.** There is no regression to fall back to, only an unclaimed benefit. The external
  architect's own stated escape hatch — *"require declaration really does collapse into punishing the
  user"* — **does not apply to prikk**, because the status quo is the fallback rather than a penalty.
  **This removes the load-bearing assumption they said would make them abandon their advice.**

  **Measured, rather than reasoned:** in this repository's own 1519 commits, 99 (6.5%) contain a
  rename, 345 file-renames in total. **The population is bimodal and neither half is accidental** —
  83% of rename-commits move exactly one file (RFC lifecycle moves, deliberate and one at a time), and
  71% of all renames sit in commits of five or more (planned restructures; the largest are 57, 30, 24
  and 21 files). **Both shapes are deliberate acts that a declaration path would naturally capture.**

  **Honest limit on that evidence:** this is one repository, worked by an architect and a dev team
  following a documented process. **It is not evidence about a solo developer refactoring in an IDE**,
  who moves files without thinking. It bounds the question for this project's own work and no further.

- **SUPERSEDED — the original statement of the risk, kept because the reasoning above answers it:**
  **The load-bearing assumption is untested, and it is not ours.** §4c's "declare, never infer" is
  honest *only if declaring is ergonomic* — the external architect said plainly they would abandon
  their own advice if it cannot be, and cited Mercurial as evidence that people do record moves when
  the tool makes it cheap. **We have no evidence either way for prikk.** And the realistic flow is
  against us: a user runs `mv` in a shell and *then* commits, by which point the declaration was
  never made. **The options at that moment are all unattractive** — record delete+create (today, and
  honest), ask interactively (breaks scripted and CI use), require a post-hoc
  `prikk mv --record` (awkward, and easy to forget), or infer-and-mark-derived (the thing the rule
  refuses). **This is the risk most likely to invalidate the whole approach, and nothing in this RFC
  resolves it.**
- **A comparison surface is a permanent contract** in a way `show` is not: `show` reports what a patch
  says, `diff` asserts what two states mean relative to each other.
