# RFC 133 §6d.1 — the object index's resident cost

**Ruled:** `rfcs/proposed/133-performance-cost-and-its-evidence.md` **§6d.1** (as corrected). Follows the
attribution round (`2492e6c`), which is **accepted** — this does not reopen it.

**This round measures one candidate. It optimises nothing and changes no production code.**

## 1. Why this candidate, and why it is not what the last round called it

The attribution round filed `containers/index.container` under *"the wrong unit — disk bytes, not RSS."*
**It is not disk bytes.** `object_store.rs:228-240`: `ObjectReadSnapshot::open` holds
`snapshot: IndexSnapshot`, and `IndexSnapshot::open` decodes the whole file via
`replay_index_with_extent` into `entries: Vec<IndexEntry>`. `ObjectWriteSession::open` (`:275-281`) does
the same — **and that one is the commit path.** Both doc comments say "decoding it exactly once."

So this is **the only named candidate known to be materialized wholesale in RAM per session**: 8.5 MB on
disk at N=64,000, more as a `Vec` of structs. Every other candidate was eliminated on shape or onset;
this one has never been measured in the unit that matters.

**Its file is linear from N=100 with no kink, so it is not on its own an explanation of the 4,000-8,000
departure.** You are not being asked to confirm a cause. You are being asked how many of the ~87 MB of
unattributed growth at N=64,000 this structure occupies.

## 2. No visibility change is needed — check this before designing around it

**`ObjectReadSnapshot` is `pub` and exported from the crate root** (`lib.rs:140`). The probe runs from the
existing `prikk-cli` instrument against the **public** API.

**Do not add a `test-support` helper, do not widen `IndexEntry`, and do not build a synthetic
`Vec<IndexEntry>`.** A synthetic reconstruction measures your own fixture; opening a real repository's
index measures the thing that actually runs. `IndexEntry`'s `pub(crate)` visibility was never an obstacle
to the measurement worth taking.

## 3. What to measure

Across step 1's ladder (100 → 64,000), in a **fresh process** per point:

1. **`ObjectReadSnapshot::open(layout)` held live** against a repository with N objects, peak RSS, versus
   an otherwise-identical process that opens nothing. The difference is the resident index.
2. **`ObjectWriteSession::open(layout)`, the same way** — this is the commit path, so it is the number
   that bears on §6c/§6d directly. If a lock or a side effect makes it unsuitable for a bare open-and-hold
   probe, **say so and report (1) alone** rather than working around it silently.

Then place the result beside the attribution table: at each N, how much of the residual does the resident
index account for? **That comparison is the deliverable**, not the raw series.

**Note the object count is not the node count.** N nodes produce blobs, patches, blocks and ref states,
so the index holds more entries than N. **Report the actual indexed-object count at each N**, not just N
— otherwise per-entry figures cannot be compared against the 133 bytes/node the file series gave.

## 4. Method — same as the two rounds before it

Release build, Linux, tmpfs, `getrusage(RUSAGE_CHILDREN).ru_maxrss`, fresh process per measurement, **at
least 3 samples per point with the spread reported**. Extend
`crates/prikk-cli/tests/rfc133_node_count_memory.rs`'s existing harness — **do not build a second
instrument**. `#[ignore]` it as the others are. `python3` and Linux-only remain acceptable.

**Report figures in the unit you computed them in.** The last two rounds each labelled a
divided-by-1000 value as a binary unit ("1.78 KiB/node" for 1,781 bytes; "15.9 MiB" for 15,880 KiB).
Neither changed a conclusion, and this is a request for precision, not a criticism of the results — have
the instrument emit the unit and quote it.

## 5. What this round must NOT do

- **No production code change.** Not `IndexSnapshot`, not `IndexEntry`, not `replay_index_with_extent`,
  not visibility of any of them.
- **Do not execute AUD-01**, and do not build the map its completion condition names. §6d.2 amended that
  condition to require a memory measurement first — **this round is that measurement's prerequisite, not
  its execution.** If the result makes the map look worse for memory, that is a finding to report, not a
  licence to redesign.
- **No `seen_ids` removal** (still sequenced later), no optimisation of any kind, no gate, no threshold.
- `MILESTONES.md` untouched; `NFR-PERF-01` status untouched.

## 6. Gates and report

Full ten-gate set per `rfcs/EXECUTION-ORDER.md` §6 rule 9 against your final commit, plus the instrument's
own deliberate run.

Report to `.git-exclude/review-request/`. **Lead with the comparison against the residual**, and state
plainly whether the resident index is a large, moderate, or negligible share of it. If it turns out to be
negligible, say so — that eliminates the last named candidate and makes the residual's owner genuinely
unknown, which is a result worth having stated cleanly rather than softened.
