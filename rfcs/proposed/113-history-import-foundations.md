# RFC 113 — History import foundations (Git, Subversion, CVS)

**Status.** **Proposed.** Originated by the project owner 2026-08-19: *"I would like to offer migration
tools to migrate from other VCS projects with the history safely preserved… the primary target is Git
projects. In addition, I would like to support Subversion projects and CVS projects in order to help
growth and maintenance of such great ancestors."*

**Recorded with the architect's assessment so the direction is reviewable before anyone designs against
it. No design exists and implementation must not start from this record.**

**Amended 2026-08-19 — the work splits across two projects.** The owner ruled that the decoding and
encoding tooling becomes **its own project**, so other VCS projects can use it for their own migrations:
decoders for Git/Subversion/CVS into an intermediate representation, and encoders from it.

**The decisive reason is dependency surface, not tidiness.** prikk's entire third-party runtime
dependency set is **five crates** — `ed25519-dalek`, `getrandom`, `rustix`, `sha2`, `windows-sys`. A Git
decoder needs `gix` (on the order of a hundred crates) or `libgit2` (C, and `forbid(unsafe_code)` holds
everywhere outside `prikk-ffi`). Either is a step change in the audited surface of a product whose claim
is verifiability, and SVN and CVS follow. **RFC 112 is the same lesson one size down**: three core
operations ended up in the wrong crate. Three VCS parsers inside prikk would be that mistake, larger and
harder to reverse.

**So this RFC is now prikk's *import contract*, not an importer plan.** It states what prikk accepts,
how import provenance is recorded, what `verify` says about imported history, and which source features
must be refused rather than approximated. **The separate project is the tooling that satisfies this
contract.** Every decision in §4 remains prikk's and remains necessary; they simply stop being entangled
with Git parsing.

**Scope.** The **shared** problem, not any one source. Git, Subversion and CVS differ enormously in
difficulty (§5), but they fail against prikk's model in the *same three places*, and answering those
once is what makes three importers possible instead of three separate research projects.

## 1. The central difficulty: prikk records what these systems do not

prikk's model is **operations against node identity**. A Patch says *this node moved, this node's
content changed*. `NodeId` is first-class and carried in sealed history.

**Git has no node identity.** A commit is a tree snapshot; renames are **inferred at read time** by
similarity heuristics and are not recorded. Subversion records copies path-wise but not node identity.
CVS versions files independently with no atomic commit at all.

**So an importer must either infer what prikk requires, or admit it did not have it.** Inference is a
guess, and prikk's entire proposition is that it does not guess about history. **The import cannot make
the source's history more precise than the source recorded it**, and any design that appears to must be
wrong.

**The honest form of this is a recorded confidence, not a silent heuristic**: an imported operation that
was *derived* must be distinguishable from one that was *stated*. Which mechanism carries that is §4's
question; that it must exist is not negotiable.

## 2. The signature problem, and why DC-53 makes it sharper — and easier

`verify` now cryptographically checks every reachable Patch's AUTHOR signature (DC-53, 2026-08-18).
**Imported commits cannot carry valid prikk AUTHOR signatures.** A Git author did not sign a prikk
preimage; even a GPG-signed Git commit signed *Git's* object, not prikk's.

Three options, and two are wrong:

- **Sign imported patches with the importer's key.** This asserts that the importer authored ten years
  of someone else's work. **Rejected** — it is a false statement, and DC-53 exists to make authorship
  claims true.
- **Forge or synthesise author signatures.** Not possible and not desirable; excluded for completeness.
- **Import them unsigned, and let `verify` report them.** DC-53 already produces exactly the right
  outcome for this: **`Unverifiable`** — *"no key material recorded for this `key_id`"*, which passes
  `verify` while being reported, never silently treated as sound.

**The vocabulary imported history needs already exists, and it was built for a different reason.**
Imported history is precisely "present, readable, and not verifiable as authored" — the state DC-53
Stage 1 defined and made visible. **A design should use it rather than invent an import-specific
status.**

