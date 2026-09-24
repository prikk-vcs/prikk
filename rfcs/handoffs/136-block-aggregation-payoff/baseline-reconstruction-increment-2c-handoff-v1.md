# Baseline reconstruction from the nearest anchor — RFC 136 increment 2c handoff v1

**Live 2026-09-24, and it is next.** Written on the owner's instruction (*"Write it."*). **This is a design round:
measure and report, then stop.** No product code lands in this round. Implementation is handed off as an addendum
after the architect rules on your report. That order was ruled on 2026-09-17
(`.git-exclude/reviewed/warm-cache-commit-anomaly-review-v1.md` §2.4: *"2c's design round measures (a) before
implementation"*).

**Next after this, in order:** 2c's implementation (an addendum here, after the ruling), then the gate plan. Neither
is live until it is next.

**Read first:** `rfcs/done/136-block-aggregation-payoff.md` §9.3.2, §10.3c (rulings 2 and 3), and §10.5 increment 3.
Then the warm-cache review above.

## Why

After 0.43.0's anchoring, reads and worktree writes are flat past depth 128 (~4 s at depth 256). **Two things
are not:**

1. **Cold-cache `commit` and `merge-evidence`**: ~13.4 s at depth 256, growth exponent ~1.35, unchanged by
   anchoring (§10.5 increment 3). They are §9.3.2's second chain walk.
2. **The warm cache misses on most tips.** DC-64's one-block incremental step cannot materialize text whose
   content came from an earlier block's `EditText`. It then falls back to full replay (DC-65). **This happens on
   56–71 % of tips** in the corpus (`664895b3`). So on edit-heavy history, a warm `commit` usually costs what a cold
   one does.

## What is true at source (checked by the architect, 2026-09-24)

- **Every route ends in one function.** `lifecycle_cache::replay_derived_state`
  (`crates/prikk-store/src/lifecycle_cache.rs:111`) runs `replay::replay_lineage` from the horizon. It has **five
  production callers**:

  | caller | reached by |
  |---|---|
  | `lifecycle_cache/cache_ladder.rs:817` | `commit`'s rung 3 (cold) |
  | `lifecycle_cache/incremental.rs:70` | the incremental path |
  | `lifecycle_cache/incremental.rs:142` | the DC-65 fallback after `try_incremental_step` returns `None` |
  | `patch_algebra/evidence.rs:31` | patch-algebra evidence |
  | `merge/evidence.rs:56` and `:172` | `merge-evidence` |

- **The DC-65 miss** is `try_incremental_step` (`incremental.rs:162`). It clones the cached state and applies
  one block. It returns `None` on `MissingBlobForLifecycleEffect`, because a one-block text cache cannot hold
  content made by an earlier edit.
- **Ruling 3 (§10.3c) binds any anchored baseline.** `replay_derived_state` builds a `NodeLifecycleState` whose
  `latest_tombstone_by_id` and `seen_ids` are **history**, and `create_node` consults both
  (`crates/prikk-replay/src/node_lifecycle/mutation.rs:37-51`). An anchored baseline is allowed **only** at a
  **replay-verified** anchor (ruling 2, `verified_blocks.rs`), and **only** when the whole anchored state,
  **both history fields included**, equals full replay's.
- **A snapshot does not carry those two fields.** Manifest v2 holds files, node ids, kinds, modes and content
  Blob ids. So the two halves of 2c are not the same problem:
  - **the DC-65 half** already has a full state (the cached one, history fields included). What it lacks is **one
    node's text**. Direction (a) fills in only that text.
  - **the cold half** (rung 3 and `merge-evidence`) has no state at all. A snapshot gives it the live tree but
    **not** the history fields, and ruling 3 forbids starting without them.
- `CHECKPOINT_CADENCE` is 64 (`snapshot.rs:31`), so the nearest anchor is at most 63 blocks back.

## 1. The two questions this round answers

### 1a. The DC-65 half — measure direction (a)

**Direction (a), ruled as the direction to measure:** when the one-block step needs a text node's content that it
does not hold, get that content from the **nearest replay-verified anchor** at or before the cached baseline. The
anchor stores every entry's content Blob (increment 1b's ruling (i)). Replay **text only**, forward from the anchor
to the cached baseline — at most 63 blocks. Then retry the one-block step.

- The cached state is **kept**, history fields and all. Only the text cache is seeded, so ruling 3's equality
  question does not arise.
- **No anchor in the verified set** (received history, a missing record, or genesis before the first checkpoint)
  means today's behaviour: full replay.

**Answer, measured:**
- **Rung.** On the corpus, at each of the five depths, what share of tips now take the incremental rung? It was
  56–71 % full replay.
- **Cost.** Warm `commit` at a tip that edits previously edited text: before, and with a **prototype** of (a).
- **Memory.** Peak RSS of that `commit`: before, and with (a).
- **Identity.** Is the resulting state byte-identical to full replay's at every tip of every depth? Compare the
  whole `NodeLifecycleState`, history fields included, not only the tree.

### 1b. The cold half — say what is possible under ruling 3, with numbers

This one has no ruled direction. **Lay out the options; do not choose** (the architect rules). At least these:

