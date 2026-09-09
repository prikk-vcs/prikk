# RFC 133 — What performance costs this project has, and what evidence holds them

**Status.** **ACCEPTED by the project owner 2026-09-03**, as the extraction they instructed: the
measurement concern is *"an independent subject or theme"*, not a verification-culture gate.

**The §6 rewrite was accepted by the owner 2026-09-03. That accepted the question's shape, not an
answer — §6 remains unruled.**

**Accepting this RFC did not rule §6, and §6 has since been rewritten.** Its first draft asked
"does peak RSS get standing protection, and in what shape?" — **the wrong question**, as the owner's
challenge established: most of it was already settled by the 2026-07-30 steady-state ruling and by
DC-86's exchange limits. **§6 now asks the one thing genuinely outstanding: whether memory
independence from repository size should be a stated requirement at all.**

**§7 holds: no increment is handed over from this RFC until §6 is ruled**, because the ruling decides
whether a fix must arrive with a standing measurement or without one.

**Tracks.** Cost and the evidence for it. **No behaviour change is proposed here.**

---

## 1. Why this is its own RFC

**The concern was scattered across three documents, and one of its own labels was undefined.**

| Where it lived | What |
|---|---|
| RFC 126 §5/§5a | the standing peak-RSS measurement question |
| `ROADMAP.md` `AUD-01` | `IndexSnapshot::lookup` is a linear scan; `verify`/`seal` do O(objects) lookups |
| `ROADMAP.md` `AUD-02` | `wal.rs` replays the whole WAL on every append — O(N²) over N queued commits |
| `ROADMAP.md`:328 | "the two performance walls", listed Unscheduled |
| nowhere | the commit create-path memory shape measured in §2 below |

**RFC 126 is "gates that do not exist", and all four of its flanks have been reached.** Keeping it
open for a cost question kept a finished body of work nominally unfinished and filed a performance
defect where nobody would look for one.

## 2. The measured memory shape — and a correction to a claim this project has been repeating

**DC-62's problem statement is quoted in this repository as though it described the present.** It
does not, and the architect repeated the error on 2026-09-03 before checking:

> Commit memory is O(total worktree bytes) regardless of change size: a 1 GB worktree allocates 1 GB
> whether one byte changed or none.

**That sentence describes the state DC-62 was written to measure. The very next line says DC-56
*will* fix it.** DC-56 did.

### 2.1 What was measured

Release build at `995d144`, Linux, worktrees under `/tmp` (tmpfs), peak RSS from
`getrusage(RUSAGE_CHILDREN).ru_maxrss` — an exact kernel figure, not `dc59`'s sampled `VmHWM`, so it
cannot miss the peak of a short run.

**Genesis commit, against total worktree bytes** (128 files of increasing size):

| worktree | peak RSS | over baseline |
|---|---|---|
| 8 MiB | 17.2 MiB | +9.2 |
| 16 MiB | 25.2 MiB | +9.2 |
| 32 MiB | 41.5 MiB | +9.5 |
| 64 MiB | 73.3 MiB | +9.3 |
| 128 MiB | 137.0 MiB | +9.0 |

**Slope 1.0 against a ~9 MiB constant.** A 1 GB first import would take roughly 1 GB of RAM.

**Genesis commit, against file *count*** (256-byte files): 100 → 14.5 MiB, 1,000 → 14.2, 4,000 →
14.0, 8,000 → 21.7. **Flat.** Cost follows bytes, not paths.

**Incremental commit** — 1 MiB changed in an already-committed 128 MiB repository, measured in a fresh
process so `RUSAGE_CHILDREN`'s running maximum could not contaminate it:

```
genesis, 128 MiB worktree            -> 137.3 MiB
incremental, 1 MiB of 128 MiB changed ->  16.3 MiB
```

### 2.2 What that means

**DC-56 works, and the common case is fixed.** An incremental commit costs ~16 MiB whatever the
repository's size — the changed-path index genuinely skips reads for unchanged files, which is
exactly what it was built to do.

**The create path is what remains.** `node_authoring.rs:360` accumulates
`create_candidates: Vec<(String, Vec<u8>, u32)>` — every newly-created file's **content**, pushed at
`:449`, sorted at `:476`, drained at `:477`. So first import and any mass-add hold all new content at
once.

**The buffer exists for a correctness reason**, stated in its own comment: fresh creates are minted in
canonical path order so node-id assignment does not depend on worktree traversal order. **But that
ordering needs the paths sorted, not the bytes held.** Whether the bytes must be resident is an open
question this RFC records rather than answers.

**This is the narrow, true version of the sentence in §2** — and it took measurement to find, which
is DC-62's own point turned back on this project: *"the specific risk is not 'did memory improve' but
is there still a path that loads everything."*

