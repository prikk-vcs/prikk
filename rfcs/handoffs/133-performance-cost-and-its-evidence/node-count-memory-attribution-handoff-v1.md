# RFC 133 §6c.2 — attribute the node-count memory growth

**Ruled:** `rfcs/accepted/../proposed/133-performance-cost-and-its-evidence.md` **§6c.2**. Follows
§6b.3 step 1 (`11871bfd`), which is **accepted** — this does not reopen it.

**This round attributes. It optimises nothing**, exactly as step 1 measured nothing away.

## 1. What step 1 established, and the hole it leaves

Incremental commit memory is **flat to ~4,000 nodes, then O(N) at 1,781 bytes/node**. The required
control compared that against the persisted `lifecycle-state.v1`, and the cache **does not explain it**:
the cache is linear from N=100 with no departure at all, and is **~11x smaller per node** than RSS.

**At N=32,000, RSS growth over baseline is ~47 MiB.** `lifecycle-state.v1` (4.95 MiB) plus the
`commit-index.v1` observed in passing (~3.76 MiB) are **~8.7 MiB — under a fifth. Four fifths of the
growth has no named owner.**

§6c.1 leaves two explanations open and does not choose:

- **(a)** the *in-memory* `NodeLifecycleState` costs ~11x its packed serialization — `BTreeMap`/`BTreeSet`
  node overhead, per-entry allocations, `String` path fields;
- **(b)** something else materialized during commit also grows with N and is not that cache at all.

**Your job is to distinguish them, or to establish that neither is sufficient.**

## 2. REQUIRED — the deliverable is a residual, not a list

Produce an attribution table across the same N ladder step 1 used (100 → 64,000): for each N, the
measured RSS growth over baseline, one column per **named** component you can measure, and a final,
explicit **residual** row — *growth not accounted for by anything named*.

**The residual is the result.** A round that lists three growing things and does not say how much is
still unexplained has not answered the question. If the residual stays large, **say so plainly** — "four
fifths remains unattributed, and here is what it is not" is a good outcome and a publishable one.

**Do not attribute by plausibility.** "BTreeMap overhead explains it" is a hypothesis; the number that
supports it is a measurement of a `NodeLifecycleState` holding N nodes and nothing else.

## 3. The two measurements that are certainly in scope

**(a) `NodeLifecycleState`'s in-memory cost, in isolation.** Construct a state with N nodes and measure
peak RSS against an otherwise-identical process that constructs nothing, so the difference is the
structure and its allocations. Vary N over the ladder. This is the number that decides whether ~11x
in-memory-vs-serialized is real or whether (a) is dead.

Be careful and say what you did: allocator behaviour, page granularity and fragmentation all sit between
"bytes the structure logically needs" and "RSS the process shows". **You are measuring what the process
costs, which is the honest quantity** — but do not describe it as the structure's theoretical size.

**(b) `commit-index.v1`'s growth**, on the same ladder, committed this time. Step 1 saw it at ~3.76 MiB
at N=32,000 in an uncommitted feasibility probe. Make it a real measured series.

**Then sweep for a third.** Step 1's own instrument already captures the repository directory; enumerate
**every** file under `.prikk/` whose size grows with N, rather than only the two already named. A grower
nobody has named yet is the most valuable thing this round could find, and it is cheap to look.

## 4. Method — comparability is not optional

**Follow §2.1's method exactly**, as step 1 did: release build, Linux, worktrees under tmpfs,
`getrusage(RUSAGE_CHILDREN).ru_maxrss`, each measured commit in a **fresh process**, **at least 3 samples
per point** with the spread reported. Numbers that cannot sit beside step 1's are worth much less than
numbers that can.

**Reuse `crates/prikk-cli/tests/rfc133_node_count_memory.rs`'s own harness** rather than building a third
one. If it needs extending, extend it. A second instrument measuring the same ladder differently is the
incomparability RFC 139 §2 exists to retire.

`python3` and Linux-only remain acceptable instrument prerequisites; `#[ignore]` the new work the same
way.

## 5. What this round must NOT do

- **Do not remove `seen_ids`**, and do not touch any state structure. §6c.2 re-ruled that removal to a
  **simplification**, and it is deliberately sequenced *after* this round for a concrete reason:
  **removing it changes `lifecycle-state.v1`'s serialized size, which is one of the quantities you are
  attributing.** Doing both at once would invalidate the comparison.
- **No optimisation of any kind**, including the `try_incremental_step` clone. If you find something
  obviously wasteful, **report it — do not fix it.**
- No gate, no threshold. No `MILESTONES.md`. No `NFR-PERF-01` status change.

## 6. Gates and report

Full ten-gate set per `rfcs/EXECUTION-ORDER.md` §6 rule 9 against your final commit, plus the instrument's
own deliberate run.

Report to `.git-exclude/review-request/`. **Lead with the attribution table and its residual row.** State
explicitly whether **(a)**, **(b)**, both, or neither is sufficient to explain the growth — and if the
honest answer is "neither, and here is what remains," that is the answer, not a failure to find one.