- **(i) Split the walk.** Take content from the anchor. Derive `latest_tombstone_by_id` and `seen_ids` with an
  **id-only** walk from the horizon that materializes no text. This only pays if the id-only walk is much cheaper
  than today's full replay. **So measure the split first:** of today's rung-3 time at depth 256, how much is text
  materialization, and how much is the lifecycle bookkeeping? If bookkeeping dominates, (i) is dead, and say so.
- **(ii) Store the history fields with the anchor.** This is a format question — a manifest change or a new
  object — and it needs its own RFC. Say what it would cost in bytes per checkpoint, measured on the corpus, and
  say whether any existing reader would have to change.
- **(iii) Leave the cold half to the warm cache.** If 1a's measurement shows the incremental rung almost always
  holds, a cold rung 3 becomes rare: a first `commit` after clone, after a cache loss, or after a branch switch.
  Measure how often rung 3 actually runs in the corpus build once (a) is in. If it is rare, a measured reason to
  leave it is an acceptable answer: §10.3c says *"or a measured reason to leave it to DC-64"*.
- **`merge-evidence`** does not go through the cache ladder, so (iii) does not help it. State separately which
  option covers it, and what its cost is at the five depths today.

**For every option, answer §9.3.2's acceptance question per call site:** which of the five callers in the table
above it covers. *"Does this also cover `merge/evidence.rs`?"* is an acceptance question, not a follow-up.

## 2. Measurement rules

- **Corpus:** RFC 139, `profiles/prikk-self.toml`, depths **32, 64, 128, 192, 256**. **Pair within one plan**:
  `plan(profile, N)` rescales, so a plan built to 64 is not the first 64 blocks of the plan built to 256 (F2 of
  the warm-cache review).
- **Three interleaved samples per cell**, with per-point elapsed time. Compare **ratios within one session**,
  never minutes across sessions: the same sweep took 2.14× longer on a busier day (RFC 133 measurement round).
- **Peak RSS exactly**, by `getrusage(RUSAGE_CHILDREN)` — the method of `crates/prikk-cli/tests/support/rusage_child.py`,
  which the corpus instrument does not use today — never sampled `VmHWM`.
- **Report corpus build time as a secondary signal** (F1 of the warm-cache review): corpus construction mostly
  pays full replays too, so (a) should show up there.
- **The architect runs nothing on this machine while you measure.** State the load you could not stop, as you have
  been doing.
- **The rung is read from the probe** `baseline_cache_rung_for_test_support` (`patch_replay.rs:770`), cross-checked
  against the cache header the binary persists, as `664895b3` did. The instrument is
  `tools/corpus/tests/warm_cache_commit_anomaly.rs`. Extend it or add a sibling; do not fork it.

## 3. What may land, and what may not

- **May land:** instrument changes and new `#[ignore]`d measurement tests, feature-gated test-support probes (the
  established pattern, review §2.5), and the report.
- **May not land:** the prototype of (a), or any product change. Build and measure the prototype in a **separate
  worktree** that is never committed to `main`. The report shows its diff summary and the one function it
  changes, so the implementation round starts from something reviewed.
- **Controls you already know the shape of**, to be written in the implementation round, not this one. State in the
  report how each would be built and how it would be shown red:
  - at a tip that edits previously edited content, with the cache one block behind, the probe says `Incremental`
    and the header shows `steps + 1` (warm-cache review §2.3);
  - **a lying anchor:** a snapshot that passes the loader but is **not** in the verified set is never used, and
    the result is full replay's;
  - byte-identity of the whole lifecycle state against full replay, history fields included, at every tip.

## 4. Out of scope — say so if you find you need it

- **`seal`'s per-block O(N) lineage reads** (RFC 111's gate; RFC 133 §5b.2). This is a different walk, and it is
  **not** 2c. It is the unscheduled prerequisite for history import (theme 17), and brygge has been told plainly
  that 2c does not decide it. **One paragraph, information only:** after reading `derive_next_state_root`, can the
  same verified anchor bound seal's walk, or not, and why? No design, no measurement. The owner will schedule it
  from that.
- The checkpoint cadence (§10.5 increment 3's open question). Do not change `CHECKPOINT_CADENCE`.
- Any cache schema change. Direction (a) was chosen because it needs none (review §2.4).

## 5. What the report states

- the gates, on the exact final commit (the instrument lands, so all fourteen apply);
- 1a: rung shares, cost, memory and identity at five depths, before and with the prototype;
- 1b: the time split of today's rung 3, each option's cost and call-site coverage, and `merge-evidence` on its own;
- the prototype's diff summary, and the function it changes;
- how each future control would be built and shown red;
- the paragraph on `seal`;
- load during measurement, and anything you could not stop;
- **anything in this handoff that is not true at source.** The table of callers above and ruling 3's reading are the
  architect's, from 2026-09-24. If either is wrong, say it first.

## Addendum 1 — 2026-09-24: the design round is accepted; implement (a) and the anchor-text helper

**The design round is ACCEPTED** (six commits ending `4c739cab`; review `rfc136-2c-design-round-review-v1`; 14/14
gates re-run by the architect). **This addendum is live, and it is next.**

**Two corrections to this handoff, found by the round and verified by the architect:**
- **The caller table above is wrong.** `cache_ladder.rs:817` and `patch_algebra/evidence.rs:31` are test-only.
  `incremental.rs:70` is `verify_divergence`. `incremental.rs:142` is rung 3 for **every** miss reason. And the
  `materialize_edited_text` family was missing.
- **13.4 s in *Why* is a debug-build figure.** `locate_prikk_binary` builds without `--release`. In release, cold
  `commit` at depth 256 is ~0.5 s.

**Ruled (review §3):**
- (a), plus the same helper for the `materialize_edited_text` family, is 2c.
- (i) is deferred and scheduled with `seal`'s lineage walk (theme 17, the owner's scheduling).
- (ii) is rejected.
- (iii) is the interim state: the reanchor and `verify_divergence` stay full replays.

