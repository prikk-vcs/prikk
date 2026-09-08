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
