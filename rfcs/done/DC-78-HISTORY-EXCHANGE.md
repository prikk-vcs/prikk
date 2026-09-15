# RFC (accepted) - DC-78 History Exchange

**Status.** **ACCEPTED by the project owner 2026-08-09**, same day as proposal.

**Cleared for §4's investigation only.** Design may not begin until §4 is answered and reported —
including §3.2's dependency question, which may itself change when this increment can proceed.
§2's recommendation (scope to exchange, defer transport) is accepted; **§3.1 is a position to test, not
a ruling**, and the RFC says so.
**Authored by** the architect. **Independence.** Author-reviewed — the standing ceiling.
**Arises from.** `MILESTONES.md`'s status-claim criteria, **criterion 1**: a distributed VCS that cannot
distribute. Priority ruled 2026-08-09 — this is the next design work after DC-76, may run alongside macOS
mutation, and **precedes Windows mutation**.

## 1. What is missing

**Nothing in the tree exchanges history between repositories.** Branches merge within one repository;
two machines cannot share anything. This is criterion 1 of six, and the only one with no increment behind
it.

Cross-platform mutation lets more people use prikk *alone*. **Only exchange lets two people work
together**, which is the premise the two-role author/maintainer model exists to serve.

## 2. The framing this RFC proposes — separate exchange from transport

The instinct is to call this "sync" and reach for a network protocol. **That bundles two problems with
very different shapes, and only one of them is hard for prikk.**

- **Exchange** — *what* moves between repositories, and what the receiver verifies before accepting it.
  Every genuinely difficult question lives here, and they are all identity and trust questions.
- **Transport** — *how* the bytes travel. A solved problem generally, and an expensive one here
  specifically: `prikk-store` may depend on exactly `getrandom` and `rustix`
  (`tools/release-policy/src/boundary/placement.rs`), so **any network transport is an
  `ALLOWED_THIRD_PARTY` decision** covering TLS, HTTP, or SSH.

**Exchange is achievable with no transport at all.** A verifiable subset written to a file and read back
elsewhere — any file-moving mechanism carries it, and **no new dependency is needed.** That also delivers
the system proposal's RΔ2 evidence bundle, which was reaching for the same artifact from the auditor
side.

**Recommendation, for the owner to accept or reject: scope this increment to exchange only.** Transport
becomes a later, separable decision made once the trust model is settled rather than alongside it.

## 3. The questions that actually decide this — none answered here

1. **What is the unit of exchange?** Objects are content-addressed, so a bundle could carry objects,
   blocks, or patches. Patch theory suggests patches; the sealing model suggests blocks; content
   addressing makes objects the simplest. **This is not a packaging choice** — it determines what a
   receiver can verify without already holding the rest of the history.
2. **Against whose keys does the receiver verify?** The decisive question — **see §3.1, which states a
   starting position rather than leaving this blank.** Trust today is a **local** store (`trust/`) naming
   maintainers permitted to seal *here*. When B receives history sealed by A's maintainer, B must decide
   whether that authority means anything locally. **Exchange forces the trust store to become
   distributed, and that is an identity problem, not a networking one.**
3. **What does a receiver do with refs?** Publication is compare-and-swap against local state. Two
   repositories advancing the same ref independently is the divergence merge handles *within* a
   repository; across repositories there is no equivalent concept yet.
4. **Must a bundle be verifiable standalone, or only against a repository that already shares history?**
   NFR-PERF-04's spirit forbids a bundle that is a *summary* — it must be a verifiable subset, never a
   new root of trust.
5. **What does partial history mean?** `verify` re-derives state roots by walking lineage to genesis. A
   receiver holding a suffix cannot do that. Either exchange is always genesis-complete, or lineage
   horizons acquire a meaning they do not currently have.

### 3.1 A starting position on question 2 — to argue against, not to inherit

Added 2026-08-09 at the owner's request, after they named the real tension: **security and cleanliness
against usefulness and intuitiveness.** That tension is genuine. This is offered so the investigation
starts from a position to attack rather than a blank page; **it is not a ruling, and §4 still requires
question 2 answered from the code and the requirements.**