## 3. Two costs carried in from the corrective program

Both re-verified at `995d144` rather than trusted from rows written against `0.27.1`:

- **`AUD-01`** — `object_store.rs:174-179`: `self.entries.iter().rev().find(|e| e.object_id == id)`.
  A linear scan; `.rev()` is what gives last-entry-wins. `verify`/`seal` do O(objects) lookups.
- **`AUD-02`** — `wal.rs` calls `self.replay()?` inside append, so appending to a queue of N commits
  replays all N.

**Neither is measured.** Both are read from source, exactly the standard §2 shows to be unreliable.

## 4. "The two performance walls" — a label with no definition

The phrase appears three times and is defined nowhere: `ROADMAP.md`:328 lists it as unscheduled, RFC
126:107 says it is *"tracked in `ROADMAP.md`'s corrective program"*, and the architect's own criterion
handoff repeated it a third time. **Following either pointer leads back to the other.**

It most likely means `AUD-01` and `AUD-02`. **That is an inference, and an inference is not a record.**
**This RFC retires the phrase.** If two specific walls were meant that are not `AUD-01`/`AUD-02`,
whoever knows must say so; otherwise the named costs above are the whole list.

## 5. What is held by evidence, and what is asserted

| Property | Held by |
|---|---|
| `verify` is not superlinear in history length | **A gate** — `rfc111_index_decode_cost_gate.rs`, observed failing before its fix |
| `seal`'s decode cost | **A gate** — `rfc111_seal_decode_cost_gate.rs` |
| Incremental commit memory is independent of repository size | **One measurement, in §2 of this RFC.** No gate |
| Create-path memory is O(added bytes) | **One measurement, in §2.** No gate |
| `AUD-01`/`AUD-02` costs | **Nothing.** Source reading only |
| `status --format json`'s queue enumeration is bounded by `worktree-status` | **One measurement, §5a below.** No gate |
| Checkout and merge-evidence both cost O(depth^1.45), from two separate uncached chain walks | **One measurement, RFC 136 §9.3.** No gate |
| Commit memory is O(nodes ever created), with a monotonic component that never shrinks | **Nothing.** Source reading, §6b — and it is a *cost*, not a property being kept |

**Time has two gates; memory has none.** That asymmetry is the subject of §6.

## 5a. Queue enumeration cost, measured 2026-09-06 (RFC 140)

**Recorded here because RFC 140 §5 required it, and because the alternative was losing it.** The
implementing round measured this and reported it into `.git-exclude/review-request/`, which is
**untracked and invisible to a clone** — a figure that exists only there is not evidence this project
holds. `rfcs/` is where a measurement survives.

**Method.** One repository, genesis commit sealed, then an unsealed queue grown to 500 patches (one
file created per commit, thresholds raised so nothing sealed automatically). Five runs per cell,
wall clock. Reported by the implementing round; **not independently re-timed by the architect**, and
that limit is stated rather than glossed.

| Command | Empty queue | 500-deep queue | Per call, deep |
|---|---|---|---|
| `status` (prose) | 0.004 s | 0.036 s | ~7.2 ms |
| `status --format json` | 0.004 s | 0.192 s | ~38.4 ms |
| `worktree-status` | — | 0.154 s | ~30.8 ms |

**What it establishes.** RFC 140 §5 ruled option (b) on a **bound** rather than a measurement: the
resolution is the same derivation `worktree-status` — a read command — already performs. **The bound
holds when measured**: ~38 ms against ~31 ms at the same depth, the same order of magnitude, both
running `resolve_folded_worktree_baseline`.

**On an empty queue prose and JSON cost the same**, because both skip the derivation entirely. The
architect confirmed the stronger form of that claim structurally rather than by timing: `run_status`'s
prose body is **byte-identical** to its pre-RFC-140 form — the only deletion in `main.rs` outside
imports was the function signature.

**This is a figure from before the corpus exists**, taken on one machine with a synthetic queue of one
shape. **RFC 139's corpus is what will let it be re-measured properly**, and having this number now is
how we will know whether the corpus changes the answer.

## 5.1 What the published documentation tells users — one row is badly stale

`docs/src/reference/architecture.md`'s known-limits table is the user-facing version of §5. Two rows
concern this RFC:

| Published row | State |
|---|---|
| *"`prikk verify` is roughly **O(N³)** in sealed block count — 34 s at 160 blocks — Tracked, unowned"* | **FALSE, and has been since 2026-08-18.** `MILESTONES.md` criterion 3 records `verify` **linear**, **27.04 ms at N=160**, per-doubling ratio 1.97 — MET via DC-92 and RFC 111, and now held by `rfc111_index_decode_cost_gate.rs` |
| *"Commit cost is not yet bounded independently of repository size (NFR-PERF-01) — Reduced, still missed"* | **Status genuinely open** — see §6 |