**What still needs deciding** is whether the *importer* additionally makes a signed claim — "I imported
this from that source" — which is a true statement it can sign. That is §4.

## 3. What "safely preserved" has to mean

The owner's phrase is the requirement, so it must be given a checkable meaning. **It cannot mean
"verified", because prikk cannot verify what the source never signed.**

**It should mean reproducible and comparable:**

- The import records **what it was made from** — source commit/revision identifiers, source repository
  identity, importer version, and the parameters that governed inference (§1).
- **Re-running the same importer over the same source produces the same prikk objects**, or the
  difference is explainable. Determinism is what lets a third party check the claim at all.
- A reader can always tell **"prikk verified this"** from **"this was faithfully imported and prikk
  verified nothing about its authorship."**

**The third point is the one that will be lost if it is not held deliberately.** Ten years of imported
history that looks like native history, in a tool whose selling point is verifiability, is worse than no
import — it is the manufactured-verification failure RFC 110 §4 already names, arriving through a
different door.

## 3.1 The three questions the intermediate representation turns on

**Raised by the project owner 2026-08-19**, and they are the design, not preliminaries to it: *"What is
'record'? What should be preserved? What can be omitted?"*

**The trap they identify is real.** An IR designed to be *neutral* across Git, SVN and CVS converges on
their common denominator — **snapshots plus metadata** — which is precisely what prikk is not. Encoding
prikk from a snapshot IR would require exactly the identity inference §1 warns about, and a neutral IR
offers nowhere to record that the inference happened. **Design the IR for faithfulness with provenance,
not for neutrality.**

### What is a "record"?

The atom differs per source, and not only in size: Git's is a commit (a snapshot plus metadata), SVN's a
revision, CVS's a **per-file** revision with changesets that must be *reconstructed*, prikk's a Patch of
operations against node identity.

**So the IR must decide what its own atom asserts**, and — more importantly — **carry how that atom came
to be**: stated by the source, or derived by the decoder. A CVS changeset and a Git commit may occupy
the same slot in the IR while being epistemically different things, and an IR that cannot say so is
lying by omission.

### What should be preserved?

**The test: preserve what a future reader's ability to *check something* depends on.** Not "everything",
which is unachievable, and not "what looks important", which drifts.

That reasoning keeps content, structure, ancestry, messages, authorship metadata **as claims rather than
verified facts**, and — the one most likely to be dropped as useless — **the source's own identifiers and
signatures**. A Git commit SHA and its GPG signature cannot verify anything in prikk, but they are **the
only cryptographic link back to the original**, and preserving them opaquely is what lets a third party
check the import against the source it claims to come from. **That is what §3's "safely preserved" can
actually mean**, and it is lost forever if the decoder discards it as unverifiable-here.

### What can be omitted?

Two classes are safe: **representation rather than assertion** (packfile layout, delta encoding, index
state, reflogs — reconstructible or purely local), and **advisory data known to be unreliable**, of which
SVN's mergeinfo is the standing example. **Preserving wrong data as though authoritative is worse than
dropping it.**

One class is never safe to omit silently: **anything whose absence makes a remaining claim look
stronger than it is.**

**And the governing rule across all three questions: the boundary itself is recorded.** An import states
what class of information it dropped and what it derived, so a reader knows the *shape* of the loss
without re-deriving it. This is the same discipline as DC-53's `Unverifiable` and RFC 111's refusal of
silent caps: **a known limit that is written down is a property; the same limit unwritten is a defect
waiting to be discovered by whoever trusts the output.**

**These answers must be shared across decoders even though they resolve differently per source.** If each
decoder invents its own notion of loss, the IR cannot compare them, and "how faithful was this import?"
stops having an answer.

## 4. What a design must decide