**Most of the sharpness comes from conflating two things that need not be conflated: *having* history and
*trusting* it.**

- **Reception needs no trust.** Objects are content-addressed, so nothing can be forged into an existing
  object id and receiving bytes cannot corrupt what the receiver already holds.
- **Authority is the only thing needing a decision** — whether the sealer's maintainer key means anything
  locally.

**So the trust question is asked once per key, at the moment received history is made authoritative for a
local ref — not per object, and not at reception.** Fetching is cheap and safe; adopting is the act that
requires a decision. That separation is also what users already expect, which is why the ergonomic cost
is far lower than "verify everything against keys you must obtain first" suggests.

**First contact is then the only hard case**, and the proposal is **trust on first use, recorded and
thereafter enforced**: the store notes that key X was accepted at block Y, and every later exchange is
checked against that record. Strictly weaker than out-of-band key agreement, and strictly stronger than
silent TOFU — a later key substitution stops validating and is detectable. It is also honest about what
it is, which matters more here than claiming a strength the model does not have.

**The line that must not be crossed.** Received history must **never** be trusted by default for
convenience. This is the same defect shape as the system proposal's RΔ5 Git-import delta, ruled on
2026-08-02: imported history must be **distinguishable at the object level, permanently and
non-strippably**, or the central claim becomes false for any repository that ever used the bridge.
**Received-but-unadopted history has exactly that property.** If it is indistinguishable from locally
verified history, the claim dies quietly the first time anyone pulls.

### 3.1a §3.1 tested — two clauses were wrong. Corrected 2026-08-09.

§4's investigation tested the proposition rather than inheriting it, as instructed, and found this:

- **Clause 1 holds.** Reception needs no trust; `ObjectId` derives from
  domain ‖ type ‖ schema ‖ len ‖ payload (`id.rs:114-122`) with signatures excluded, so storing received
  bytes is safe independent of any trust decision.
- **Clause 2 was wrong.** *"Authority is the only thing needing a decision"* presumes a verification
  mechanism exists per signer role. **`trust.rs:215` is the sole production `verify_ed25519` call site
  and it hardcodes `SignerRole::Maintainer`.** Author signatures are **never** cryptographically
  verified — for received *or* purely local history. Adopting a remote author's key would have nothing
  to verify against.
- **Clause 3 overclaimed by tense.** TOFU does not exist: DC-11 declined to build it, and
  `security-setup.md:67` and `trust-threat-model.md:61` both state plainly that there is no
  trust-on-first-use rule. **This increment builds it from nothing.**

**The RΔ5 line is unaffected and is now more load-bearing, not less:** because authorship is exactly what
nothing checks, permanent non-strippable provenance marking does real work.

### 3.1b Ruled 2026-08-09 — the trust claim this increment may make

**"This history was sealed by a Maintainer key you adopted."** Buildable today with zero new
cryptographic capability — `add_trusted_maintainer` already is ask-once-per-key adoption, never yet
pointed at a remote peer's key.

**Not: "the received patches' authorship is verified."** That code exists nowhere, and building it here
would absorb DC-53. **State the claim explicitly in design and in user-facing docs; never imply the
stronger one.**

**Also ruled:** exchange is **genesis-complete for v1** — the lineage walk reaches a literal Root
unconditionally and no partial-history concept exists anywhere, including `specs/`. Recorded as a stated
limitation. And **TOFU is new construction**, so it is the part designed first and reviewed hardest.

### 3.1c Ruled 2026-08-09 — multiple adopted maintainer keys (option B)

**The owner ruled B**, after correctly observing that option A is "less functional for production use."
Investigating that objection showed it was **stronger than the architect had put it**:

- **`add_trusted_maintainer` is documented "Add or *replace* the single trusted MAINTAINER key"**, and
  `MaintainerTrustPolicy` (`trust.rs:40-45`) holds one `key_id`, not a list. **Adopting a remote key
  would replace the local one**, so a repository could receive history *or* seal its own, never both.
  That is not a viable production shape, and the architect's "publish/subscribe with local authoring"
  framing of A was simply wrong.