**The first row tells readers a solved problem is live, by a factor of about 1,250.** It is the
inverse of the usual documentation risk: the project is understating itself in public.

**And it is not one row.** `architecture.md:197-205` carries a dependent passage that restates DC-92
§1's *problem statement* almost verbatim, including its strategic framing:

> **The mechanism that bounds patch cost is the one that creates prikk's actual cost.** History is
> sealed into a chain carrying state roots, and `verify` re-derives that chain **from genesis, for
> every block** — which is exactly the O(N³) term above.
>
> **Prikk did not inherit Darcs's problem. It has a different one, and it lives in the verification
> path rather than the merge path.**

**That is the project's own published answer to "did the block-oriented approach fail?", and it froze
at the moment the problem was stated.** DC-92 went on to fix it — `verify` is linear and gate-held —
so a reader today is told prikk traded a merge-path cost for a cubic verification-path cost that is
still open. **It is not.**

**Correcting it needs care, not just a number swap.** The passage's mechanism claim — "`verify`
re-derives that chain from genesis, for every block" — may itself no longer describe what happens:
`verify.rs` now routes through `lifecycle_cache::incremental`, and DC-92 was memoization. **A fix that
updates the cost and leaves a stale mechanism sentence would replace one wrong claim with another.**
Tracked here as its own piece of work rather than done in passing.

## 5b. Sealing cost is quadratic in history depth — measured 2026-09-07 (RFC 139 increment 2)

**The first result the measurement corpus produced, and it arrived before the corpus was finished.**
Recorded here because RFC 139 §6 requires it and because `.git-exclude/` does not survive a clone.

**Method.** One repository grown commit-by-commit and sealed from `profiles/prikk-self.toml` scaled to
depth, every commit and seal timed, peak RSS sampled from `/proc` at checkpoints. Raw table and the
binary's own SHA-256: `rfcs/handoffs/139-measurement-corpus/build-cost-curve-report-v1.md`. **One
sample run, on one machine, to depth 128 — not to RFC 139 §6's 2,048 floor.** Reported by the
implementing round; the arithmetic below was re-derived independently by the architect.

| Depth | Seal (ms) | Cumulative (s) | seal ÷ depth |
|---:|---:|---:|---:|
| 32 | 1501 | 71.1 | 46.9 |
| 64 | 3597 | 376.3 | 56.2 |
| 96 | 4636 | 749.3 | 48.3 |
| 128 | 5721 | 1166.2 | 44.7 |

**`seal ÷ depth` is roughly constant, so per-seal cost is linear in history depth and cumulative
build cost is quadratic.** An independent power-law fit of the cumulative column gives an exponent of
**2.03**, matching the implementing round's own figure. Extrapolated to depth 2,048: **≈23 hours**
(from the last two points) to **≈101 hours** (from all four).

### 5b.1 This does not violate RFC 111's gate, and the distinction is the point

`rfc111_seal_decode_cost_gate.rs` asserts *"that sealing one more commit performs a number of full
object-index **decodes** that does not grow with how much history already exists."* Its own doc states
the rest plainly: **`seal` "reads O(N) objects to seal one more block"** — the gate fixed how
expensive each read is, never how many there are.

**So linear per-seal cost is the architecture as gated, and this is the first time anyone measured
what it costs.** §5's table above says time has two gates and memory none; this adds a third category —
**a cost that is gated at the wrong granularity to notice it.** The gate is green and the property it
guards is intact; the property nobody guarded is the one that makes a 2,048-block corpus take days.

### 5b.2 What it now blocks

**RFC 139's corpus cannot reach its own stated floor until sealing gets cheaper.** That dependency did
not exist before this measurement and is the substantive consequence: reducing seal's per-block object
reads is now a prerequisite for the corpus's own purpose, not merely a performance nicety.

**It is not scheduled here.** RFC 133 §7 keeps this RFC descriptive, and §6's ruling still gates any
increment from it. Recorded so the next scheduling conversation has the number.

## 5c. Checkout and merge baseline reconstruction, measured 2026-09-07 (RFC 139 increment 3)

**The measurements RFC 136 was held pending, and the corpus's reason to exist.** Full table, method
and caveats: **RFC 136 §9.3**; raw data
`rfcs/handoffs/139-measurement-corpus/two-measurements-report-v1.md`.

**Both grow as depth^1.45** — checkout `1.446`, merge-evidence `1.445`, re-derived independently —
while tree size over the same range grew only as `depth^0.859`. **Cost tracks replay depth, not tree
size.**

