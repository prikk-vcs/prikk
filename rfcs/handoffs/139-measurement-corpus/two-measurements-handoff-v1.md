# RFC 139 increment 3 — the two measurements the corpus was held for

**RFC:** `rfcs/accepted/139-measurement-corpus.md` — increment 3 of §9. **The depth ruling recorded
with increment 2's acceptance is settled input and bounds what this increment may claim.**
**Base:** `main` at `253efad`.
**Reports into:** RFC 136 §9 items 1 and 2 — that is where the decision these inform lives.

**Read §2 before estimating. Item 2 needs a builder capability that does not exist, and the profile
cannot supply its shape.**

---

## 1. What this is

The two quantities RFC 136 was held pending, neither of which has ever been measured:

1. **Checkout cost at realistic history depth** — *"the number Option A and C both claim to cut.
   Never measured."* The external reviewer confirmed it has **no honest git-history proxy**, unlike
   §5.1 and §5.3, because it is a property of prikk's *replay*, not of a file-change distribution.
   **This is the reason the corpus exists.**
2. **Baseline reconstruction cost on the merge path.** DC-64 measured the commit path; merge's own
   `replay_derived_state` call was never in that scope. Confirmed still live at
   `crates/prikk-store/src/merge_evidence.rs:51`.

## 2. What you can measure, and what you must not claim

### 2.1 Depth is capped at 256, and that costs you something you must state

Increment 2 measured build cost as **quadratic in depth** (per-seal cost linear; exponent 2.03), which
puts §6's 2,048 floor at **23–101 hours**. The ruling: **2,048 stands as the design's requirement and
is unreachable in practice; 256 is the accepted working depth.**

**256 is 4 `REANCHOR_BOUND` intervals, not 32.**

- **Item 1 is answerable at 256.** Checkout cost against depth needs a trend, and four intervals give
  one.
- **Cadence tuning against `REANCHOR_BOUND` is NOT answerable at 256**, and **this increment must not
  present its numbers as cadence evidence.** Four points show direction, not shape. If a reader could
  mistake your table for a cadence curve, say in the report that it is not one.

### 2.2 Only the replay checkout path is measurable — the snapshot path is unreachable

**Do not try to compare snapshot-backed checkout against replay-backed checkout.** Verified:
`prepare_snapshot_checkout_plan` (`prikk-store/src/checkout.rs:89-98`) **returns
`Integrity("checkout target ... does not contain a snapshot blob")`** when `snapshot_blob_ref` is
`None` — and RFC 136 §5 established that all three block-creating paths write `None`. **There is no
repository on which the snapshot path runs.**

**That is not an obstacle — it is the point.** RFC 136 §9 item 1 asks for *checkout cost **today***,
the baseline Option A claims to cut. Measure `--patch-materialize` (and `--patch-plan`, if the split
between planning and writing is cheap to separate). **The comparison Option A wants comes later, when
something writes a snapshot; your job is the "before".**

### 2.3 Item 2's divergence shape is invented, and the report must say so

**The corpus builds one linear history on one ref.** `tools/corpus/src/execute.rs:34` hardcodes
`REF_NAME = "heads/main"`. There is no branching, so **there is nothing to merge**.

**And the profile cannot tell you what a realistic merge looks like.** Its own recorded extraction
command carries `--no-merges` (`profiles/prikk-self.toml:7`) — **merges were excluded by construction**,
so the profile contains no merge data at all and none can be derived from it.

**So: extend the builder to produce a divergence, and be explicit about what is profiled and what is
not.**

- **The depth is profiled.** Branch from a corpus at real depth; that part is honest.
- **The divergence shape is invented** — how far the branches run, how much they overlap. **Choose it,
  state it, and justify it in one paragraph.** Overlapping edits to the same paths stress
  reconstruction differently from disjoint ones; say which you built and why.
- **Do not present an invented divergence as a profiled one.** A measurement whose realistic half and
  invented half are not distinguished is worth less than one that says which is which.

**This builder extension is IN SCOPE for this increment.** It is not scope creep to be deferred —
without it item 2 cannot be done at all. If it turns out larger than it looks, **stop and report**
rather than silently shrinking item 2 into a shallow one-off, which is the incomparable measurement
the corpus exists to retire.

## 3. Method

**Follow increment 2's own harness shape**: `#[ignore]`d instruments, one growing repository,
checkpoints along the way, peak RSS from `/proc` on Linux only with a missed sample reported as **not
measured, never as zero**.

**Record the binary identity**, as increment 2's build-cost report does — a measurement is not
comparable without knowing what produced it.

**Report the depths you actually reached.** If 256 proves slower than extrapolated, report the depth
you got and its cost. Increment 2 stopped at 128 and said so plainly; that was right.

## 4. Out of scope

- **Any change under `crates/`.** In particular do not make anything write `snapshot_blob_ref` to make
  §2.2's comparison possible — that is RFC 136's own decision, unruled, and it is what these
  measurements exist to inform.
- **Cadence tuning conclusions** (§2.1).
- **Reducing seal cost.** Increment 2's finding makes it a prerequisite for the 2,048 floor; it is
  recorded in RFC 133 §5b.2 and is not this increment's work.
- **The second profile.** Increment 4.
- **Committing a built corpus.** RFC 139 §3.

## 5. Controls

1. **The measurement moves with depth.** Checkout cost at your shallowest checkpoint differs from the
   deepest by more than run-to-run noise — otherwise you have measured something other than depth.
   **State the noise level you observed**, not just the means.
2. **The corpus is the one under test.** Assert the repository you measured was built from
   `profiles/prikk-self.toml` at the stated depth — not a hand-made repository that happens to be deep.
3. **Item 2 actually exercises the merge path.** Confirm `replay_derived_state` is reached — a merge
   that short-circuits before baseline reconstruction measures nothing. Demonstrate it, do not assume
   it from the command succeeding.
4. **The divergence is what you say it is.** The two branches contain the operation counts and path
   overlap your report claims.
5. **A rebuild at the same depth reproduces the manifest** (increment 2's §5a property), so the two
   measurements are of the same planned history.

**Each control seen to fail before it passes.**

## 6. Gates

The full set, verbatim from `rfcs/EXECUTION-ORDER.md` §6 rule 9:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
- `cargo test --workspace --locked`
- `cargo +1.85.0 test --workspace --locked`
- `cargo +1.85.0 check --workspace --all-targets --locked`
- `git diff --check`
- `cargo audit --no-fetch`
- `RUSTDOCFLAGS="-D rustdoc::private_intra_doc_links" cargo doc --workspace --no-deps`
- release-policy `check`, `boundary-check`, `reference-check`

**Cross-target clippy applies again** if your diff carries `#[cfg(target_os = "linux")]` for the RSS
pass — it did last round and **caught a real error**.

## 7. No `CHANGELOG.md` entry

`tools/corpus` is `publish = false` and ships to nobody. **Ruled here rather than left unsaid.**

## 8. Reporting

`.git-exclude/review-request/`. The numbers themselves will be filed by the architect into **RFC 136**
(where the decision lives) and **RFC 133** (where measurements survive) — `.git-exclude/` does not
survive a clone, which is why increment 2's raw table was kept in-tree.

Include:

- **both measurements, with the depths actually reached and the noise level**;
- **which half of item 2 is profiled and which is invented** (§2.3), in your own words;
- **whether anything you measured argues against RFC 136 §7's Option A ruling.** You will be the first
  to hold the number Option A was ruled on without. **If it undercuts the ruling, say so** — that is
  worth more than a confirmation, and RFC 136 is still open precisely so this can change it.