- **B is cheaper than the architect implied.** The on-disk policy is already
  `[maintainer] / required = 1 / keys = ["<id>"]` — **`keys` is already an array and `required` already
  a threshold field.** The parser pins both (`trust.rs:244-263`), but the *schema* anticipated growth.
  **DC-11 declined to implement multi-key; it did not decline to allow expressing it.** The architect's
  "reopens a deliberate constraint" framing overstated the cost.

**Consequence for the design:** the substantive question is no longer *how many keys* but **"trusted for
what?"** Adopting a peer's key cannot sensibly mean "everything that key ever sealed, anywhere"; it
needs scoping — most plausibly to what a named ref points at, with the TOFU record binding key to ref at
first contact. **That is the design's central problem and it is the architect's to answer.**

### 3.2 A sequencing consequence, corrected

An earlier architect framing said a receiver could "verify structurally on receipt." **That is weaker
today than it sounds:** `verify` performs no cryptographic verification of author signatures at all — the
product's only crypto verification call site is a policy signature (`crates/prikk-store/src/trust.rs:215`).

**Superseded 2026-08-09 by §4's investigation: DC-53 is *conditionally* a prerequisite, and under
§3.1b's ruling it is not one.** A receiver verifies exactly as much as a local user does today, so
criterion 1 is reachable without criterion 5. The original claim below stands only for the stronger
trust claim that §3.1b rules out: A receiver cannot meaningfully check what it was sent while
nothing checks author signatures. This should be settled before this increment is sequenced, not
discovered during it.

## D. Design — authored 2026-08-09 by the architect, under rulings 1–4 and option B

Design is the architect's (`EXECUTION-ORDER.md` §6 rule 1). This section is the design; §4's
prerequisites are answered and accepted, and the developer implements from here.

### D1. The finding that shapes everything: single-key trust breaks the *receiver's own* repository

`verify/objects.rs:223` applies the publication-trust check to **`ObjectType::Block | RefState`** — so
`verify_repository` already checks **every block** against the trusted maintainer policy, not merely ref
publication objects. Combined with DC-11's single-key policy, that means:

> **If Alice adopts Bob's key today, every block Alice sealed herself becomes untrusted, and her own
> repository fails `verify`.**

Option A was therefore never merely "less functional" — **it is destructive to the receiver.** The
owner's instinct was right and the evidence is stronger than the argument that produced the ruling.

### D2. Trust model — a set of adopted keys, and object-trust separated from ref-authority

**The policy becomes a set.** `MaintainerTrustPolicy` holds `Vec<(key_id, public_key)>`; the parser
accepts `keys = ["a", "b", …]` and `required = 1` retains its present meaning (**a block needs one
trusted signature, not a threshold of several**). The on-disk shape already expresses this — DC-11 pinned
the values, not the schema.

**"Trusted for what?" — trusted to have sealed *objects*, not to advance *refs*.** This is the design's
central answer, and it falls out of how the code already separates the two:

- **Object trust** is what `verify` checks: a block is valid if *some* adopted key signed it.
- **Ref authority** is a separate, local, CAS-guarded act. `RefStore::publish` requires a signature from
  **this operator's** signer, and `verify_signer_trusted` gates sealing.

**So adopting Bob's key lets Bob's blocks be valid objects here. It does not let Bob move any ref.**
Advancing `heads/main` still requires a local seal by the local operator. The blast radius of adoption is
therefore much smaller than "Bob is trusted", and it needs no ref-scoped trust machinery — which would
mean making `verify`'s object scan ref-aware, a far larger change for no additional safety.

### D3. Provenance — already intrinsic, and this is how RΔ5 is satisfied

**No new provenance mechanism is needed, and none should be invented.** The sealer's `key_id` lives
inside the block's own signature. It cannot be altered without invalidating that signature, and it cannot
be stripped without making the block untrusted. **That is exactly RΔ5's "distinguishable at the object
level, permanently and non-strippably"** — provenance recorded in the objects, not in documentation or a
side file.

**What is missing is only the reporting.** `verify` must surface, per block, **which key sealed it**, so
an auditor can ask "which parts of this history did *I* seal?" and get an answer. That is a reporting
change, not a format change.

### D4. Import never advances a local ref