**Two separate implementations, not one shared function**, verified at source:
`patch_replay/read.rs::single_parent_chain` (terminates at parent `None`) and
`lifecycle_cache/replay.rs::walk_single_parent_chain` (terminates at a horizon). Neither is cached —
DC-64's incremental cache is scoped to the commit path, and DC-92's memo lives in `verify.rs`.

**Added to §5's table as a third measured-but-ungated property.** It joins §5b's quadratic sealing cost
in the same category the table now makes visible: **costs that are real, measured, and held by nothing.**

## 5d. `prikk show` at depth, measured 2026-09-08 (RFC 142)

**Recorded because the number exists, not because it is comparable to §5c.** `show` resolves
node-addressed paths through the same `lineage_horizon`/`replay_derived_state` mechanism §5c measured,
so it inherits that cost; but the figures filed with the `show` round were taken on a **degenerate
fixture — one file, held constant while depth grew** — where §5c's corpus grows the tree alongside
depth.

| depth | wall clock, mean of 5 |
|---:|---:|
| 1 (genesis) | 0.001 s |
| 8 | 0.002 s |
| 32 | 0.009 s |
| 128 | 0.020 s |
| 256 | 0.033 s |

**CORRECTED 2026-09-08 (RFC 143): the 0.67 below is not trustworthy, and neither is a re-measurement
that disagrees with it.** RFC 143's round re-measured **the same command on the same degenerate
fixture** and got ≈1.07 by the same two-point method. **Neither number is wrong about the code; both
are fitting process-spawn overhead.** Absolute times across this whole range are 1-4ms, so a two-point
fit at this scale is dominated by exec cost rather than replay cost — confirmed by their min-based
figures tracking their mean-based ones (no outlier skew) while still landing nowhere near 0.67.
**Treat the table below as real times and the exponent as an artifact.** What RFC 143 established
instead is structural and does hold: **`checkout --patch-plan --format json` and `show` are
indistinguishable at every depth measured**, which is what sharing `replay_supported_patch_chain`
predicts.

**Exponent ≈ 0.67 on this shape, against §5c's 1.45** — superseded by the correction above.** The two are not in tension and neither
supersedes the other: **§5c varies depth and tree together, §5d varies depth alone.** Read together
they say the cost §5c measured is carried substantially by tree growth, not by chain length by itself
— which is consistent with §5c's own finding that neither replay implementation is cached, and worth
re-testing directly rather than inferred from two differently-shaped runs.

**`show` also performs exactly one replay per invocation for a block target even when no operation in
the block is node-addressed.** Named here as a known, unclaimed inefficiency; no gate holds it.

**Third measured-but-ungated cost after §5b and §5c** — the pattern in §5's table is now four rows
deep and unchanged: real, measured, held by nothing.

## 6. The ruling this RFC carries — corrected 2026-09-03 after the owner questioned its shape

**The first draft of this section asked "does peak RSS get standing protection?" That was the wrong
question**, and the owner was right to ask what part of it is even this project's responsibility.
Checking the record answers most of it:

**What is already settled, and was before this RFC existed:**

- **Memory is not covered by any requirement at all.** DC-56 §163, verbatim: *"Objective 2 is not
  covered by any requirement. NFR-PERF-01 bounds cost in a latency sense; nothing names"* memory.
- **The owner already ruled the genesis case out of scope on 2026-07-30**: *"NFR-PERF-01 bounds
  **steady-state** commit cost, not every commit including the first."* §2's 137 MiB genesis figure
  is therefore outside what this project has ever committed to bound — **the owner's instinct that a
  large import is the user's matter is not a new opinion, it is the standing ruling.**
- **Untrusted input is already validated.** DC-86's `PRIKK_BUNDLE_MAX_BYTES` /
  `PRIKK_EXCHANGE_MAX_BYTES` / `..._MAX_OBJECTS` bound what a received bundle can make this process
  allocate. That is the part which genuinely was a validation problem, and it is done.

**So the residue is one question, and it is a requirements question rather than a testing one:**

> **Should "commit cost does not scale with repository size" be a stated requirement covering
> *memory*, as `NFR-PERF-01` already states it for latency — or is memory deliberately left
> unbounded?**

**Why it is the owner's:** requirements are. Nothing else here needs a ruling — if the answer is yes,
the evidence follows mechanically from §2's method and the architect writes it; if no, §5's table
gets a row saying memory is deliberately unbounded and this RFC closes.

**What §2 contributes to the decision:** the property is currently **true and free**. Incremental
commit measured **16.3 MiB against a 128 MiB repository**, independent of size — DC-56 already won
it. The question is only whether winning it should be *kept* by evidence rather than by nobody
noticing it broke.