### 0. The instrument measures release

`tools/corpus/tests/support/mod.rs::locate_prikk_binary` builds with `--release`, **fixed in source**, not an
environment knob. A separate, explicitly named function builds debug, for the bridging columns the re-measurement
round will need.
**Control:** assert, from Cargo's own JSON `compiler-artifact` record, that the default binary's profile is
optimized (`opt_level` not `"0"`).
**Perturb:** drop `--release`; the control must go red.

### 1. (a) in product code, as the prototype built it

In `try_incremental_step`'s `MissingBlobForLifecycleEffect` arm:
- find the nearest block at or before the **cached** baseline that has a snapshot **and** is in the replay-verified
  record;
- validate its manifest;
- read the text of the nodes the tip block edits, and carry it forward through the blocks after the anchor;
- **use a text only if its content id equals the blob id the cached state names**;
- apply the tip block to the **cached** state.

Otherwise, fall back to full replay. The fold and `persist` are unchanged.

### 2. The anchor-text helper for the `materialize_edited_text` family

It follows the same two rules: a verified anchor only, and the hash check. **Every production caller is covered,
or named with a measured reason not to be:**
- `commit` (`node_authoring.rs:1323`);
- `merge-evidence` (`patch_algebra/evidence.rs:177`);
- the queued-patch fallback (`replay.rs:493`);
- `diff` (`worktree_read.rs:127`). The prototype left `diff` out, and reading an anchor writes nothing.

### 3. Failures are classified (review §3.3)

Falling back to full replay is always allowed, since the answer stays correct. But a **verified** anchor whose
manifest fails validation, or whose carried text fails its hash, is **named on stderr**, as `warn_anchor_fallbacks`
names a snapshot a reader could not anchor at (§10.3b.4). It is never silently folded.

### 4. Not in this round

- rung 3 for any other reason (no cache, reanchor, horizon or parent mismatch);
- `verify_divergence`;
- option (i);
- `CHECKPOINT_CADENCE`;
- any cache schema change;
- rewriting `current-state.md`'s figures. The re-measurement round after this one does that, and the page
  carries a dated correction until then.

### 5. Controls — landed, not `#[ignore]`d unless they need the corpus; each shown red

1. **The DC-65 miss becomes incremental.** Use a store-level fixture sealed through `seal_block`, so that a
   checkpoint and a verified record exist, and whose tip edits text an earlier block edited, with the cache one
   block behind. The probe says `Incremental` and the header shows `steps + 1`.
   **Perturb:** the arm returns `Ok(None)`.
2. **A lying anchor is never consulted.** Use a snapshot that passes the loader but is not in the verified set
   (`publish_snapshot_fixture_for_test_support`). The rung is full replay, and the state equals full replay's.
   **Perturb:** remove the verified-set guard.
3. **The hash check is its own layer.** Carried text that does not hash to the cached blob id makes the path fall
   back, naming it on stderr, and the answer is correct.
   **Perturb:** remove the hash check. Per the prototype, the step then *errors* instead of falling back.
4. **Whole-state identity.** The entire `NodeLifecycleState`, history fields included, equals full replay's at
   every tip of a small non-ignored fixture history. The history crosses a checkpoint and edits the same file
   repeatedly. The corpus walk (`two_c_chain_walk`) is the `#[ignore]`d large version.
   **Perturb:** apply the block to a snapshot-shaped state (no tombstones).
5. **Per call site.** For each of §2's callers, remove the anchoring at that site. **Its** control goes red, and no
   other does.
6. **Outputs are byte-identical** with and without anchoring on the fixture: `commit`'s patch ids, and
   `merge-evidence`'s printed evidence.

### 6. Measurement — release primary, debug secondary, one session

Repeat the design round's §3.2 table (warm `commit` at the five cells, new-file and edit) and §4.3
(`merge-evidence`) with the product code. Three interleaved rounds, and peak RSS by `getrusage`. **Release is the
column the CHANGELOG quotes.** The architect runs nothing while you measure.

### 7. CHANGELOG

One entry, with release figures: a warm `commit` on history that edits earlier-edited text, and `merge-evidence`,
no longer replay the whole history.

**Next after this, in order:** the release re-measurement round (drafted, goes live when this closes), then the gate
plan.
