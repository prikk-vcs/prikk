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