An import writes **objects** and **adopted-key records**. It **must not** advance `heads/*` or create a
ref the local operator did not ask for. Received refs land in a distinct namespace the local operator
never seals to; incorporating that work into local history is an ordinary **merge** (DC-74/DC-75) —
already built, already reviewed, already recording the merge structurally.

**Corrected 2026-08-09 by Stage 3.** The claim above — *"receive, then merge, using machinery that
exists"* — **is false as built, and the error is the architect's.** `execute_merge` (`merge_execute.rs:65`)
validates `from_ref` through `validate_local_branch_ref`, which **rejects the `remotes/` prefix** by
design (`refs.rs:386-389`). So a received ref cannot be a merge input today.

**The gap is real and not small.** `prepare_merge_evidence` assumes both sides are ref-log-backed local
branches with a `previous_ref_state_id` chain reachable through `RefStore`; a received pointer has
neither a ref-log nor CAS semantics. Closing it needs a new `MergeEvidenceTarget` variant and a ruling on
what confluence means against a source with no local publication history — **its own increment, opened as
DC-85.**

**Practical consequence today:** an operator can receive, inspect (`branch list`, `verify`, `log --ref`),
and adopt trust for a bundle, **but cannot fold it into local history.** Exchange is complete for an
auditor and incomplete for a collaborator.

**Amended in premise 2026-09-16 by RFC 154 (owner-accepted).** The letter stands: import advances
nothing and creates no ref the operator did not ask for. A received RefState signed by a locally adopted
key may become a local ref through a **separate, explicit adoption act**: fast-forward only, not
re-signed, with every block replay-verified and publication protected by the local compare-and-swap
(`rfcs/accepted/154-shared-branch-adoption.md` §4). Until that act ships, the consequence above holds as
written.

### D5. The TOFU record

Adoption binds, in the trust store: **key id, public key, the block id at which it was first accepted,
and the ref name it arrived under.** Thereafter every exchange from that peer is checked against the
recorded public key; a changed key for a known key id is **refused, not re-prompted**.

**This is new construction**, not an extension — DC-11 declined to build TOFU and both
`security-setup.md:67` and `trust-threat-model.md:61` say so today. Those two sentences must change in the
same increment, or the docs become false.

### D6. The exchange artifact

A **verifiable subset**, never a summary — NFR-PERF-04's spirit forbids a bundle that is a new root of
trust. It carries the objects reachable for the exchanged lineage, **genesis-complete** (ruling 2), and a
receiver verifies it with **`verify_repository` unchanged**. The only new code is a serialization
boundary and the import path.

### D7. What the implementation must still answer, before writing code

1. **Does the multi-key parser stay strict and fixed-shape?** DC-11's parser is deliberately not a general
   TOML implementation. Extending it to a list must not turn it into one.
2. **What does `required` mean once several keys exist?** This design says "one trusted signature
   suffices"; confirm nothing in `trust.rs` or DC-11 assumed otherwise, and **report if it did**.
3. **Which ref namespace do received refs land in**, and does anything today assume every ref under
   `refs/` is locally sealed? Check `branch list`, `history`, `verify` counts.
4. **Does `verify`'s per-block trust check cost change** when the policy is a set rather than one key? It
   runs per block; `FINDINGS.md` already records `verify` as O(N³).

## 4. Blocking prerequisites

- §3's five questions answered **from the code and the requirements**, reported before any design, in
  the pattern that has widened the recorded scope in five consecutive increments.
- **Question 2 answered first**, with §3.1 treated as a position to test rather than adopt. If
  distributed trust has no acceptable answer, the rest is wasted work.
- **§3.2's dependency confirmed or refuted:** is DC-53 genuinely a prerequisite? If so this increment
  cannot start before it, and the 0.20.0 sequence changes.
- An explicit statement of whether **criterion 1 of the status-claim criteria** is satisfied by exchange
  alone, or requires transport too. **That determines whether this increment moves the badge.**

## 5. Non-goals

- **Network transport of any kind**, unless §2's recommendation is rejected.
- Any new dependency, and any change to `ALLOWED_THIRD_PARTY`.
- Conflict resolution, patch aggregation, remote-tracking refs — each separate, and none required to
  move history between two machines.