1. **Where import provenance lives: attestation or payload.** `payload/attestation.rs` already carries a
   claim *about* a block, as its own signable object, and refs already carry
   `required_attestation_ids`. **An import claim is exactly that shape** — an assertion by a named party
   about history, separable from the history itself. Sealing source metadata *into* patch payloads would
   make hearsay look prikk-verified, which RFC 110 §4 rules against. **This is my recommendation, not a
   ruling** — the design must weigh it, because it also determines what a receiver of a bundle sees.
2. **How derived operations are marked** (§1). Rename inference is the concrete case: Git gives
   similarity, prikk wants identity. What does an importer record when it is 80% confident?
3. **What the importer signs, if anything.** "I imported this" is true and signable; "this person
   authored this" is not. Decide whether an import attestation is required, optional, or absent — and
   what `verify` says about an import with none.
4. **Whether imported history may be sealed at all**, and by whom. Sealing is a maintainer act with a
   verified signature; a maintainer sealing imported blocks is making a real claim about inclusion, which
   is defensible — but it must be a deliberate answer, not a consequence of reusing the seal path.
5. **What the source-side floor is.** Which Git features are refused rather than approximated —
   submodules, octopus merges beyond two parents, replace refs, grafts, shallow clones. **A refusal is a
   better outcome than a silent approximation**, and the list belongs in the design, not in bug reports.

## 4a. Architect's rulings, 2026-08-27

**The owner accepted this RFC as the next theme. These rule the §4 decisions that are the architect's;
§4.3–§4.5 are marked below as the owner's and are deliberately left open.**

### §4.1 — provenance lives in `Attestation`, and this would be its first real use

**Ruled: `Attestation`, not the patch payload.**

**The decisive fact, verified rather than assumed:** `AttestationPayload` is fully defined
(`target_block_id`, `policy_version`, `plugin_set_hash`, `results`, `status`), admitted at schema 1,
canonically encodable, and carried through every format gate — and **nothing in production constructs
one.** Its only construction site is `vectors.rs`, the test-vector generator. Four consecutive G1
fixture coverage tables record it as absent for exactly this reason.

**So the choice is not neutral.** Putting import provenance in the patch payload would change what a
`Patch` *is* for every prikk repository, imported or not. Putting it in an `Attestation` activates a
type that has been defined, gated, and vector-tested for its whole life without ever being written —
**and whose existing shape already says what an import needs to say**: a statement *about* a target,
carrying a status and a set of results, signed separately from the thing it describes.

**A design must still answer** whether `AttestationPayload`'s current fields fit an import statement or
need a schema, and **that is a format change if so** — see RFC 114's frozen surface.

### §4.2 — derived operations are marked at the operation, not inferred at read time

**Ruled: an importer that infers must record that it inferred, in the object it writes.**

Rename inference is the case: Git records a deletion and a creation, and a renamer's confidence is a
heuristic. **A reader must be able to tell an asserted rename from an inferred one without re-running
the heuristic**, because re-running it is exactly what a different tool version would do differently.

**This follows the project's own established line** — facts derive, judgment is authored, the join is
gated. **An inference is judgment.** It must be recorded as such, not recomputed by whoever reads next.

### §3.1 — the three questions the IR turns on

**Ruled, in the RFC's own terms:** the IR's atom is *what the source system could actually guarantee*,
not what prikk would like it to have guaranteed. **The governing rule the RFC already states — "the
boundary itself is recorded" — is adopted as binding**, and it answers all three: a record asserts what
its source guaranteed, preservation covers whatever a future reader's ability to *check* depends on,
and omission is permitted only where the omission is itself stated.

**This is the same discipline as `verify`'s stage inventory**: absence must be explicit, never silent.

### Owner's, not the architect's — left open deliberately

- **§4.3 — what the importer signs, if anything.** *"I imported this"* is signable; *"this person
  authored this"* is not the importer's to assert. **This is DC-35 territory** — who may sign what —
  and the architect should not settle it alone.
- **§4.4 — whether imported history may be sealed at all, and by whom.** Sealing is a maintainer act.
  Whether an import may be sealed, and whether the importer may be that maintainer, is an authority
  question.
