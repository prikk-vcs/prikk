# RFC 110 — Agent safety and code provenance

**Status.** **ARCHIVED 2026-09-13 on the owner's authorization** — a concept record with no requirement behind it and §2–§5's objections standing. Returns the day a requirement names it. Earlier: **Proposed** — rough concept, direction not yet established. Authored by the project owner
2026-08-18, who states the direction is unproven. Recorded with the architect's assessment of whether it
is right. **No design exists and implementation must not start from this record.**

**Tracks.** A governance and safety layer alongside RFC 109's interface work.

## 1. The concept

AI-native development needs a dedicated governance layer beyond a fast CLI, addressing two operational
challenges.

**Pillar 1 — transactional sandboxing and non-destructive operations.** Agents rely on hypothesis testing;
version control treats commands as immediate and irreversible.

- **Shadow commits and invisible checkpoints** — ephemeral, non-polluting restore points created
  automatically before agent actions, for zero-risk execution and instant recovery.
- **Atomic agent transactions** — multi-step operations wrapped in isolation; a failed test suite or
  hallucination rolls back cleanly without cluttering history.
- **Intent previews and dry-runs** — human supervisors or validator agents inspect structural changes
  before they reach the primary workspace.

**Pillar 2 — AI provenance and context lineage.** Knowing *how* and *why* code was written becomes as
important as *what* changed.

- **Generation metadata** — model, prompt parameters, execution timestamp, recorded with the change.
- **Context map tracking** — the dependencies, repository files and external documentation the agent
  ingested to produce it.
- **Auditability and IP compliance** — lineage for debugging AI regressions, security audits, and
  intellectual-property verification.

**Guiding principle: intent-driven safety defaults**, so agents can be granted high autonomy without
sacrificing integrity or auditability.

---

# Architect's assessment — the direction, tested

The owner asked whether the direction is right. **Partly.** Below is where it holds, where it does not, and
what would make the weak half sound.

## 2. Pillar 1 is in direct tension with prikk's core proposition — and the resolution is not a compromise

**prikk's value is that everything is permanent, attributable and verifiable.** Shadow commits are
deliberately **impermanent and unattributed**. Both in one tool is coherent only if the boundary between
them is crisp, and the concept does not draw it.

Only two resolutions exist, and one is unavailable:

- **Shadow state sealed but marked ephemeral** — requires deleting sealed history. prikk has no amend, no
  rewrite, no force-push, and `compact` reclaims container records rather than blocks. **Not available, and
  making it available would dismantle the guarantee prikk exists to provide.**
- **Shadow state entirely outside sealed history** — a per-Workspace scratch layer. **This is the only
  coherent option**, and it means Pillar 1 is a *Workspace* feature, not a history feature.

**So Pillar 1 largely redistributes into RFC 108.** Its "workspace cleanly rolls back," multi-path
experimentation, and isolation are RFC 108's subject; its reference to not "cluttering `.git` reflogs"
does not apply, as prikk has no reflog. The dry-run half already exists — `checkout --plan-only`,
`merge-plan`, `inverse-plan`, `rollback-preview` and others.

**What is genuinely new here is a policy, not a mechanism:** *when* to checkpoint automatically, and what
an agent is permitted to do without one. That is worth designing, and it is much smaller than the pillar
implies.

## 3. Pillar 2's value proposition currently has no floor

The stated value is **auditability, security compliance, and IP verification**.

**`verify` does not check author signatures.** Confirmed. Badge criterion 5 records it: *"signatures are
carried and preserved, but `verify` never checks them — the only cryptographic verification in the product
is one policy-signature call site."*

**So provenance metadata would sit on top of history whose authorship prikk never verifies.** A compliance
story built there answers "what does this record claim?" while leaving "was this actually signed by whom it
says?" unchecked. **That is a floor problem, not a feature gap** — and it means **criterion 5 is a
prerequisite of Pillar 2, not an adjacent nicety.**

## 4. The deeper objection: sealing unverifiable claims manufactures the appearance of verification

prikk's signature attests that *an author sealed these bytes*. If a block payload also contains "generated
by model X, from prompt Y, having read files Z," the signature covers those bytes too — **so it looks
verified to exactly the same degree as the content.**

It is not. **prikk cannot observe what an agent read or which model ran.** Those are assertions, and any
author — human or agent, honest or not — can write anything there. Sealing them into the block gives
hearsay the appearance of prikk-grade verification, in a tool whose entire proposition is that its claims
are checkable.

**This is the strongest reason the direction, as written, is not yet right.**

### 4.1 The constructive answer: prikk already has the right home for a claim

`crates/prikk-object/src/payload/attestation.rs` defines an attestation carrying `target_block_id`,
`policy_version`, `plugin_set_hash`, `results`, and `status` — **a claim *about* a block, as its own
object, rather than content sealed *into* it.** Refs already carry `required_attestation_ids`.

**Provenance belongs there.** An attestation is structurally honest about what it is: an assertion by some
party concerning a block, separable from the block's own integrity, and independently signable by whoever
makes it. A reader can then distinguish *"prikk verified this content"* from *"this party claims it was
generated this way"* — which is the distinction the concept currently collapses.

## 5. Context map tracking is the largest unpriced item

*"Records the exact dependencies, repository files, and external documentation ingested by the agent."*

Three costs the concept does not price:

- **Size.** An agent may read hundreds of files per change. Recorded per change, permanently, this could
  exceed the content it describes.
- **Unverifiability**, per §4 — and worse than model identity, because the claim is large and detailed
  enough to look authoritative.
- **Permanence.** Paths and external URLs are recorded forever, with no rewrite. Internal directory
  structure and documentation locations become permanently public in any shared repository.

**Combined with RFC 109 §6's prompt-secrets problem, the rule I would propose is the same:** record a
*digest* of the context set, not the set. It is compact, comparable, and reveals nothing — and it answers
"was this the same context?" which is the question that matters for debugging a regression.

## 6. What is right about the direction

Stated plainly, because §2-§5 are critical:

- **Intent-driven safety defaults are correct**, and prikk is unusually well positioned: the destructive
  operations the concept guards against **do not exist here**, worktree writes already refuse rather than
  clobber, and the dry-run surface is broad. The work is exposing that posture, not inventing it.
- **The separation instinct is right.** Safety and lineage are cross-cutting: they apply whether or not
  Workspaces exist, and to human authors as much as agents. That justifies this being its own RFC rather
  than dissolved into 108 and 109.
- **Auditability is a genuine differentiator** — once criterion 5 gives it a floor.

## 7. What a design must answer first

1. **Is criterion 5 accepted as a prerequisite of Pillar 2?** If not, what does an audit claim mean over
   unverified authorship?
2. **Attestation or payload**, per §4.1 — and if payload, what answers the manufactured-verification
   objection.
3. **Digest or content**, per §5 and RFC 109 §6 — decided before anything provenance-shaped is sealed,
   because prikk cannot unseal it.
4. **What remains of Pillar 1 after RFC 108 absorbs the mechanism** — the checkpoint policy, and what an
   agent may do without one.
5. **Whether an unverifiable claim should be signed at all**, or recorded without a signature to make its
   status legible.

## 8. Non-goals

- **Not deletable history.** §2 rules this out; anything requiring it is a different project.
- **Not agent policy enforcement** — permissions, approval flows, sandboxed execution. This records and
  protects; it does not govern what an agent may attempt.
- **Not sealing unverifiable claims as though verified**, per §4, unless that objection is answered.