**The cost of yes:** a test that builds two repositories of different sizes and asserts the ratio
between their incremental-commit peaks, run in the ordinary suite. It needs no threshold to
maintain — a ratio does not drift when the machine changes, which was RFC 126 §6's whole objection to
hand-maintained numbers. Its real cost is build time on every gate run, at sizes large enough for the
signal.

**The cost of no:** honest, and cheaper. It means a future change that reintroduces a full-worktree
read on the incremental path passes all 1,558 tests and reaches a release, and the first report comes
from a user with a large repository.

**A separate matter, also the owner's, surfaced by the same reading:** `NFR-PERF-01` itself is
recorded **unmet** — `MILESTONES.md`'s M1 row, DC-56's criterion 8 (*"Recorded 2026-07-31: still
missed"*), and the published table above. **Nothing has re-checked that claim since DC-64, RFC 111,
and DC-92 landed.** Whether it can now be claimed was always the owner's on evidence (DC-92 §101),
and no such evidence has been gathered. **This RFC does not ask for that ruling — it records that the
question is open and unmeasured**, since §2 measured memory and `NFR-PERF-01` bounds latency.

## 6a. ANALYSIS 2026-09-09 — the owner asked whether the property is achievable at all, and the answer splits by axis

**The owner's objection, recorded verbatim because it reframes §6:** *"Is it actually possible in real
world? If not, it should not be stated, for it will harm project procedure and growth."*

**The objection is correct, and §6 as written invites the unachievable reading.** "Commit cost does not
scale with repository size" is not one property. It is two, on two different axes, and they have
opposite answers.

### 6a.1 Against content bytes — true today, achievable, and worth holding

This is what §2 measured and what DC-56 won: an incremental commit costs **16.3 MiB against a 128 MiB
repository**. Nothing about that is fragile or aspirational — the changed-path index skips reads for
unchanged files, which is exactly its purpose. **A requirement on this axis is meetable, is currently
met, and costs nothing to keep.**

### 6a.2 Against node count — structurally impossible today, and stating it would be the harm the owner names

`NodeLifecycleState` (`prikk-replay/src/node_lifecycle/types.rs:52-57`) is four in-memory collections,
every one of them keyed per node or per path:

```rust
live_by_id: BTreeMap<NodeId, LiveNode>,
path_to_id: BTreeMap<RepoPath, NodeId>,
latest_tombstone_by_id: BTreeMap<NodeId, Tombstone>,
seen_ids: BTreeSet<NodeId>,
```

Every commit must materialize this state. So commit memory is **O(nodes)**, unavoidably, for as long as
lifecycle state is a materialized map rather than something streamed or paged.

**And `seen_ids` is worse than O(live nodes): it is monotonic.** Verified at source — it has inserts
(`mutation.rs:51`, `:386`, `:403`) and **no removes anywhere in the crate**. It is the replay-time guard
against node-id reuse, so a deleted node's id must stay in it forever. **Commit memory therefore grows
with every node the repository has *ever* contained, not with its current size**, and a long-lived
repository that creates and deletes files never gives that memory back.

`lifecycle_cache/incremental.rs`'s `try_incremental_step` additionally **clones the whole state**
(`let mut state = cached.state.clone();`) before stepping, so the peak carries a factor of two.

**A requirement of size-independent memory stated without an axis would therefore be unmeetable by
construction.** Adopting it would mean either an immediately-failing requirement, or — likelier and
worse — a requirement quietly reinterpreted until it meant only the bytes axis anyway. **That is
precisely the "harm to procedure and growth" the owner names, and it is a good reason to refuse the
unqualified form.**

### 6a.3 But "memory is deliberately unbounded" is not the right answer either, and §2's own table shows why

§2 reports genesis against file *count* (256-byte files) as: 100 → 14.5 MiB, 1,000 → 14.2, 4,000 → 14.0,
**8,000 → 21.7**, and concludes **"Flat. Cost follows bytes, not paths."**

**Its own fourth row does not support that reading.** Three points are flat and the fourth is not:
+7.7 MiB, a **55% increase**, for a 2x increase in path count — while content over the same step grew by
only 1 MiB (4,000 x 256 B = 1.0 MiB, 8,000 x 256 B = 2.0 MiB). **The jump is not explained by bytes**,
which is the very hypothesis the row is cited to support.

**No cause is asserted here** — one sample per point cannot carry one, and this project has twice stated
a one-sample reading as fact. What is asserted is narrower and sufficient: **the "flat" conclusion is
drawn from a series whose last point departs from it, and the series stops exactly where it departs.**
The node-count axis is therefore not measured to the point where it matters, and prikk's own profile
(875 distinct paths) sits below the region where the anomaly appears. **A real repository is where it
would bite.**

### 6a.4 Recommendation to the owner — the ruling remains theirs

1. **Refuse the unqualified form**, for the owner's own stated reason.
2. **State it on the bytes axis**, where it is true, free, and meetable: *incremental commit memory is
   independent of repository content size.* The evidence follows mechanically from §2's method.
3. **Record the node-count axis in §5's table as a known structural cost, not as a requirement** —
   O(nodes) with a monotonic `seen_ids` component and a 2x clone at peak — so it is visible rather than
   promised.
4. **Re-measure the count axis before anything is ruled on it**, past 8,000 and with more than one sample
   per point. If the departure is real, it is a finding about long-lived repositories that no current gate
   or measurement would catch.

**Item 4 is a measurement task and does not need the owner.** Items 1-3 are the ruling, and it is theirs.

## 6b. RECORDED 2026-09-09 — the node-count cost is now tracked, at the owner's instruction

**The owner's instruction, verbatim:** *"It should be fixed and improved whichever now or later. At
least, it should be recorded and started to be in control now."*

This section is that record. **No AUD id is minted** — `AUD-01`..`AUD-10` belong to the external audit's
own numbering and six of them are retired, so a new one would collide with a retired id. This cost lives
in §5's table and in this section.

### 6b.1 What the cost actually is, decomposed

`NodeLifecycleState` carries **two monotonic components**, and they are not the same size:

| Component | Grows with | Per entry | Redundant? |
|---|---|---|---|
| `seen_ids: BTreeSet<NodeId>` | every node ever created | 32 bytes | **Yes — provably** (§6b.2) |
| `latest_tombstone_by_id: BTreeMap<NodeId, Tombstone>` | every node ever *deleted* | `NodeId` + kind + content (blob id / symlink target) + path | **No** — carries restoration-equivalence data |
| `live_by_id`, `path_to_id` | current tree | node + path | No — this is the working set |

**The dominant monotonic term is the tombstone map, not `seen_ids`** — a `Tombstone` is several times a
bare id. Stating this plainly because the cheap fix below is a *partial* one, and reporting it as the
answer would misrepresent what remains.

### 6b.2 `seen_ids` is redundant, and the codebase already asserts the invariant that makes it so

Verified at source, both directions:

- **Every insert lands in one of the other two maps.** `mutation.rs:51` (into `live_by_id`, having first
  removed any tombstone at `:50`), `:386` (`seed_live_node` → live), `:403` (`seed_tombstone` → tombstones).
  There is no fourth insert.
- **There is exactly one `live_by_id.remove`** (`mutation.rs:60`, in `delete_node`) and it inserts the
  tombstone at `:81`. The two early returns between them are `Err` paths on an already-desynchronised
  path index, which abort replay.
- **Live and tombstoned are disjoint by design** — `mutation.rs:48-50`'s own comment: *"no node_id may
  be both live and tombstoned."*

So on every non-error path `seen_ids == keys(live_by_id) ∪ keys(latest_tombstone_by_id)`.

**This is not merely inferred — the codebase already enforces it.** `cache_ladder.rs:268` validates
`seen_ids` strictly ascending **and** equal to the union, rejecting a decoded cache otherwise:
`"seen_ids must equal exactly live ∪ tombstoned"`. **The project already treats the equality as an
invariant while still storing both sides of it**, in memory, in the clone
(`try_incremental_step`'s `cached.state.clone()`, which doubles peak), and on disk in the cache ladder.

Its three read sites (`query.rs:17`, `validation.rs:27`, `:39`) are membership tests, each replaceable
by `live_by_id.contains_key(id) || latest_tombstone_by_id.contains_key(id)` at the same complexity.

### 6b.3 The steps, in the order their evidence supports

1. **Re-measure the node-count axis** past 8,000 files, with more than one sample per point (§6a.3).
   **This comes first**: nothing should be optimised against a series whose shape is not established, and
   the departure at 8,000 may not be the structure named here at all. Needs no owner ruling.
2. **Remove `seen_ids`.** Correctness-neutral against an invariant already validated on decode; saves
   32 bytes per node ever created, doubled at peak by the clone, and again on disk. **It changes the
   cache-ladder persisted format**, so it needs version handling — the cache is rebuildable by design
   (`incremental.rs`: a save failure is a performance regression, not a correctness one), which makes
   invalidation an acceptable answer, but the format change must be deliberate, not incidental.
3. **Then the real question: can tombstones be bounded?** They exist for restoration-equivalence on
   reintroduction. Whether they can be pruned past a horizon, or reconstructed from sealed history on
   demand rather than held, is a design question this RFC does not answer and must not pre-empt — it is
   the difference between a repository whose commit memory settles and one whose commit memory only ever
   grows.
4. **Independently: `try_incremental_step`'s clone.** It exists so a failed step cannot corrupt the
   cached state. Now that rename resolution is fail-atomic (RFC 144 §4k.3), whether the clone is still
   required for *every* operation kind is worth asking — but it is a separate thread, and "some
   operations are fail-atomic" is not "the step is."

**Step 3 is the one that decides whether the cost is bounded at all.** Steps 1 and 2 are cheap and make
it smaller and measured; neither makes it stop growing.

## 6c. MEASURED 2026-09-09 — incremental commit memory is O(N) past ~4,000-8,000 nodes, and the cache does not explain it

**§6b.3 step 1 delivered at `11871bfd`.** Release build, Linux, tmpfs, `getrusage(RUSAGE_CHILDREN)`,
fresh process per measured commit, **3 samples per point**, N from 100 to 64,000. Both series re-derived
independently at review.

**Incremental commit — the in-scope property** (median peak RSS, KiB): 100 → 11,888 · 1,000 → 11,788 ·
4,000 → 11,840 · **8,000 → 18,476** · 16,000 → 31,856 · 32,000 → 60,028 · 64,000 → 115,688.

**The shape is flat-then-linear.** Flat through N=4,000 across a 40x increase in node count, departing
between 4,000 and 8,000 (1.56x for that step, every prior step <=1.09x), then linear — doubling ratios
1.56x → 1.72x → 1.88x → 1.93x, converging on 2.0x as O(N) does. Large-N fit: **1,781 bytes per node**,
intercept **4.22 MiB**. **A single line does not fit**: extended back it predicts 18,908 KiB at N=4,000
against 11,840 observed, so the kink is real. The genesis series shows the same break independently.

**§2's "flat" reading is now corrected with evidence.** It covered 100-8,000, read three points as flat
with a fourth that disagreed, and stopped at the departure. The fourth point was the start of a real O(N)
region that continues for three more doublings.

### 6c.1 The control returned the *other* answer, and it is decisive

§6b.3 step 1's required control compared peak RSS against the persisted `lifecycle-state.v1` size.

**The cache file is linear in N from the very first point** — ~154 bytes/node, constant across two orders
of magnitude, **no flat region and no departure at all**. RSS is flat then linear. **Different shape, and
~11x larger per node** (slope ratio 11.49x; the 4,000→8,000 step 11.03x — both re-derived at review).

**So the persisted lifecycle state explains neither the shape nor the magnitude.** Two explanations stay
open and this round deliberately does not choose: (a) the *in-memory* `NodeLifecycleState` costing ~11x
its packed serialization, or (b) something else growing with N. A `commit-index.v1` file was observed at
a comparable order (~3.76 MiB at N=32,000) and is flagged as a lead, not a cause.

**Scale of what is unattributed:** at N=32,000, RSS growth over baseline is **~47 MiB**, while
`lifecycle-state.v1` (4.95 MiB) and `commit-index.v1` (~3.76 MiB) together are **~8.7 MiB — under a
fifth.** Whatever explains the rest is not a file on disk.

### 6c.2 RE-RULED — §6b.3 step 2 is a simplification, and attribution replaces it as the next memory step

**Step 2 ("remove `seen_ids`") held its position on a premise this measurement removed** — that the
lifecycle state is where the growth lives. The serialized lifecycle state is ~1/9 of the per-node growth,
and `seen_ids` is ~1/5 of that file.

1. **Removing `seen_ids` remains justified as a simplification, not as the memory remedy.** It is provably
   redundant against an invariant `cache_ladder.rs:285` already validates (§6b.2); removing proven
   redundancy stands on its own under "clean over rich". **It must no longer be described as the fix for
   commit memory.**
2. **The next memory step is attribution.** Measure `NodeLifecycleState`'s in-memory cost in isolation and
   `commit-index.v1`'s own growth, and distinguish (a) from (b) **before changing anything**. Optimising
   the understood ninth while four fifths is unattributed would repeat, in a smaller font, the error §6a.3
   was written about.

**§6b.3's ordering paid for itself.** Measurement was sequenced before optimisation deliberately; had both
been handed off together, `seen_ids` would now be removed, the curve would be essentially unchanged, and
the round would have looked like a fix.

## 6d. ATTRIBUTED 2026-09-09 — none of the named candidates explains the departure, and the residual is the result

**§6c.2 delivered at `2492e6c`.** Same method and ladder as step 1, so the two rounds sit side by side.
Attribution table re-derived exactly at review.

**RSS growth vs the isolated `NodeLifecycleState` probe** (KiB): N=8,000 growth 6,732, probe **−480** ·
16,000 growth 20,368, probe **−216** · 32,000 growth 49,036, probe 4,024 (8.2%) · 64,000 growth 104,700,
probe 17,412 (**16.6%**).

**(a) is ruled out for the departure, not merely shown partial.** At the two points where step 1 located
the kink, the isolated structure shows **no measurable cost at all**. It does grow eventually (11.60 MiB
at N=100 → 28.58 MiB at 64,000) but its onset is an order of magnitude further along the ladder. **A cause
cannot begin after its effect.**

**The kink survives the test that would dissolve it.** Had the large-N rate (1.749 KiB/node) held from the
start, growth at N=4,000 would be **6,998 KiB against 300 observed** — 23x, far outside the sample band.
This is a real regime change, not a linear cost emerging from under baseline noise.

**Shape argues against the persisted files too.** `commit-index.v1` (115-118 bytes/node) and the object
index (133-137 bytes/node) are both **linear from N=100 with no kink**, as `lifecycle-state.v1` already
was. **No named candidate's shape matches the departure's shape.**

**The sweep found five paths scaling with N** and confirmed every other path under `.prikk/` flat by
measurement. Two of the five are the blob and patch containers — the repository's own content, 1:1 with N
by construction, correctly not counted as derived structures.

**So the honest statement stands: the great majority of the departure is unattributed**, and this round
narrows what it is not.

### 6d.1 FINDING — the object index is an in-memory materialization, not a disk-bytes candidate

The round classified `containers/index.container` as *"the wrong unit — disk bytes, not RSS."* **It is
not.** `object_store.rs:160-168` stores `entries: replay.entries` — **the whole index decoded into a
`Vec<IndexEntry>` in memory on every write session** (verified at review; the round confirmed the same
call at source and then filed it under the weaker heading anyway).

It is **the only named candidate known to be held wholesale in RAM per session** — 8.5 MB on disk at
N=64,000, more as a `Vec` of structs. The shape objection still applies and it is not asserted as the
cause; the point is that it belongs in the RSS-commensurate category its own source reading puts it in.

**The obstacle to probing it is tooling, not evidence** — and on checking, there is no obstacle at all.

**CORRECTED 2026-09-09, same day: this section first said the probe "belongs inside `prikk-store`" behind
`test-support`. That was wrong, and checking the crate's own exports before writing the handoff found it.**
`ObjectReadSnapshot` is `pub` and exported from the crate root (`lib.rs:140`), and its `pub fn open`
holds `snapshot: IndexSnapshot` (`object_store.rs:228-240`) — as does `ObjectWriteSession::open`
(`:275-281`), which is the commit path. Both decode the index exactly once, by their own doc comments.

**So the probe needs no visibility change, no `test-support` addition, and no new crate-side code**: it
runs from the existing `prikk-cli` instrument against the **public** API, measuring the real path rather
than a synthetic reconstruction of it. `IndexEntry`'s `pub(crate)` visibility was never in the way — it
only blocked building a *synthetic* `Vec<IndexEntry>`, which is the weaker measurement anyway.

### 6d.2 RULED — AUD-01's completion condition must gain a memory measurement before it is executed

ROADMAP's `AUD-01` completion condition is **"a map built inside `IndexSnapshot::open`, last-entry-wins
preserved, with the existing RFC 111 cost gate extended to cover it."**

**That fix addresses time and pays in memory.** Swapping the `Vec<IndexEntry>` for a map keeps every entry
resident and adds per-entry overhead for the same population — against a structure now measured growing at
~133 bytes/node. And the named gate is **RFC 111's, which is a time gate**: AUD-01 could be executed,
pass its own stated condition, and enlarge the very structure §6d.1 flags.

**RULED: before AUD-01 is executed, its completion condition must require a memory measurement alongside
the RFC 111 time gate.** Not a reason to hold AUD-01 — a reason not to execute it blind.

**AUD-01 is also no longer "unmeasured"**: this round gives its structure a first measured growth series.

## 7. Scope

**In:** the costs named in §2 and §3; the evidence tables in §5 and §5.1; §6's ruling; retiring §4's
phrase; correcting `architecture.md`'s stale `O(N³)` row.

**Out:** fixing any of them. `AUD-01` and `AUD-02` keep their `ROADMAP.md` rows and their completion
conditions; this RFC is where their cost is described, not where it is repaired. **No increment
should be handed over from this RFC until §6 is ruled**, because the ruling decides whether a fix
needs to arrive with a standing measurement or without one.