- **§4.5 — the source-side floor.** Which Git features are refused rather than approximated is product
  scope: it decides who can migrate and who is told no.

**Architect's recommendations 2026-09-13, for the owner to rule** (`.git-exclude/reviewed/owner-blocked-items-review-2026-09-13.md`): §4.3 — the importer signs the *import declaration* under its own key, source authorship carried as recorded text, never asserted; §4.4 — only an adopted maintainer seals imported history, and the importer's key must be one; §4.5 — refuse, never approximate: submodules, symlinks, non-UTF-8/backslash paths, replace refs and grafts, octopus merges in the first cut; commit signatures carried as text; every refusal reported before any object is written.

**License and copyright (owner's question, 2026-09-13):** the proposals create no exposure and reduce one. Migration copies bytes whose license and copyright are unchanged by the move; the importer needs the right to redistribute the source project, which is theirs to hold, not the tool's to confer. §4.3's rule — sign the *import*, carry source authorship as recorded text, never assert it — is what keeps attribution intact (the notices licenses such as Apache-2.0, MIT and the GPLs require) and prevents the one real misrepresentation an importer could make: claiming authorship of others' work under their own key. Commit signatures carried as text are public data. **Two things to watch, neither a license matter:** (1) author names and e-mail addresses from the source are personal data and, once sealed, permanent — the import contract must say identities are carried as-is, and a pseudonymisation option is the owner's later call; (2) the decoder project (brygge) owns its own dependency licenses — reading Git, Subversion or CVS through their libraries or CLIs is its licensing question, and prikk's import contract does not touch it.

**§4.3–§4.5 are the gate on any per-source RFC.** §5's sequencing (foundations, then Git, then SVN,
then CVS) cannot start until they are ruled.

## 5. The three sources are not one problem, and should not be one increment

**Git — hard but tractable.** Content-addressed, atomic commits, a real DAG. The work is identity
inference (§1) and the feature floor (§4.5).

**Subversion — different, not easier.** Atomic revisions help, but branches and tags are *path copies*,
not first-class refs, so branch identity must be reconstructed by convention (`/trunk`, `/branches/x`)
that many real repositories violate. Mergeinfo is advisory and frequently wrong.

**CVS — a research problem with known-imperfect prior art.** There are **no atomic commits**: a
changeset must be *reconstructed* from per-file revisions by clustering on author, message and time
window. This is what `cvs2svn` and successors do, and they do it imperfectly by nature. **A CVS importer
cannot be more faithful than the reconstruction**, and §3's honesty requirement bites hardest here.

**Recommended shape: this RFC's foundations first, then one RFC per source**, in that order, with CVS
last and explicitly permitted to conclude that a faithful import is not achievable and that the honest
deliverable is a lossy one, clearly labelled.

## 6. Where this sits against current work

**Not scheduled, and not next.** It depends on decisions that are open:

- **Badge criterion 2 (format stability)** — an importer writes a great deal of history in one go. Doing
  that before the compatibility promise exists means the largest repositories in the project's life are
  written against an unstated format contract.
- **Badge criterion 1 (sync)** — an imported repository that cannot be exchanged is half a migration.

**Neither blocks recording this, and recording it now is the point**: the design decisions above are
easier to make before an importer exists than after one has shipped and set precedent.

## 7. Non-goals

- **Not a Git compatibility layer.** prikk is not a Git wrapper and does not use `.git/` as storage; this
  is one-way import, not interoperability.
- **Not round-tripping.** Exporting prikk history back to Git is a separate question and not implied.
- **Not authorship laundering.** No mechanism here may make imported history appear natively authored or
  natively verified.
- **Not a promise of completeness.** §4.5's refusal list is a feature.
- **Not this project's implementation.** The decoders, the IR and the encoders live in the separate
  project ruled above. What lives here is the contract they must satisfy — and prikk must be able to
  state that contract without depending on any of them.
