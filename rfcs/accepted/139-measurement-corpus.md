# RFC 139 — The measurement corpus: a repository worth measuring against

**Status.** **ACCEPTED by the project owner 2026-09-06**, the same day it was opened.

**Moved to `rfcs/accepted/` 2026-09-06 on acceptance.** The trigger is **design complete**, not
handoff issued — RFC-000's 5-folder variant defines `accepted/` as *"review complete; implementer may
start"*, and §9's increment 1 is startable. Moved in the same act as the acceptance rather than when
a handoff issues, which is the error this project corrected three times in the preceding week.

**What the acceptance covers, stated because a bare acceptance is scope-ambiguous.** It accepts the
whole design as written: §3's ruling that the corpus is **a profile plus a deterministic builder and
never a stored repository**; §4's provenance requirements and the demand for a second, structurally
different profile; §5's determinism-as-a-tested-property; §6's deliberate refusal to fix a depth
target ahead of the build-cost curve, with 2,048 sealed blocks as a **floor** rather than a target;
§7's placement and its two prohibitions (**not a CI job, not a correctness fixture**); and §9's four
increments in their stated ordering. **It does not rule anything §8 lists as out of scope** — RFC 133
§6 in particular remains the owner's and is untouched by this acceptance.

Originally opened as: **PROPOSED, 2026-09-06**, at the project owner's instruction to proceed on the
measurement corpus, which they approved as the next scheduled item on 2026-09-06 (`ROADMAP.md`, "The
order", item 1).

**Nothing in this RFC awaits the owner.** Every question it raises is a design question and is ruled
here. It is recorded explicitly because two of this project's recent RFCs submitted questions that
were the architect's to answer, and the owner was right to send them back.

**This RFC does not depend on RFC 133 §6, and must not be scheduled behind it.** §6 asks whether
memory independence becomes a *stated requirement*; RFC 133 §7 holds increments from **that** RFC
until it is ruled. The corpus is an **instrument**, not a requirement and not a gate. Which gates —
if any — are built on it is decided elsewhere, by §6 for memory and by whoever proposes any other.
Building the instrument does not presuppose any of those answers. See §8.

**Author-review independence.** The architect wrote this RFC and is also its only reviewer — the
standing gap recorded on every architect-authored design in this project. Compensated at
implementation review, not here.

**Tracks.** Measurement method. **No behaviour change is proposed in this document**, and nothing in
it touches the object model, the format, or any shipped surface.

---

## 1. The weakness, named twice independently

Two reviewers reached the same conclusion from opposite ends, four days apart, without either
prompting the other.

**The external architect**, closing their block-materialization review
(`.git-exclude/tasks/architect/audit-20260904-block-materialization/block-materialization-review.md`),
on being asked what to build first:

> **If you asked me what to build first, it would not be the snapshot — it is the corpus.**

They gave three reasons: it turns RFC 136 §9 item 1 from unmeasured into measured; it lets a
checkpoint cadence be tuned against real depth-versus-storage curves instead of `REANCHOR_BOUND`'s
integrity-chosen `64` (`crates/prikk-store/src/lifecycle_cache/incremental.rs:36`); and it **retires
the standing methodological weakness that every performance decision in this project currently
inherits**. They then held their own recommendation: committing storage and a schema-touching field
to an unmeasured benefit is *"the one place I would hold, and I would hold it until the corpus
exists."*

**The architect**, in RFC 136 §9.1's own statement of limits, had already recorded the first half of
the same finding — limit 1, verbatim:

> It is prikk's git history, not prikk sealed history, **because no prikk repository with realistic
> development history exists to measure.**

RFC 136 §9.2.8 records the convergence and endorses the recommendation. This RFC is that
recommendation, designed.

## 2. What performance claims rest on today

Every performance statement this project makes falls into one of four classes. Only the first is
sound, and it covers two properties.

| Class | Examples | What it is worth |
|---|---|---|
| **A standing gate** | `rfc111_index_decode_cost_gate.rs`, `rfc111_seal_decode_cost_gate.rs` | Real. Observed failing before its fix. Two properties, both about **time** |
| **A one-off synthetic measurement** | RFC 133 §2's memory shape; DC-59/DC-62/DC-64/DC-69/DC-92's harness runs | True when taken, against a repository that no longer exists and cannot be rebuilt identically |
| **A proxy from a different system** | RFC 136 §5.1, §5.3, §9.1 — prikk's **git** history standing in for prikk sealed history | Bounded and honest, but the reviewer confirms some quantities have **no honest git proxy** at all |
| **Source reading** | RFC 133 §3's `AUD-01`, `AUD-02` | Nothing. RFC 133 §2 is this project's own demonstration that source reading gets cost wrong |

**The second class is the one this RFC addresses, and its defect is not accuracy — it is
incomparability.** `dc59_commit_benchmark.rs` and `dc92_lineage_replay_benchmark.rs` are careful
instruments: both are `#[ignore]`d deliberately, both document their methodology at length, both
state what they could not measure rather than defaulting it to a number. Neither is being faulted
here. But each **generates its own repository, measures it, and discards it.** Two consequences
follow, and they are structural rather than a matter of care:

1. **No two measurements in this project's history were taken against the same repository.** A figure
   from DC-64 and a figure from DC-92 cannot be placed side by side, because the thing measured
   differed in shape, not only in the axis under test.
2. **No measurement can be re-taken.** When RFC 133 §2 had to check whether DC-62's problem statement
   still described the present, it could not re-run DC-62's measurement — it had to build a new
   instrument and measure again. That is why the stale claim survived long enough for the architect
   to repeat it.

**The shapes these harnesses build are also, by design, not realistic**, and they say so. `dc92`
holds tree size fixed by churn — deleting the oldest tracked file and creating one at a fresh path
each generation — precisely so history depth is isolated from repository size. That is correct for
its question and wrong for every question about what a real repository costs.

## 3. The corpus is a specification and a builder, not a stored repository

**This is the load-bearing decision in this RFC, and the obvious alternative is worse.**

The obvious reading of "build a corpus" is: create a `.prikk` repository of a few thousand sealed
blocks and commit it. **That is refused, for four reasons, the last of which is disqualifying on its
own.**

1. **A stored repository is a format hostage.** RFC 114 guarantees any release can read every object
   any prior release wrote — so a stored corpus would remain *readable*. But a corpus frozen at
   `Patch` schema 4 measures the cost of objects **a writer that no longer exists** produced. The
   thing under measurement is today's writer and today's reader; a fixture that ages silently
   measures neither, and would drift furthest exactly when a format change makes measurement most
   valuable.
2. **Size.** A corpus deep enough to be useful (§6) is large enough that committing it would dominate
   the repository, and every clone would pay for it whether or not it ever measures anything.
3. **It cannot be extended.** A corpus that must grow deeper to answer a new question would require
   regenerating and re-committing the whole artifact, and the new artifact would not be comparable to
   the old one — reintroducing the exact defect §2 identifies.
4. **A sealed corpus requires a signing key, and the corpus is worthless without seals.** Depth is
   measured in **sealed blocks**. Sealing requires a MAINTAINER key. Committing a repository whose
   history is sealed means either committing the secret that sealed it — never — or committing a
   history nobody can extend, which is (3) again. The existing harnesses solve this by deriving a
   **fixed benchmark key from a fixed seed at build time**, which works precisely because the
   repository is built rather than stored.

**Therefore the corpus is two artifacts, and neither is a repository:**

- **A profile** — a small, human-readable, version-controlled description of a history's *shape*,
  with its provenance recorded (§4).
- **A builder** — a deterministic program that materializes a real prikk repository from a profile
  (§5), on demand, using today's code.

**The repository itself is derived, disposable, and never committed.** It is built into an ignored
working directory, measured, and may be deleted; rebuilding it from the same profile yields the same
repository (§5). This is the same move RFC 118 made for a different problem — *derive, never
transcribe* — applied to a fixture instead of a document.

## 4. Realism, and the boundary of the claim

**A profile is derived from a real history, and records how.** An invented distribution would
reproduce the weakness this RFC exists to retire: it would be precise, reproducible, comparable, and
about nothing.

**Each profile must record, in the file itself:**

- the source repository, and the exact commit range or revision it was extracted at;
- the extraction command, verbatim, so a reader can re-derive the numbers rather than trust them;
- the date of extraction;
- the distributions themselves — at minimum files-changed-per-commit, and the file-path reuse
  structure that determines whether successive changes touch the same paths or disjoint ones.

The second of those is not bureaucracy. RFC 136 §9.1's measurement is trustworthy **because** it
names `git log --pretty=format:'@@%H' --name-only --no-merges -n 600` and lets anyone re-run it; the
same measurement without its command would be a number with a story attached.

**One profile is not enough, and RFC 136 §9.1 already says why.** Its limit 2, verbatim:

> It is one project with a disciplined one-theme-per-commit rhythm. A project that accumulates many
> small fixups against the same file would show a higher ratio. **This bounds *prikk's* case, not
> every case.**

A corpus built only from prikk's own rhythm would let this project tune itself to itself. **At least
one profile must come from a structurally different project** — specifically one with the opposite
property, many small changes concentrated on few files — so that a decision tuned on one can be
checked against the other. §9 increment 4 is that profile.

**Extracting a profile from another project's public history is a factual measurement of a public
artifact, not a use of its content**, and the profile stores aggregate distributions, never file
contents or paths from the source. It is ruled admissible here; recording the source and range is
what keeps it checkable.

## 5. Determinism is a tested property, not an aspiration

**If the builder is not deterministic, the corpus retires nothing** — two measurements taken a week
apart would again be measurements of two different repositories, which is §2's defect wearing a new
name.

**The requirement:** building a corpus twice from the same profile, on the same platform, must
produce repositories that are equal in every respect measurement can observe — identical sealed head,
identical state roots at every depth, identical object set.

**It must be proven by a test, not asserted by the builder's design.** The test builds a shallow
corpus twice and compares; shallow, so it can run in the ordinary suite rather than joining the
`#[ignore]`d instruments. A determinism test that only runs when someone remembers to run it protects
nothing.

**Two known sources of nondeterminism must be closed explicitly**, because both are real in this
codebase rather than hypothetical:

- **Randomness.** Content and change selection must come from a seeded generator with the seed
  recorded in the profile. The existing harnesses already do this — `dc59_commit_benchmark.rs` uses a
  `SplitMix64` copied deliberately from reviewed material rather than an invented generator, and
  documents why the copy exists. The builder follows that precedent rather than inventing a third.
- **Time and identity.** Seals carry signatures; RFC 123 put an optional `message` inside object
  identity. Anything a seal commits to that varies per run makes the corpus nondeterministic. The
  builder must fix every such input in the profile, and the determinism test is what catches the one
  that was missed.

**Cross-platform determinism is not required and must not be claimed.** The existing harnesses are
already Linux-scoped for their memory pass (`/proc`-based, skipped cleanly elsewhere), and comparing
timings across platforms was never meaningful. What is required is that a corpus is identical to
itself on the machine measuring it.

## 5a. AMENDED 2026-09-06 — §5 and §7 were in direct conflict, and §5 is the one that yields

**Increment 1 found a contradiction between two of this RFC's own requirements.** §5 demands that two
builds from one profile produce "identical sealed head, identical state roots at every depth,
identical object set". §7 demands the builder drive `commit`/`seal` through the ordinary CLI. **Both
cannot hold**, and the reason is structural rather than incidental:

- A `NodeId` is *"a uniform 256-bit draw from the OS CSPRNG"* — `crates/prikk-store/src/node_id_gen.rs`'s
  own module doc — deliberately **not** derived from path, content, position, timestamp or baseline
  state, because it must survive rename, edit, chmod and binary replacement, and it is part of the
  text `span_id` preimage.
- `commit_worktree_changes_signed` — the public, CLI-reachable entry — hardcodes
  `NodeIdGenerator::production()`. The injectable form, `commit_worktree_changes_with_generator`, is
  **`pub(crate)` and documented test-only**.
- `node_id` sits in seven operation payload fields, so it is inside object identity.

So a CLI-built corpus mints fresh random node ids on every build, and every object id, state root and
sealed head differs. **Verified independently by the architect at the cited lines, not accepted from
the report.** *(Nothing else varies: Ed25519 signing here is deterministic — `getrandom` appears only
in `generate_seed()` — and `RefUpdatePayload.created_at` is pinned to `0`. The node id is the whole
of it.)*

### 5a.1 Two ways out, both refused

**Widening `commit_worktree_changes_with_generator` to public — refused.** It would let the builder
inject a deterministic generator, at the cost of promoting a documented test-only seam into a shipped
crate's public API, and of building the corpus through a path no real command sequence uses. **That is
exactly what §7 refuses**, and §7's reasoning is unchanged by this problem.

**A CLI-level deterministic-entropy override — refused, and this one on security grounds.** A flag or
environment variable whose only purpose is to make identity minting predictable is a hazard out of
proportion to a measurement convenience: node-id collision-avoidance is a fail-closed invariant of the
minting authority, and an override that escaped into ordinary use could mint colliding identities
across repositories. **The owner's standing instruction is that security be protected as strongly as
possible and reasonable**; shipping a predictable-identity switch to every user so that an internal
benchmark can diff two heads is not that. If a future round finds an independent reason to want one,
it is its own design with its own review — not a corner of this RFC.

### 5a.2 RULED: determinism is a property of the builder's actions, not of the repository's bytes

**§5's requirement is replaced, not relaxed away.** What must be identical between two builds from one
profile is **the sequence of operations the builder performs** — the same paths, created and edited
and deleted in the same order, with the same content and sizes, at the same depths. The builder emits
a manifest of exactly that, and **the determinism test compares manifests**, not sealed heads.

**Why this is sufficient for everything the corpus exists to do.** Cost follows shape: replay depth,
tree size, operation counts, file bytes. None of the quantities §1 and §6 care about reads a node id
or an object id as anything but an opaque key. Two structurally identical corpora are as comparable
for cost purposes as one corpus measured twice.

**The limit, stated rather than discovered later: any measurement whose result depends on the
*values* of ids is not comparable across builds** — anything sensitive to ordering within an
id-keyed structure, or to hash distribution. **A measurement of that kind must run against a single
built corpus, twice, and must say so.** No such measurement is currently planned; this exists so the
first person who wants one recognizes the constraint instead of trusting a cross-build comparison.

**§5's other requirements stand unchanged**: a seeded generator with the seed in the profile, the
determinism test running in the ordinary suite rather than joining the `#[ignore]`d instruments, and
no claim of cross-platform determinism.

## 6. Depth — a target this RFC deliberately does not fix

The external architect asked for *"a synthetic-but-realistic prikk history of a few thousand sealed
blocks"*. That is the right order of magnitude and the wrong kind of number to write into a design.

**What can be justified now** is the *floor* and the reason for it. Cadence tuning is one of the two
things the corpus exists for, and the quantity being tuned against is `REANCHOR_BOUND = 64`. A corpus
shallower than a few multiples of 64 cannot show a depth-versus-storage curve at all — it shows a
handful of points. **Thirty-two reanchor intervals, i.e. 2,048 sealed blocks, is the floor** at which
the curve has shape.

**What cannot be justified now is the target**, because nobody has measured what building 2,048
sealed blocks costs. That is not a small unknown: `dc92_lineage_replay_benchmark.rs` grew a
repository to **160** blocks and its own module doc calls that expensive, at a time when `verify` was
believed cubic. `verify` is linear now, and per-seal cost has its own curve from that same harness —
but **neither figure has been extended to 2,048, and extrapolating a build cost is exactly the
species of claim RFC 133 §2 exists to warn against.**

**Therefore §9 increment 2 measures the build-cost curve first and confirms or revises the depth
target against it**, and this RFC states the floor rather than pretending to a target. If 2,048
proves to cost hours, that is a finding about this project's seal cost — a result worth having, not
an obstacle to route around — and it belongs in RFC 133's evidence table rather than being absorbed
silently into a smaller corpus.

## 7. Where it lives, and the two things it must not become

**Placement follows an existing mould rather than inventing one.** `tools/benchmarks` is already a
workspace member with `publish = false`, excluded from `default-members`, and described in its own
manifest as *"Repository-internal Prikk performance benchmarks. Not part of the shipped product; not
built by default."* The corpus builder has the same character and takes the same treatment: a
workspace member under `tools/`, `publish = false`, out of `default-members`, shipped to nobody.

**It must be usable from both places measurement already happens** — `tools/benchmarks`' criterion
benches and the `#[ignore]`d integration harnesses under `crates/prikk-cli/tests/` — which means the
builder is a library with a thin binary over it, not a binary with logic inside it.

**The builder drives the same surface a user drives.** The existing harnesses invoke the `prikk`
binary through `tests/support/mod.rs`'s `init`/`commit`/`seal` helpers, and the corpus builder must
too, rather than reaching into `prikk-store` to construct history faster. A corpus assembled through
a private path would be a repository no real sequence of commands could produce, and measuring it
would answer a question nobody asked. **This makes the build slow, and that cost is accepted
deliberately** — it is also why §6's build-cost curve is a first-class deliverable rather than an
incidental one.

**Two things it must not become:**

1. **Not a CI job.** The corpus is a deliberately-invoked instrument, exactly as `dc59` and `dc92`
   are `#[ignore]`d instruments. Building thousands of sealed blocks on every push would dominate CI
   and protect nothing that a gate could not protect more cheaply. **Ruled here, not deferred.** The
   one exception is §5's determinism test, which is shallow by construction and belongs in the
   ordinary suite.
2. **Not a correctness fixture.** The corpus is for cost. Correctness fixtures in this project are
   built to exercise a boundary and are read by assertions; a corpus is built to be big and is read
   by a clock. Using it as a correctness fixture would make every future measurement hostage to an
   unrelated assertion, and would eventually make someone reluctant to regenerate it.

## 8. What this RFC does not decide

**It does not rule RFC 133 §6.** Whether memory independence from repository size becomes a stated
requirement is the owner's, is unaffected by this RFC, and is not made easier or harder by it. If §6
is ruled yes, the corpus is available to hold the evidence; if no, the corpus is unaffected.

**It does not choose RFC 136's snapshot design, cadence, or format.** It builds the instrument that
those decisions were held pending. RFC 136 §7's Option A ruling stands as the owner made it; §7.2's
content-check precondition is unaffected and remains a precondition.

**It does not decide which measurements get standing gates.** Building an instrument is not a claim
that any particular property deserves permanent protection. Each such claim is its own proposal with
its own cost, and §2's table is deliberately honest that this project has exactly two.

**It does not fix `AUD-01` or `AUD-02`.** RFC 133 §7 keeps those; this RFC only makes it possible to
measure them rather than read them.

## 9. Increments

**Handoff issued for increment 1, 2026-09-06:**
`rfcs/handoffs/139-measurement-corpus/profile-format-handoff-v1.md`. Issued after the move to
`rfcs/accepted/`, not before it.

**One design decision was made in the handoff rather than here, and it corrects this RFC's own
framing.** §4 speaks of "files-changed-per-commit" and cites RFC 136 §9.1's `--name-only` command.
That is insufficient for a builder: **prikk's operation kinds do not cost the same**
(`OperationKind` — `CreateFile`, `DeleteNode`, `EditText`, `RenamePath`, `ChangePerm`,
`CreateSymlink`, `ReplaceBinary`), and a profile without the operation-kind mix would force the
builder to guess it. The handoff therefore specifies `--name-status`, and adds a file-size
distribution from `git ls-tree -r -l` because RFC 133 §2 measured that commit cost follows **bytes,
not paths**. **§4's minimum stands; it was a floor, not a specification.**

**Increment 1 — the profile format, and prikk's own profile.**
Define the profile file's shape and the provenance fields §4 requires. Re-derive prikk's own profile
at a stated revision, using the RFC 136 §9.1 command recorded verbatim. Cheap: the extraction is
already proven, and this increment mostly writes down what §9.1 did in a form a builder can read.

**Increment 1 — DELIVERED and ACCEPTED 2026-09-06** (`735eee1`). `tools/corpus`, the profile format,
the pure extractor, and `profiles/prikk-self.toml` at 600 commits / 875 distinct paths. **The
provenance recipe was re-run by the architect and re-derived the committed profile byte-identically.**
It also produced §5a's finding, which is worth more than the increment.

**Increment 2 — the builder, determinism, and the build-cost curve.**
The library-plus-binary of §7, driving the CLI through the existing `tests/support` surface. The
determinism test of §5, shallow and in the ordinary suite. Then the build-cost curve of §6 —
wall-clock and peak RSS against sealed depth, out to the depth the curve itself says is reachable —
and **a stated depth target confirmed or revised against it.** This increment ends with a number, and
the number may be unwelcome.

**Handoff issued for increment 2, 2026-09-07:**
`rfcs/handoffs/139-measurement-corpus/builder-and-build-cost-handoff-v1.md`.

**It resolves two obstacles in §7's own wording, verified rather than assumed.** §7 says the builder
must drive the CLI "through the existing `tests/support` surface". **It cannot**: that module is
test-only inside `crates/prikk-cli/tests/`, not a library, and it locates the binary with
`env!("CARGO_BIN_EXE_prikk")`, which Cargo defines only for the test targets of the crate declaring
that binary. **§7's actual requirement — drive the real CLI, not `prikk-store` — is unaffected**; the
`tests/support` reference was precedent, not a dependency. The executor takes the binary path
explicitly and **records its identity**, which §4's provenance discipline requires anyway: a corpus
built by one binary is not comparable to one built by another.

**And §5a's ruling turns out to buy more than it was made for.** Because determinism is a property of
the builder's **action manifest**, and a manifest can be computed without executing anything, the
determinism test needs **no binary and no repository** — which is what makes §5's "in the ordinary
suite rather than joining the `#[ignore]`d instruments" achievable at all. **Separating planning from
execution is therefore structural, not stylistic**, and the handoff says so.

### Increment 2 — DELIVERED and ACCEPTED 2026-09-07 (`329715b`), and §6's floor is RULED unreachable

**§6 deliberately refused to fix a depth target before the build cost was measured. That refusal
paid.** Measured: per-seal cost is **linear in history depth**, so cumulative build cost is
**quadratic** — power-law exponent **2.03**, re-derived independently by the architect. Depth 2,048
extrapolates to **23 to 101 hours**. Full figures and the RFC 111 distinction: **RFC 133 §5b**.

**RULED: the 2,048 floor stands as the design's requirement and is unreachable in practice today.
Those are two different statements and both are recorded.**

- **The floor is not revised away.** §6 chose 2,048 as the depth at which a depth-versus-storage curve
  has *shape* — 32 `REANCHOR_BOUND` intervals. Nothing measured changes what the curve needs.
- **The implementing round's practical target of 256 is accepted for increment 3**, exercising the
  authority §6 gave it. **It is 4 intervals, not 32** — the round said so plainly rather than
  softening it, and that honesty is why it is accepted.
- **So increment 3 must state what 4 intervals can and cannot support.** RFC 136 §9 item 1 (checkout
  cost at realistic depth) is answerable at 256. **Cadence tuning against `REANCHOR_BOUND` is not** —
  four points show direction, not shape. **Increment 3 must not present a 256-deep result as
  cadence-tuning evidence.**

**The substantive consequence, and it is new:** **reducing `seal`'s per-block object reads is now a
prerequisite for this RFC's own stated purpose**, not a separate performance nicety. The corpus did
not fail to reach its floor — **it measured the thing that blocks it**, which is what an instrument is
for. RFC 133 §5b.2 carries that dependency.

**Two things this increment establishes beyond its own scope.** §5a's ruling held up exactly as
designed — determinism compares action manifests, and the round's own control asserts sealed heads
**differ**, putting §5a into the test suite rather than leaving a future reader to assume they should
match. And the planner/executor split the handoff required made the determinism test runnable with **no
binary and no repository**: verified, it runs in the ordinary suite with **0 ignored**.

**Increment 3 — the two measurements the corpus was held for.**
RFC 136 §9 item 1 (checkout cost at realistic depth — the quantity the reviewer confirmed has no
honest git proxy) and item 2 (baseline reconstruction on the **merge** path, which DC-64 never
covered). Both reported into RFC 136, which is where the decision they inform lives.

**Increment 4 — the second profile.**
A structurally different project per §4, chosen for the opposite change-concentration property, so
§9.1's limit 2 stops bounding every conclusion this project draws.

**Ordering is not negotiable between 1 and 2**, since a builder needs a format to build from; 3 and 4
are independent of each other and either may go first.

**Three things increment 1 hands to increment 2, all settled input:**

1. **§5a's ruling** — the determinism test compares the builder's **action manifest**, not sealed
   heads. The builder must emit one.
2. **The `--name-status` letter → `OperationKind` mapping is increment 2's own decision.** The profile
   records git's letter vocabulary (`added`/`modified`/`deleted`/`renamed`/`copied`/`type_changed`)
   because `M` cannot be resolved to `EditText` / `ReplaceBinary` / `ChangePerm` without text-vs-binary
   and content-vs-mode information `--name-status` does not carry. **Increment 1 was right not to
   invent it.**
3. **The profile carries distributions, not a sequence.** Choosing an application order is increment
   2's design work, not a gap in the format — RFC 139 §3's "a profile plus a builder, not a
   recording".

## 10. Scope

**In:** the profile format and its provenance requirements; the builder and its determinism
guarantee; the build-cost curve and the depth target derived from it; the placement decision; the two
measurements of increment 3; a second profile.

**Out:** any behaviour change; any object-model, format, or schema change; any new standing gate; any
fix to `AUD-01`/`AUD-02`; RFC 136's snapshot format, cadence, or content check; RFC 133 §6's ruling.

## 11. Risks

**The corpus goes stale between uses, exactly as the existing harnesses can.** `dc59`'s module doc
already states this and accepts it deliberately rather than treating it as an oversight. The same
acceptance applies, with one difference in this corpus's favour: a profile is a small text file that
a reader can check against its recorded extraction command, whereas a harness's realism is buried in
its code.

**The build cost may make the useful depth unreachable in practice.** §6 treats this as a finding
rather than a failure, but it is the most likely way this RFC disappoints. If it happens, the honest
result is a recorded seal-cost curve and a shallower corpus with its limits stated — not a corpus
that quietly measures 200 blocks while the design says 2,048.

**A realistic corpus makes some current claims look worse.** The purpose of measuring at realistic
depth is to learn things the synthetic harnesses could not show, and there is no reason to assume the
news is good. That is the point, and RFC 133 §2 is this project's precedent for publishing the
correction rather than the comfortable version.

**Related:** RFC 133 (what performance costs exist and what evidence holds them — the discipline this
RFC supplies the instrument for), RFC 136 (block aggregation payoff — held pending this corpus),
RFC 111 (the two cost gates that are this project's only standing performance evidence), DC-59/DC-62/
DC-64/DC-69/DC-92 (the harnesses whose method this builds on and whose incomparability it retires),
RFC 114 (format stability — why a stored corpus was refused), RFC 118 (derive, never transcribe — the
same principle applied to a fixture).
