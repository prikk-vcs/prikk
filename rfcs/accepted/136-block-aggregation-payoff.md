# RFC 136 — The block aggregation payoff: what sealing a block should make cheap

**Status.** **ACCEPTED by the project owner 2026-09-04**, the same day it was opened at their
instruction, in answer to their own question: the block concept came from the fact that patch-based
version control is forced into heavy calculation, and the intent was *"to make it by far more efficient
by aggregating multiple patches into a single patch as block when a cycle of development on some theme
is finished."*

**Re-reviewed 2026-09-13: nothing external blocks this RFC.** The reviewer answered (§9.2), the owner ruled Option A (§7), §9 items 1–2 are measured (§9.3: cost grows as depth^1.45, not with tree size). What remains is *design* — the path-to-Blob snapshot format (§5), the checkpoint cadence (§9.2's `REANCHOR_BOUND` question), and §9 item 4 measured under a candidate policy — and it is the architect's to write. **SCHEDULED as 0.43's theme, "sealed snapshots" — owner approved 2026-09-13.** The design is §10.

**What the acceptance covers, stated because a bare acceptance is scope-ambiguous.** It accepts the
problem record, the evidence, and the shape of the question — the same reading RFC 133's acceptance
carried. **It does not answer §7**, and §7 remains the one thing outstanding. Following RFC 101/102's
precedent, **acceptance clears §9's measurements only**: no object-model change, no snapshot policy,
no composition definition, no handoff.

**§9.1 was measured under that clearance on 2026-09-04, and it eliminates one option.** Option B's
collapse ratio over this project's own 600-commit history is **1.16-1.25x at realistic block sizes**,
and the measured figure is an *upper bound* on what composition could achieve. **The architect
recommends against Option B**; Options A and C are untouched by it, because neither depends on
collapse. §7's question is unchanged and still the owner's.

**§7 RULED by the project owner 2026-09-04: Option A — the acceleration travels.** Their words:
*"If you mean it shared among the same projects of the remote, Yes. Not 'purely local'. I prefer that
blocks are shared in a strict format where they belong to the same context."* Option C is refused;
Option B was already recommended against on §9.1's measurement. **See §7.1 for the mechanism this
ruling meets, which is not uniform across prikk's two transport paths, and §7.2 for the check it
obliges that does not exist today.**

**§9.2 AMENDMENT 2026-09-06 — the external architect answered, and the framing changed.** The residual cost is **replay depth**, not materialization generally; A/C/deltas are one mechanism (a checkpoint), and B was never on that axis. §8.3's principle is replaced by **entailment**; §6.1's dilemma has a third position (**materialization is not certification**, with a derivation gate); §5.3's 211 MiB was an artifact of an every-seal strawman — at reanchor cadence it is **3.4 MiB**. **Both reviewers independently reached the same hold: build the measurement corpus before committing storage and a schema-touching field.**

**Still not ruled:** the snapshot format, the checkpoint cadence work, and §9 items 1, 2 and 4, none of which are measured. No handoff is issued.

**Author-review independence.** The architect wrote this RFC and is also its only reviewer, the
standing gap recorded on every architect-authored design in this project. Compensated at
implementation review, not here.

**Tracks.** Cost, and the object model that carries it. **No behaviour change is proposed in this
document.**

---

## 1. Where the concept came from, and why it needs a record

The block was not a storage convenience. It was the answer to the classical patch-VCS cost problem:
if history is a bag of patches, then reasoning about a merge means reasoning about relations among all
of them, and the work grows with history rather than with the change being made. Sealing a run of
patches into one unit was meant to cut that.

That intent has never been written down in this repository. `docs/src/reference/data-model.md`
describes what a Block *is*; nothing describes what sealing one is *for*. The result is that half the
mechanism was built, the other half was left as a field nobody writes, and no document records that
the second half is missing. This RFC is that record.

## 2. What a Block is, as built

`crates/prikk-object/src/payload/block.rs:49`:

| Field | Meaning |
|---|---|
| `parent_block_ids` | sorted; 0 for `Root`, 1 for `Normal`, 2 for `Merge` |
| `kind` | `Root` / `Normal` / `Merge`; `Repair` / `Import` are minted but unauthorized |
| `patch_ids` | the patches this block seals, in canonical block order |
| `state_merkle_root` | commits to the complete replay-derived live-node set in canonical path order |
| `snapshot_blob_ref` | **optional full snapshot of that state** |
| `mainline_parent_id`, `merge_baseline_block_id` | `Merge` only (DC-75) |

`prikk commit` appends patches to the active WAL; `prikk seal` turns everything accumulated since the
last seal into one block. The seal boundary is already exactly the owner's *"a cycle of development on
some theme is finished."*

## 3. What the block already buys — the half that works

**3.1 Merge work is bounded by the block window, not by history.**
`crates/prikk-store/src/merge_evidence.rs:293-315` derives a side's candidate sequence as
`ancestors(target) \ ancestors(baseline)` — the blocks strictly between the sealed baseline and the
target. Confluence then checks cross-pairs between the two candidate sequences
(`docs/src/reference/patch-algebra.md`, "Flat Confluence"). The work is **O(L x R) in patches since
the sealed baseline**, not in history length.

This is the escape from the classical cost, and it is real. The sealed block is what makes "since the
baseline" a bounded, cheap-to-name set instead of the whole history.

**3.2 Whole-tree state comparison is constant time.** `state_merkle_root` commits to the entire
live-node set, so two blocks' states compare in O(1) with no replay at all.

**3.3 A merge records the scope of its own proof.** A `Merge` block carries
`merge_baseline_block_id`, the block confluence was proven against — which `verify` re-derives rather
than trusts.

## 4. What it does not buy — and the exact evidence

**4.1 A block names its patches; it does not compose them.** `patch_ids` is a list. Every patch stays
individually addressed and individually replayed. There is no "block as a single patch" anywhere in
the object model.

**4.2 `snapshot_blob_ref` is never written.** All three paths that create a block hard-code `None`:

- `crates/prikk-cli/src/seal.rs:174` — `prikk seal`
- `crates/prikk-store/src/seal_from_accepted.rs:224` — sync receive
- `crates/prikk-store/src/merge_execute.rs:175` — `prikk merge`

`SnapshotManifest::encode` (`crates/prikk-store/src/snapshot.rs:74`) is called from **test code only**.
No production path has ever produced a snapshot.

**4.3 The consequence.** State at a block is derived by replaying its lineage — O(patches from
genesis). `merge_evidence.rs:50-51` shows the asymmetry in a single place: *classification* is bounded
by the window, but establishing the **baseline state** that window is measured against is a full
`replay_derived_state` from the lineage horizon.

**4.4 Two caches soften this, and neither of them is the block.**

| Cache | Scope | Persistence |
|---|---|---|
| `LineageStateMemo` (DC-92, `block_state.rs:83-131`) | one process invocation; takes a whole `verify` from O(N^2) to O(N) | never persisted |
| incremental baseline cache (DC-64, `lifecycle_cache/incremental.rs`) | **commit path only**; re-anchors to a full replay every 64 steps | persisted, rebuildable, never authoritative |

This matches the RFC 133 measurement directly: incremental commit memory flat at 16.3 MiB, genesis
commit 1:1 with total worktree bytes.

## 5. What is already built on the read side — and the one place it is not fit for purpose

The read half of the snapshot mechanism is complete and waiting:

| Site | What it already does |
|---|---|
| `checkout.rs:219-227` | classifies a block with a snapshot as `RequiresSnapshotMaterialization` instead of `RequiresPatchEngine` |
| `checkout.rs:88-118` | `prepare_snapshot_checkout_plan` decodes the manifest and plans the materialization |
| `verify.rs:1565` | checks the referenced Blob exists |
| `bundle.rs:801` | transports it as a reachable blob |
| `patch_replay/read.rs:116` | reads it as a replay baseline |

**But the v1 snapshot format inlines file contents.** `SnapshotEntry` is `{ path: RepoPath, bytes:
Vec<u8> }` (`snapshot.rs:14-19`) — full content, not a Blob reference. A snapshot written at every
seal would therefore store a **second complete copy of the worktree per block**, so N sealed blocks
cost N x worktree bytes on top of the blobs already stored.

That is disqualifying as written, and the fix is already implied by the model: the state root's own
leaves bind *"the exact repository path, nonzero NodeId, node kind, normalized mode, and either the
file Blob ObjectId or opaque UTF-8 symlink target"* (`data-model.md`). A path-to-Blob-id manifest is
the state entry set the Merkle root already commits to, and it is small. **The existing snapshot
format is a worktree materialization aid, not a history-scale artifact, and this RFC should not
pretend otherwise.**

## 6. The constraint that shapes every option

`docs/src/reference/data-model.md` already states the governing rule, in shipped documentation:

> Snapshots and caches may be used only as checked auxiliary data; they cannot override replay.

This has a consequence worth stating plainly before any option is weighed: **verification cannot be
accelerated by any of this.** `verify` must still replay a block's patches and compare the result
against the recorded `state_merkle_root`; a snapshot that hashes to the same root proves the snapshot
matches the root, never that replaying the patches produces it. The accelerable surfaces are
**checkout** and **baseline reconstruction** (commit, merge), not verification.

Any option that appears to speed up `verify` has violated this rule and is wrong.

## 7. The option space — and the question for the owner

Three shapes are available. They are not variations on one design; they differ in what is stored, what
travels, and what identity depends on.

### Option A — full-tree snapshot in the block — **RULED 2026-09-04, the owner's choice**

Write `snapshot_blob_ref` at seal, as a path-to-Blob-id manifest (§5, not the v1 inline format).

- **Buys:** O(tree) checkout at any sealed block with no replay; O(tree) baseline reconstruction.
- **Costs:** a new snapshot format; a policy for *when* to snapshot (every seal is wasteful, never is
  useless); storage proportional to snapshot frequency.
- **Travels in bundles:** yes — a fresh receiver gets the acceleration immediately.
- **Identity:** the field is inside the canonical encoding (`block.rs:282`), so the block id depends
  on it. See §8.

### Option B — a composed net patch per block

Store the block's **net effect** as one operation sequence, alongside (not instead of) `patch_ids`.

- **Closest to the owner's original words.** "Aggregating multiple patches into a single patch as
  block" is literally this.
- **Buys:** applying one block becomes O(net change) rather than O(all operations in the block) —
  the win grows with how often a block's patches touch the same files, which is exactly what a
  finished theme looks like.
- **Costs:** composition must be defined and proven equal to sequential replay, over the full
  operation set including `EditText` — and RFC 134 is this project's own evidence that composition
  over text spans is where the hard cases live. No field exists.
- **MEASURED 2026-09-04 (§9.1): the payoff is 1.16-1.25x at realistic block sizes, and that figure is
  a ceiling.** **Architect recommends against.** Not refused — the owner rules — but the proof cost is
  not proportionate to a 20% reduction in operations replayed.
- **Travels in bundles:** yes.
- **Identity:** same question as A.

### Option C — a repository-local snapshot cache, outside the object model — **REFUSED 2026-09-04**

Keyed by block id, in the DC-64 mould: persisted, rebuildable, never authoritative, no object-model
change at all.

- **Buys:** the same checkout and baseline-reconstruction wins, locally.
- **Costs:** does not travel — a fresh clone replays once before it benefits. Cache-invalidation
  surface, which DC-64 already has an answer for (`REANCHOR_BOUND`).
- **Identity:** **untouched.** Nothing enters a signed object.

**The question, ANSWERED 2026-09-04:** *is the acceleration something a repository should be able to
hand to another repository (A or B), or is it purely local (C)?* **The owner ruled that it travels.**
With B measured out at §9.1, **Option A stands.** §7.1 records what "travels" actually means across
prikk's two transport paths; §7.2 records the check that ruling now obliges.

## 7.1 What "travels" means — it is not uniform, and the ruling meets only one of two paths

**Blocks travel verbatim through `bundle`, and do not travel at all through `sync`.** This was not in
this RFC when §7 was written and it is load-bearing for the ruling.

| Path | Do blocks travel? | Would a snapshot travel? |
|---|---|---|
| `bundle export` / `import_bundle` (DC-78) | **Yes, verbatim.** Import *"writes exactly those objects plus one `received` pointer"* (`bundle.rs:1-10`), and closure validation already names *"a Block's own `snapshot_blob_ref`"* among the blobs a bundle must carry (`bundle.rs:58-62`) | **Yes** |
| `sync` (RFC 115/116, `seal_from_accepted.rs`) | **No.** The receiver reconstructs *"a local equivalent of that block — the same patches, in the same order, on the receiver's own parent, sealed under the receiver's own maintainer key"* | **No** |

`seal_from_accepted.rs:1-13` states the reason, and it is a deliberate ruling rather than an
omission: *"The resulting block id will not equal the claim's `block_id`, and that is correct, not a
failure... identity lives at the patch level, not the block level."*

**The owner's instinct is already prikk's design, and more strongly than the ruling assumed.** *"Blocks
are shared in a strict format where they belong to the same context"* — a block is so context-bound
that it does not survive a move at all. Sync re-seals under the receiver's own key precisely because a
block belongs to its own repository's context.

**The consequence, which the architect judges is not a gap.** Option A delivers travelling
acceleration over `bundle` — the whole-history path, where a receiver arriving with nothing is exactly
who cannot afford to replay everything. A `sync` receiver already holds the lineage and seals its own
block, so it can build its own snapshot locally at that moment for free. Making a snapshot travel over
`sync` would mean transporting a block verbatim, which would break the own-key sealing property
RFC 115 §2.4-§2.7 established on purpose. **That trade is not worth reopening, and this RFC does not
propose reopening it.** Stated here so the ruling is not read as promising more than the mechanism
delivers.

## 7.2 The check the ruling obliges, which does not exist today

§6 admits snapshots only as *"checked auxiliary data"*. **Nothing currently checks a snapshot's
content against anything.**

- The state root is computed from **replay** and compared to the block's own recorded value
  (`block_state.rs:244-245`, `:534-535`).
- A snapshot is only checked to **exist** (`verify.rs:1565`, `ensure_object_exists`); its content is
  never hashed, never compared to `state_merkle_root`.
- Yet `prepare_snapshot_checkout_plan` (`checkout.rs:88-118`) would materialize a worktree straight
  from it.

Local-only (Option C) could have leaned on the cache having been built here, from verified replay.
**A travelling snapshot cannot: it arrives from a remote.** Under the owner's ruling, a bundle can
carry a snapshot whose content no code has ever checked against the state it claims to represent, and
checkout would write it to disk.

**Therefore: the content check is a precondition of Option A, not a later refinement.** It must land
before any path writes a snapshot, and before any path trusts an imported one. This is the same
ordering argument DC-90 used for its own gate — a boundary added afterwards documents what happened
instead of constraining it.

## 8. The identity question — and why it is smaller than it first appears

The obvious objection to A and B is that they put **derived** data inside an **identity-bearing**
object: two repositories with identical history would seal different block ids depending on whether
they chose to snapshot.

That objection is real but already priced in, and `docs/src/reference/data-model.md` says so:

> `target_block_id` itself does not survive a move: blocks diverge between repositories by design even
> when the underlying history is identical.

Block identity is **already local**. Cross-repository portability runs through `patch_set_digest` —
the digest of the block's own patch closure, `DOMAIN ‖ count ‖ sorted patch ids` — which does not
depend on `snapshot_blob_ref` and would not begin to.

So the residual question is narrower than "may identity depend on a cache". It is: **should two seals
of the same patches, differing only in a local performance choice, be distinguishable objects?** For
Option C the question does not arise. For A and B it must be answered explicitly rather than absorbed.

## 9. What must be measured before an option is chosen

RFC 133 established the measurement discipline this project now expects. None of these numbers exist
yet, and this RFC should not be implemented on any of them assumed:

1. **Checkout cost today** at a realistic history depth — the number Option A and C both claim to cut.
   Never measured.
2. **Baseline reconstruction cost** on the merge path specifically. DC-64 measured the commit path;
   merge's own `replay_derived_state` call (§4.3) was not in that scope.
3. **Compression ratio for Option B** — how many operations a real sealed block's net effect collapses
   to. If a typical block's patches touch mostly disjoint files, B buys close to nothing and its
   composition-proof cost is unjustified.
4. **Snapshot storage cost under a candidate policy**, once the format is path-to-Blob-id rather than
   inline (§5).

Item 3 is the one that can kill an option outright, and it is cheap to obtain. **It was measured on
2026-09-04 — see §9.1, which is why Option B is no longer recommended.** Items 1, 2 and 4 remain
unmeasured and no option may be implemented on any of them assumed.

## 9.1 Measured 2026-09-04 — Option B's collapse ratio, and why it is a ceiling

**Source.** This project's own last 600 non-merge commits, file lists from
`git log --pretty=format:'@@%H' --name-only --no-merges -n 600`. Mean 3.37 files changed per commit.
A block is modelled as a run of K consecutive commits — non-overlapping windows, the shape a real
"seal every finished theme" produces.

**Metric.** `sum(files touched by each patch) / count(distinct files in the window)`. This is exactly
the factor by which composition collapses *file-level* operations.

| Block size K | aggregate ratio | median | p90 | blocks collapsing **nothing** |
|---:|---:|---:|---:|---:|
| 2 | 1.05 | 1.00 | 1.67 | 77% |
| 3 | 1.11 | 1.01 | 1.50 | 50% |
| 5 | 1.16 | 1.16 | 1.67 | 17% |
| 8 | 1.22 | 1.23 | 1.64 | 4% |
| 10 | 1.25 | 1.22 | 1.62 | 2% |
| 20 | 1.35 | 1.40 | 1.75 | 0% |
| 50 | 1.55 | 1.65 | 1.92 | 0% |

**This is a ceiling, not an estimate.** For a file touched by *n* patches in a block, the composed net
effect is *at most* `n` operations (all spans disjoint) and *at least* 1 (fully overlapping). So
composed operation count is bounded below by the distinct-file count, and the true operation-granular
collapse can only be **less** than the table. Composition cannot do better than these numbers; it can
do worse.

**What it means.** At realistic block sizes a block's patches touch mostly disjoint files. Option B
would buy roughly a **20% reduction in operations replayed** while requiring composition to be defined
and proven equal to sequential replay across the full operation set — including `EditText`, where
RFC 134 is this project's own evidence of how hard that is. At K=3, half of all blocks would collapse
nothing at all. **The proof cost is not proportionate to the payoff, and this is the option closest to
the original wording — which is why the number had to be taken before ruling rather than after.**

**Three limits on this measurement, stated so it is not over-read.**

1. **It is prikk's git history, not prikk sealed history**, because no prikk repository with realistic
   development history exists to measure. Git commits stand in for patches. Prikk's own granularity is
   the same or finer, since `commit` queues several patches per seal.
2. **It is one project with a disciplined one-theme-per-commit rhythm.** A project that accumulates
   many small fixups against the same file would show a higher ratio. This bounds *prikk's* case, not
   every case.
3. **It says nothing about Options A or C.** A full-tree snapshot's payoff depends on tree size against
   history depth, not on how much a block's patches overlap. Both remain fully live, and §7's question
   is unaffected.

## 9.2 AMENDMENT 2026-09-06 — the external architect's answer, and the reframing it carries

**Received `.git-exclude/tasks/architect/audit-20260904-block-materialization/`; architect's assessment
at `.git-exclude/reviewed/external-block-materialization-review-20260906-review-v1.md`.** Every
measurement in §5.1 and §5.3 reproduced independently — §5.3 to the digit, §5.1 within rounding — and
every code claim held at the cited lines. **Nothing here overturns a number. It overturns the framing,
which is what §8.4 asked for.**

### 9.2.1 The residual cost is replay *depth*, and that collapses the option space

**§1 of this RFC named the missing payoff one level too low.** The owner's original problem — merge
reasoning growing with history — **is solved** by block windowing. What remains is that anchoring that
window needs the baseline *state*, and that is `replay_derived_state` from the lineage horizon
(`merge_evidence.rs:51`): **O(patches from genesis)**. Checkout has the same shape.

**Named as depth, §7's "three options" are not peers.** A, C and §6.4's state deltas are **one
mechanism — a checkpoint** — differing only in whether it travels (ruled: it travels) and how it is
encoded. **The decision is cadence, encoding, and the trust tier of a materialized checkpoint.**

### 9.2.2 Option B's disqualifier is replaced with a better one

**§9.1 set B aside on a 1.16-1.25x collapse ratio measured by a git-history proxy this RFC itself
distrusted.** The stronger reason: **B reduces the per-block operation constant and leaves depth
untouched.** A composed block is still block *k* of *n*. Even a hypothetical 10x collapse would not
move the O(depth) baseline reconstruction. **Same conclusion; no dependence on the proxy.** §9.1's
measurement stands as a fact and is retired as the *reason*.

### 9.2.3 §8.3's principle proves too much and is replaced by entailment

*"Block ids are already local, so derived data inside them is cheap"* has no limiting principle — it
would admit a build timestamp equally. **Replaced:**

> **A field may enter a signed, identity-bearing object iff it is a pure function of content that
> object already commits to.**

The manifest is **entailed** by `state_merkle_root` — it is that root's preimage. A timestamp is
entailed by nothing. **Id-locality is why the consequence is survivable, not why the field is
admissible.** Field 5 stays in the block id; the justification changes.

### 9.2.4 §6.1's dilemma has a third position — materialization is not certification

- **(a) self-consistency**: the manifest recomputes to the recorded root, O(entries).
- **(b) derivation**: that root follows from the patches — replay, `verify`'s alone.

(a) permits **provisional** materialization without replay; a **derivation gate** forbids `commit`,
`seal` or `accept` from an unverified-materialized base until replay upgrades it. **Both horns
defeated, and §6's rule preserved exactly.**

**This is stated as a precondition, not a design detail**, on the reviewer's own instruction: *"the
derivation gate is not optional polish — it is the feature's reason to be safe."* The failure it
prevents is an unverified state laundering into signed history through a performance feature.

### 9.2.5 §5.3's storage alarm was an artifact of this RFC's own strawman

**Snapshot cadence should be reanchor cadence.** At a `REANCHOR_BOUND` reanchor a **verified full state
already exists in hand**, so persisting it is near-free *and provably derived* — discharging (b) for
the local case for free. Storage: 161 KiB x ⌈1344/64⌉ ≈ **3.4 MiB**, not 211 MiB. **§5.3's 15x blowup
was measured against an every-seal policy nobody would choose.** The real work item is promoting
`REANCHOR_BOUND` from a commit-path constant to a repository-wide checkpoint cadence.

Between checkpoints: **state-entry deltas** (§6.4), which are an overlay of manifests — O(entries
changed), no operation semantics, no `EditText`, no algebra. **B's intuition applied to state rather
than to patches, where the hard proof disappears.**

### 9.2.6 Two corrections of fact

**§8.1's sync judgement is right only conditionally.** A sync receiver self-serves **iff a local
checkpoint policy exists to fire**. Ship A transport-only and sync is genuinely unserved — the fear
§6.5 recorded and then dismissed.

**§5's "path-to-Blob-id manifest" is wrong as described.** To recompute the root the manifest must
carry the **full leaf preimage** — path, `NodeId`, kind, mode, Blob id (`state_root.rs:43-62`). The
139 B/entry sizing was right; the description was not.

### 9.2.7 One premise of theirs is false, and the fault is ours

They report `checkout.rs:69` as *"materialization is a stub"* and build a recommendation on it. **The
enum's doc comment says that; the code does not.** `materialize_snapshot_checkout` is fully implemented
(`worktree.rs:50`), writes under RFC 102 Stage 1's dirty-marking ordering, and is reachable at
`main.rs:461`. **A stale doc comment misled an external reviewer.** Their conclusion survives — no
snapshot is ever written, so none is ever materialized — but the comment is a required correction.

### 9.2.8 The hold, reached independently from both ends

§9's item 1 — checkout cost at realistic depth — remains unmeasured, and **the reviewer confirms it has
no honest git-history proxy**, unlike §5.1 and §5.3: it is a property of replay, not of a file-change
distribution. Their structural argument is *sound in shape, unquantified in magnitude*, and they say so.

**Their recommendation, which the architect endorses: build the corpus before the snapshot.** It gates
this RFC and it retires the standing methodological weakness **every** performance decision here
inherits, RFC 133 included. **That is a scheduling decision and it is the owner's.**

## 9.3 MEASURED 2026-09-07 — §9 items 1 and 2 are answered, and they turned out to be one question

**RFC 139's corpus was built for this. These are its numbers**, taken on a real
`profiles/prikk-self.toml`-shaped history rather than a git proxy — the quantity §9.2.8 recorded as
having *"no honest git-history proxy"* at all. Re-derived independently by the architect; every figure
below reproduces.

| Baseline depth | Tree files | `checkout --patch-plan` (ms) | `checkout --patch-materialize` (ms) | `merge-evidence` (ms) |
|---:|---:|---:|---:|---:|
| 32 | 29 | 1,041 | 1,045 | 1,056 |
| 69 | 42 | 3,724 | 3,683 | 3,736 |
| 138 | 91 | 9,762 | 9,798 | 10,058 |
| 207 | 129 | 16,660 | 16,659 | 16,841 |
| 276 | 175 | 23,671 | 23,650 | 23,773 |

**Item 1 is answered: checkout cost grows as depth^1.446** — superlinear in history depth, and **not**
bound by tree size, which grew only as depth^0.859 over the same range. **Cost tracks how many blocks
are walked, not how much content exists at the end of the walk.**

**Item 2 is answered, and it is the same number.** `merge-evidence` grows as **depth^1.445** — the same
exponent to three figures — and the two track each other at every checkpoint (ratios 0.986, 0.997,
0.971, 0.989, 0.996).

### 9.3.1 This is evidence *for* §7's Option A ruling, not against it

§7 ruled Option A on a premise §9 item 1 explicitly said was *"never measured"*. **It is now measured
and it is the shape Option A assumes**: superlinear in depth, on real history. The ruling stands and is
better supported than when it was made.

### 9.3.2 The correction that matters for implementing it: two implementations, not one

**The implementing round concluded that "one O(depth) replay path is walked from two call sites, and
both inherit whatever fixes it." That inheritance claim is wrong, and the round's own §2 said the
opposite correctly** — its two halves disagreed, and this is the half to keep.

**Verified at source:** these are **two separate implementations** of the same pattern, not two callers
of one function.

| | `patch_replay/read.rs::single_parent_chain` | `lifecycle_cache/replay.rs::walk_single_parent_chain` |
|---|---|---|
| reached from | `prepare_patch_replay_plan` (checkout) | `replay_derived_state` (merge evidence) |
| terminates at | parent `None` | an explicit **horizon** |
| returns | `Vec<ObjectId>` | `Vec<(ObjectId, Block)>` |
| visited set | `HashSet` | `BTreeSet` |

**So the costs match because both perform a full, uncached chain walk — not because they share code.**

**The consequence for Option A is concrete: a snapshot must be wired into both call sites
independently. Doing one leaves the other at exactly today's cost**, and nothing about fixing the first
propagates to the second. **Whoever implements Option A should treat "does this also cover
`replay_derived_state`?" as an acceptance question, not a follow-up.**

### 9.3.3 What the measurement does not cover

- **Conflict-detection cost under content overlap.** The divergence built for item 2 was **disjoint new
  files**, and its shape is *invented* — `profiles/prikk-self.toml` carries `--no-merges` in its own
  extraction command, so **no merge shape can be derived from it**. The baseline *depth* is profiled;
  the divergence is not. Same-file divergence is where conflict cost and conflicts both live, and it is
  a different question needing its own generator.
- **`RenamePath` and symlink operations**, which this codebase does not author, so the measured replay
  chain never exercises them. If Option A's snapshot format or §7.2's content check must reason about
  them, this says nothing about that cost.
- **Cadence tuning.** Five points at a 256-block practical cap is **4 `REANCHOR_BOUND` intervals, not
  32** (RFC 139 §6's floor is unreachable — RFC 133 §5b). **These numbers show direction, not curve
  shape, and must not be cited as cadence evidence.**

**Raw data and method:** `rfcs/handoffs/139-measurement-corpus/two-measurements-report-v1.md`.

## 10. Scope

**Proposed here:** the problem record, the evidence, the option space, and the §7 question.

**Not proposed here:** any change to the object model, any snapshot policy, any composition
definition, any behaviour change, and any handoff. `Repair` and `Import` block kinds stay unauthorized
and are out of scope; `verify` acceleration is excluded by §6 and is not a goal of any option.

**Related:** RFC 133 (cost and its evidence — the measurement discipline this RFC defers to),
RFC 134 (text span identity under composition — the prior art for Option B's hard case), DC-64
(incremental baseline cache — Option C's existing mould), DC-92 (lineage replay memoization),
DC-75 (merge block shape).

## 10. Sealed snapshots — the design (0.43 theme, approved 2026-09-13)

Option A, shaped by §6 (a snapshot never accelerates `verify`), §8 (block identity is already local),
§9.2.4 (materialization is not certification) and §9.2.5 (snapshot cadence is reanchor cadence).

### 10.1 The format: the state root's own leaf set, in a Blob

`PRIKK-SNAPSHOT-MANIFEST-v2\n`, then the block's state entries in canonical path order — for each:
path, NodeId, node kind, normalized mode, and the file Blob id or the symlink target — **exactly the
leaf set `state_leaf_preimage` hashes** (`state_root.rs`). No file bytes: the blobs are already stored
once. Size is ~80 bytes per tracked path (`prikk` itself: ~170 files → ~14 KiB per snapshot). Stored in a
Blob; referenced by the existing `snapshot_blob_ref` field of the Block payload, which every reader
already handles. The v1 inline format was never written by any block-creating path; its decoder stays
for fixtures and the reader dispatches on the magic.

**Self-consistency (§9.2.4 (a)) is a property of the format:** recomputing the state root from the
manifest must equal the block's `state_merkle_root`, O(entries), checked at every read. A manifest that
does not recompute is `Integrity` — a snapshot that lies about its own block is damage.

#### 10.1a What a snapshot means — ruled 2026-09-13, on the dev team's hold

The dev team held increment 1 before writing code: every reader that meets `snapshot_blob_ref` today
(`patch_replay.rs:385`, `patch_inverse.rs:112`, `bundle/preview.rs:136`, `checkout.rs`'s snapshot plan)
seeds its state from the manifest **and then applies that block's own patches** — so the v1 field meant
*the state before the block*, its parent's state. §10.1 defines the manifest as the leaf set of the
block's `state_merkle_root`, which is the state **after** the block. Measured by the team: a post-state
snapshot on a block whose patch creates `a.txt` makes every replay reader refuse with `integrity error:
CreateFile would overwrite existing path a.txt` while `verify` reports the repository clean. The
architect's "which every reader already handles" (§10.1) was true of the field and false of its
meaning; the only fixture with a snapshot on a block that has patches encodes the pre-state, and the
snapshot-checkout fixture's block has no patches at all, so neither meaning was ever tested against
the other.

**Ruling: a snapshot on block B is B's own state** — exactly the leaf set of B's `state_merkle_root`, as
§10.1 says. A reader that seeds from B's snapshot **does not apply B's patches**. Reasons, in order:
self-consistency is then checked against the block that carries the manifest, with no parent read;
`checkout --snapshot-materialize` of a ref yields the ref's state, not its parent's; and the anchor rule
of §10.3 ("start at the nearest snapshotted ancestor") means exactly this. The pre-state alternative
(option 1 of the team's report) would lock in a manifest that describes a block it is not attached to,
an always-empty genesis snapshot, and a materialization one block behind the ref; refused. Two meanings
distinguished by magic (option 2's fallback) is refused too: one field, one meaning.

**v1 is retired.** No block-creating path ever wrote it (§10.1); it exists in three test-fixture copies.
Its decoder is removed, the fixtures become one shared v2 fixture whose snapshot block **has patches**,
and a manifest with any magic other than v2 is `Integrity`, naming the magic found. Nothing shipped is
owed to it: the format-stability contract covers what a release wrote, and no release wrote a v1
manifest.

**Consequence for the increments (§10.5, revised):** the readers change *before* the writer exists, and
no commit may write a snapshot that a reader in the same tree misreads. Sealing is also not one function
today — `prikk-cli/src/seal.rs:167`, `merge/execute.rs:166`, `seal_from_accepted.rs:228` and
`rfc111_seal_simulation.rs:82` each build a Block and set the field to `None` themselves — so one seal
function in the store is the first increment, on its own.

### 10.2 The cadence: a repository-wide checkpoint every 64 blocks, and at genesis

`seal` writes a v2 snapshot when the new block's depth from the nearest snapshotted ancestor on its ref
reaches `CHECKPOINT_CADENCE = 64` — `REANCHOR_BOUND` promoted from a commit-path constant
(`lifecycle_cache/incremental.rs:36`) to one named repository-wide constant that both the reanchor and
the snapshot read — and at the first seal of a ref with no snapshotted ancestor. Not configurable: there
is no configuration surface (RFC 135 §9.1a), and a deterministic cadence has a property worth more than
a knob: **two repositories that seal the same history with the same prikk produce the same block ids**,
because the snapshot decision is a function of history, not of local choice. That answers §8's residual
question as narrowly as it can be answered: block ids remain local by design, and this feature adds no
*new* locality. `merge` and `sync accept` seal through the same path and follow the same rule.

### 10.3 What reads it, and what may not

- **Checkout** (`--patch-plan`, `--patch-materialize`) and **baseline reconstruction** (`commit`'s and
  `merge-evidence`'s `resolve_folded_worktree_baseline`) start replay at the nearest snapshotted ancestor
  instead of genesis: cost becomes O(tree + ≤64 patches) in place of depth^1.45 (§9.3). The replay from
  the snapshot is ordinary replay; nothing is skipped after the anchor.
- **`checkout --snapshot-materialize`** materializes from the manifest alone (O(tree), no replay) and
  is **provisional**: it writes a worktree marker *materialized from a snapshot; not replay-verified*.
  **The derivation gate** (§9.2.4, the reviewer's precondition): `commit`, `seal`, `merge` and `sync
  accept` refuse (`Precondition`, naming `prikk verify --ref <ref>` as the route) while that marker is
  set; a successful replay verification of the ref clears it. An unverified state cannot launder into
  signed history through a performance feature.
- **`verify` is unchanged** and must replay every block in full (§6); it additionally checks each
  snapshot's self-consistency and that its Blob exists (`verify.rs:1565` already does the latter).
- **Bundles and sync** carry snapshot blobs as reachable blobs (already, `bundle.rs:801`); a receiver
  that lacks one falls back to replay. Nothing in the exchange format changes.

### 10.3a A checkpoint changes cost, never output — ruled 2026-09-15, on the dev team's hold of increment 1b

**What the hold found.** Once the writer exists, every repository's first sealed block is a checkpoint.
Increment 1a had moved three readers to *seed from a snapshot and skip that block's patches* — the
architect's handoff said "every reader", and §10.3 names only checkout and baseline reconstruction. Live,
that changes meaning:

- **Rollback and inverse planning** reach back only to the latest checkpoint. A tip that *is* a checkpoint
  — every single-block repository, every 64th seal — refuses with *"canonical encoding error: patch
  operations must contain at least one operation"*: the inverse `PatchPayload` is built from nothing
  (`patch_inverse.rs:153-160`) and encoding refuses it (`prikk-object/src/payload/patch.rs:107`).
- **`checkout --patch-plan --format json`'s `coverage.applied_operation_kinds`**, and the prose counts
  *patches replayed* / *operations applied*, shrink to the replay window.
- **Found at review, not in the hold:** `checkout --patch-delete-plan` and `--patch-materialize-delete`
  lose every `DeleteFile` before the checkpoint. The replay fills `deleted_files` from history
  (`patch_replay/apply.rs:122`) and the anchor clears it, so a stale worktree file that earlier history
  deleted is silently kept.

§6 already forbids all three: snapshots are *checked auxiliary data* and *cannot override replay*. A
storage cadence must never move a user-visible answer.

**Rulings.**

1. **Principle.** Every output derived from state or history is byte-identical with and without
   checkpoints. Exempt, by what they are: the snapshot's own surfaces (`checkout --snapshot-*`, `verify`'s
   snapshot check); inventories that count stored objects (a bundle's object count); and block ids (the
   snapshot reference is signed, §10.2).
2. **Increment 1b is additive: no reader anchors.** `patch_replay`, `patch_inverse` and bundle preview
   replay every block from genesis and ignore `snapshot_blob_ref`. The 1a seeding goes; increment 2
   reintroduces only what ruling 5 allows. The v2 loader stays for `checkout --snapshot-*`.
3. **Rollback and inverse planning never anchor, in any increment.** Their output *is* the history they
   invert, and a snapshot carries none. Rollback's baseline is the empty state before the chain — what
   every release has shipped, since no snapshot ever existed. `rollback-preview.md`'s *"latest snapshot
   baseline"* (PR-027, written when a snapshot was imagined as a deliberate baseline) is retired. *Nothing
   to invert* becomes a `Precondition` naming the ref, not an encoding error.
4. **`coverage` keeps its meaning:** the kinds applied by a replay of the whole single-parent chain.
   stikk relies on it; their letter 006 says it *"did exactly the job you designed it for"*. The window
   meaning (the hold's option A) is refused; a v2 schema (option C) is unnecessary.
5. **Increment 2's anchor is bound by ruling 1.** A reader may start at a snapshot only for outputs that
   are functions of the tip's state: the file manifest, modes, content entries, branch switch's target
   tree. Fields that are functions of history — `deleted_files`, patch and operation counts, coverage
   kinds — are computed over the whole chain *without applying*: block payloads give patch counts, and
   decoding gives kinds and `DeleteFile` preimages. Otherwise that reader stays unanchored. Increment 2's
   control is byte-equal output of every report, prose and JSON, with and without snapshots, with the
   cost measured. If decoding the chain eats the saving, the increment returns with the numbers.
6. **Release: no cut between increments 1b and 2.** After 1b every repository holds snapshots, and
   §10.3's derivation gate for `--snapshot-materialize` is increment 2.

### 10.3b Corrections before increment 2 — 2026-09-15, checked against the code

These rulings correct §10.3 where its words do not match the code, or leave a gap. Each was checked before
the increment 2 handoff was written.

1. **`prikk verify` has no `--ref`.** Neither the CLI nor the store's `verify_repository` has a per-ref
   mode, yet §10.3 names `prikk verify --ref <ref>` as the route. So a plain `prikk verify` clears the
   provisional marker, and increment 2 adds no flag.
   - **What clears it:** a run where no condition in `verify_verdict::VERDICT_CONDITIONS` concerning
     objects, the WAL, refs or block state is true. That is the same table the exit code and the JSON
     verdict read.
   - **What does not block clearing:** a publication-trust issue alone. It says nothing about whether the
     snapshot's state equals replay.
   - **The refusal names the route:** `prikk verify`.
2. **The marker.**
   - `checkout --snapshot-materialize` writes it **before its first worktree write**, durably, beside the
     RFC 102 worktree-dirty marker. It names the ref and the snapshot's block.
   - Only (1) removes it.
   - `status` reports it, and `doctor` knows it.
3. **The derivation gate.** These refuse as `Precondition` while the marker is set:
   - `commit` and `mv`, the two ways worktree content becomes a patch;
   - `seal`, `merge`, `sync accept` and `sync seal`, which write history (§10.3's list, kept);
   - `rollback-draft --append-inverse`;
   - `branch switch`, which would move a worktree that is not replay-verified.

   Reads stay allowed, and so do `checkout --patch-*`, `tag create`, `branch create`/`close`, `trust` and
   `bundle`. None of those turns worktree content into history.
4. **A snapshot that fails validation on an anchored read** is never trusted and never silently skipped.
   The read falls back to replay from genesis and prints the `Integrity` finding on stderr, naming the
   block and `prikk verify`. Stdout is unchanged, so §10.3a ruling 1 holds.
   - `checkout --snapshot-*` has nothing to fall back to and refuses, as today.
5. **Baseline reconstruction** (`replay_derived_state`, which `commit`'s DC-64 cache, `merge-evidence`
   and the patch-algebra evidence use) builds a lifecycle state that carries **tombstones**, and a
   snapshot's leaf set carries none.
   - **The condition:** it may anchor only when the entire anchored state equals full replay's —
     tombstones and every other field, recovered over the whole chain by decoding without applying.
   - **Otherwise** it stays unanchored for this increment, and the report says why with numbers.
   - **Why it matters:** what `commit` authors must not depend on a storage cadence.

### 10.3c Anchor trust — ruled 2026-09-15, before the increment 2 handoff

**The problem.** A snapshot that passes the loader proves only that it matches its block's *signed*
`state_merkle_root`. It does not prove that replaying that block's history in this repository produces
that root; only replay proves that (§6).
- **Where an unconfirmed root comes from:** a received block (`bundle import`, `sync`, or a branch created
  at a received target) can carry a self-consistent snapshot of a root its own patches do not produce.
- **Why it matters:** state read from such an anchor is the signer's assertion. If that state reaches the
  worktree, a later `commit` authors its difference from true replay as the user's own signed patch. That
  is the laundering §10.3's derivation gate exists for. It applies to *every* anchored worktree write
  (`checkout --patch-materialize`, `--patch-materialize-delete`, `branch switch`), not only to
  `--snapshot-materialize`.

**Rulings.**
1. **Read-only reports may anchor at any snapshot that passes the loader.** They show state as its block
   signs it, and `verify` remains the certification. §10.3a ruling 1's byte-identity is required for
   histories that verify. For a history `verify` refuses, an anchored report may show the signed state
   where full replay refuses. It never writes anything, and §10.3b.4 covers a snapshot that fails the
   loader.
2. **State from a snapshot may reach the worktree or new history only through an anchor that this
   repository has replay-verified.**
   - **The record:** a durable, monotonic set of replay-verified block ids. Blocks are immutable, so a
     verified root stays verified.
   - **Who writes it:** the operations that already verify lineage roots. `seal_block`'s derivation
     (`verify_v2_lineage_roots`) covers `seal`, `merge` and `sync seal`, and `verify` covers everything.
   - **Its standing:** rebuildable and never authoritative. If it is missing, replay runs in full.
   - **The rule:** anchored worktree writes use only anchors in that set, and otherwise replay from genesis.
   - **`--snapshot-materialize`** of a block in the set needs no provisional marker, because its content
     is what replay gives. Otherwise the marker and gate of §10.3b apply.
3. **Baseline reconstruction** (`replay_derived_state`) builds a `prikk-replay` `NodeLifecycleState`
   whose `latest_tombstone_by_id` and `seen_ids` are history, and `create_node` consults both
   (`node_lifecycle/mutation.rs:37-51`).
   - **The conditions:** it may anchor only at a verified anchor (ruling 2), and only when the whole
     anchored state, both fields included, equals full replay's.
   - **Otherwise** `commit` keeps DC-64's incremental cache, which already reanchors at
     `CHECKPOINT_CADENCE`.

**2a DONE 2026-09-15** (`14ac8b3f`, `f5e8dc1c`, `8b303094`, `2d96196b`; reviewed
`.git-exclude/reviewed/rfc136-increment-2a-review-v1.md`).
- **What landed:** the provisional marker with compare-and-remove clearing; the gate, one check with a
  source-scan guard over eight commands; and anchored read-only reports whose history fields are decoded
  over the whole chain.
- **Byte identity** was confirmed by the architect on the real binaries against the pre-anchoring build.
- **Cost:** `--patch-plan` takes 0.28× at depth 256, with peak memory +12 %.
- **Rulings at review:**
  - `lifecycle-cache-divergence` does not block clearing. It is a cache fault; verify's own replay is the
    authority.
  - The damaged-snapshot test-support helper and its binary control come in 2b.
  - Checkout peak memory is measured again in increment 3.
- **2b handoff:** `136-block-aggregation-payoff/sealed-snapshots-increment-2b-handoff-v1.md`.

**2b DONE 2026-09-15** (`512a409b`, `eeef5b61`, `38222a91`, `1a492d72`; reviewed
`.git-exclude/reviewed/rfc136-increment-2b-review-v1.md`).
- **What landed:** the replay-verified record, written only by `seal_block` and `verify`; worktree writes
  anchor only at recorded blocks; `--snapshot-materialize` of a recorded block writes no marker.
- **Measured:** 0.31× at depth 256 for `--patch-materialize` and `branch switch`. Worktrees are
  byte-identical on the binaries.
- **Carried into the next round:** gate the lying-snapshot test fixture, and mark the extended report
  structs `#[non_exhaustive]`.
- **Received history cannot reach a worktree write today** (`branch create --from remotes/…` and
  `checkout --ref remotes/…` refuse). When a repository-complete or relay artifact changes that, the
  lying-anchor control must be extended to received refs before it ships.

**Order:**
1. the checkout-refusal fix (stikk letter 012);
2. increment 3: measurement, docs, CHANGELOG, done;
3. the 0.43.0 prep.

**2c follows 0.43.0** unless increment 3's measurement shows baseline reconstruction needs it.

**Increment 2 is split.** §10.5's increment 2 becomes:
- **2a:** the marker and derivation gate (§10.3b.1–3), plus anchored **read-only** reports (ruling 1)
  with §10.3a ruling 5's whole-chain history fields and §10.3b.4's fallback;
- **2b:** the verified-block record, plus anchored worktree writes (ruling 2);
- **2c:** baseline reconstruction (ruling 3), or a measured reason to leave it to DC-64.

**No release cut before 2b** (§10.3a ruling 6, extended). 2c may follow the 0.43.0 cut if it measures as
unnecessary.

### 10.4 Measurements that close §9 item 4, taken on RFC 139's corpus

Storage per snapshot and per repository at cadence 64 on `profiles/prikk-self.toml`; checkout and
merge-evidence at the five depths of §9.3 before and after (expected: flat past 64); the derivation gate
exercised: materialize from a snapshot, `commit` refuses, `verify`, `commit` proceeds. The instrument
writes under `.git-exclude/measurements/` (RFC 133's rule).

### 10.5 Increments — revised 2026-09-13 (§10.1a); the original three-step plan is superseded

0. **One seal function — DONE 2026-09-15 (`9d227afe`, `prikk_store::seal_block` + `BlockLineage`; pins verified against the pre-refactor tree by the architect).** A store function builds, signs and publishes every Block: the CLI's `seal`, the
   merge path, `sync accept`'s `seal_from_accepted` and the RFC 111 simulation all call it. No behaviour
   change; block ids byte-identical before and after on a history that exercises all four callers.
1a. **Readers on v2 — DONE 2026-09-15 (`cb7d940a`; reviewed `.git-exclude/reviewed/rfc136-increment-1a-review-v1.md`).** `SnapshotManifest` v2 encode/decode, v1 removed; `recomputed_state_root()`;
   loading by Blob id (files, node ids, kinds, modes — the readers' "mode-unaware" caveats shrink and
   say so); every reader seeds from a snapshot and skips that block's patches; `checkout --snapshot-*`
   materializes by Blob id. One shared fixture whose snapshot block has patches. No writer.
1b. **The writer — DONE 2026-09-15 (`fdbdfd79`, `f3723bd4`; reviewed `.git-exclude/reviewed/rfc136-increment-1b-review-v1.md`).** **Owed to increment 3's CHANGELOG entry:** a bundle's object count now includes checkpoint manifests, and `show` reports content a checkpoint stored where it said *unavailable*. `CHECKPOINT_CADENCE`, the decision inside the one seal function, `verify`'s
   self-consistency check, the determinism, bundle, digest and storage controls of the first handoff.
   **Two rulings from increment 1a's review (2026-09-15):** (i) *the writer stores every entry's
   content Blob when the store lacks it* — after an `EditText` a text node's `blob_id` is
   `text_blob_id(content)`, the id of the ordinary schema-1 Text Blob, and DC-65 says only that it is
   *not necessarily* stored, so storing it at a checkpoint is additive and makes every entry resolvable;
   without it no history that edits text could use its snapshot; (ii) *the E3 refusal in
   `node_authoring.rs` goes in 1b's commit*, with its test — under §10.1a it names an empty tree whose
   block carries an empty snapshot, a legitimate state, and 1b is what makes it reachable.
   **(iii) ruled 2026-09-15, §10.3a:** 1b anchors no reader, rollback never anchors, `coverage` keeps its
   whole-chain meaning, and there is no release cut between 1b and 2.
2. **The anchor and the gate — split 2026-09-15 into 2a / 2b / 2c (§10.3c)** (read §10.3a ruling 5, §10.3b and §10.3c first; 2a handoff `136-block-aggregation-payoff/sealed-snapshots-increment-2-handoff-v1.md`): checkout and baseline reconstruction start at the nearest snapshot; the
   provisional marker and the derivation gate. Controls as before: byte-equal output with and without a
   snapshot; the gate's refusal and clearing; a corrupted snapshot surfaces its `Integrity` finding.
   **Before increment 2 (2026-09-15):** the pre-existing merge defect found at 1b's review — two edits of
   one file on a side after the baseline refuse a confluent merge as an integrity error —
   `handoffs/DC-75-merge-block-lineage/merge-refuses-a-side-that-edits-a-file-twice-handoff-v1.md`.
3. **Measurement, docs, CHANGELOG** (`### Added — sealed snapshots`; `### Changed` for the gate's new
   refusal), and RFC 136 moves to `done/`.

Handoff: `136-block-aggregation-payoff/sealed-snapshots-handoff-v2.md` (increments 0, 1a, 1b; v1 of the
handoff is superseded and kept for its controls).
