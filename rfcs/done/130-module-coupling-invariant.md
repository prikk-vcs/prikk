# RFC 130 — A coupling invariant for `prikk-store`, and the gate that holds it

**Status.** **CLOSED 2026-09-06 and moved to `rfcs/done/` — shipped in 0.34.0** (`f1be4c4`).
The gate exists: `tools/release-policy/src/boundary/coupling.rs` (with `cfg_expr.rs` and `graph.rs`)
runs inside `boundary-check`, and §4b's ruling is what it enforces — **`DECLARED_CYCLES` and
`DECLARED_HUBS` are allowlists with reasons, bound in both directions**, so a new cycle or hub fails
the gate *and* a declared entry that no longer exists in the graph fails it too. §4b.3's requirement
holds in the code: every cycle entry states what would have to change to remove it, which makes the
allowlist a ledger of structural debt rather than a list of exemptions.

**The gate proved itself before this cut.** It caught a regression introduced by RFC 138 — a round the
architect had accepted one turn earlier — which is the strongest evidence available that §3's invariant
is now held by something other than attention.

**What outlives this RFC, and it is work, tracked elsewhere.** The declarations are debt, not
resolution: **8 `DECLARED_CYCLES` entries covering 13 directed edges across a six-module
strongly-connected component, and 5 `DECLARED_HUBS`.** **RFC 131** (module grouping and `pub(in ...)`
scoping) is the named next move against them, and each entry's `what_would_remove_it` is its input.

**Every count in §2 and §4b.4 is superseded — do not cite them.** §2's table is from `04e9391`; §4b.4
named four cycles across six modules; the gate's own re-derivation found fifteen edges across seven,
and carried-defect C's relocation of `maintainer_trust_policy_or_empty` then shrank it to thirteen
across six by removing the `trust -> recognition_claim` edge RFC 138 had added. **The gate's
declarations, pinned by `graph::tests::the_scc_has_exactly_this_edge_set` against the real repository,
are the only current ground truth.**

Everything below is the record as it stood. Previously: **ACCEPTED by the project owner 2026-09-01**, together with RFC 131. Accepted at the
scope written here, including §4's amendment (the hub bound is an allowlist-with-reasons, not a bare
degree threshold), §5 (line and module counts are not gated), §6 (no crate split, `fsutil` included),
and §7's ordering (this lands before RFC 131).

**Folder corrected 2026-09-05: `proposed/` → `accepted/`.** Owner-accepted 2026-09-01 with a
settled design, so an implementer may start; RFC-000 puts that in `accepted/`. Caught by the project
owner. See RFC 123's note for the systemic version of the same error.

**SUPERSEDED 2026-09-05 — this paragraph argued for the wrong thing and is kept, corrected, rather than deleted.** It said this RFC should stay in `rfcs/proposed/` until implemented, because RFC 120's Open-Work Index gates `rfcs/proposed/*.md` only (§6 Q3 excludes `rfcs/accepted/`) and moving an accepted-but-unbuilt RFC out would drop open work off the owner's board. **The cost is real; the conclusion was not.** RFC-000 makes folder location lifecycle authority, and `accepted/` means an implementer may start — which is true here. Using the folder to compensate for a gate's scope put two facts in conflict and resolved it by making the lifecycle wrong.

**The underlying gap is unresolved and belongs to RFC 120, not to this file's location**: accepted-but-unshipped work is now visible only in `ROADMAP.md`'s prose, not in anything gated. Widening the index to `rfcs/accepted/` is an amendment the architect has offered and the owner has not yet ruled on.

Originated by the project owner 2026-09-01, asking whether `prikk-store`
should be restructured because *"it is continuously growing now and also perhaps in the future as
well"*. Answered by an independent external architect review, reviewed and amended by the project
architect.

**Tracks.** A checked structural property of `prikk-store`. No product behaviour changes. **This RFC
does not move a single file** — that is RFC 131, and this one should land first (§7).

---

## 1. Why the evidence is restated here rather than cited

Both source documents — the external review and the architect's review of it — live under
`.git-exclude/`, which `.gitignore:30` excludes from the repository. **A fresh clone can read
neither.** RFC 120 §4 already names this failure mode for findings recorded only in
`.git-exclude/reviewed/`. So the load-bearing measurements are reproduced below, with the commands
to re-derive them, rather than referenced.

## 2. What was measured

At `04e9391`, over `prikk-store`'s **production** `use crate::<module>` edges (test files excluded),
**68 top-level modules, ~292–297 inter-module edges** (two independent derivations, differing ~2% on
import-form handling and agreeing on every conclusion).

### 2.1 The crate is not a DAG

**There is exactly one cycle: `active → refs → active`.** The active-session/WAL layer and the
ref-publication layer depend on each other. Both derivations found it, and found no other.

### 2.2 Four middle-of-graph hubs

Modules with **both** high fan-in and high fan-out — a change propagates in both directions:

| Module | fan-in | fan-out |
|---|---:|---:|
| `refs` | 16–17 | 12 |
| `patch_replay` | 12 | 6 |
| `wal` | 10–11 | 5 |
| `lifecycle_cache` | 6 | 8 |

### 2.3 The rest of the graph is healthily layered

- **Wide foundation, one-directional**: `layout` (41–42 in / 1 out), `fsutil` (**28 in / 0 out**),
  `object_store` (22/2), `byte_cursor` (13/0). High impact, but it flows one way, upward.
- **Isolated orchestrators**: `verify`, `sync_negotiation`, `patch_exchange`, `worktree_patch`,
  `seal_from_accepted` — high fan-out, near-zero fan-in. **Nothing depends on them, so changing them
  is safe regardless of their size.**
- **12 pure leaves** (zero out-edges).

**So the structure is a layered DAG with a wide one-directional foundation and an isolated
orchestrator top — spoiled by four middle-hubs and one cycle.** Those five things are where
"what breaks if I change this?" is genuinely hard to answer.

### 2.4 What is *not* the problem

- **Size.** 74,687 lines (34,883 of them tests), 123 top-level entries. Growth across 45 releases is
  8.4× in lines, but **three feature releases account for 63% of it** (0.18.0, 0.20.0, 0.23.0) and
  every other release moves a few hundred lines. That is lumpy feature growth, not entropy.
- **Compile time.** Independently measured at 3 s / 2.3 s for the lib from clean, 8 s / 7 s for the
  full workspace test build, ~1 s / 0.5 s incremental. **There is no compile-time problem to solve.**

**Neither line count nor module count is diagnostic, and neither should be gated** (§5).

## 3. The invariant

> **1. The top-level production module graph is acyclic.**
> **2. No module becomes a new middle-hub** — high fan-in *and* high fan-out — without a recorded
> reason.

Both halves are computable from one scan of production `use crate::` edges.

## 4. The gate, and the amendment that makes it correct

The external review proposed gating both halves with a bare degree bound (fan-in ≥ K **and**
fan-out ≥ K, suggesting K = 8). **A bare bound is wrong, and this project has the counter-example in
its own history from the same day.**

**RFC 122 (`7a01168`) moved two hubs.** Diffing the graph across that increment:

```
added:  patch_replay -> node_lifecycle      removed: worktree_patch -> lifecycle_cache
        patch_replay -> wal                          worktree_status -> checkout
        worktree_status -> {blob_access, lifecycle_cache,      worktree_status -> snapshot
                            node_lifecycle, patch_replay, wal}
```

`patch_replay` went **12 in / 6 out → 13 in / 8 out**; `wal` went 10/5 → 12/5; `worktree_patch` — an
orchestrator nothing depends on — lost one.

**At K = 8, `patch_replay` at 13/8 violates the bound. So the proposed gate would have rejected
RFC 122** — a correct fix for a High-severity defect, found by an external audit and required by the
architect.

**The reason is structural, not incidental: consolidation and hub-reduction pull in opposite
directions.** RFC 122's whole purpose was to replace two duplicate baseline derivations with one
shared function; sharing a derivation necessarily concentrates edges at the shared site. A degree
bound cannot distinguish **accretion** (a module acquiring unrelated reach) from **consolidation** (a
module absorbing a derivation that previously existed twice) — and the second is behaviour this
project should reward.

### 4.1 The shape the gate must take instead

**An allowlist whose entries carry reasons — the idiom this project already owns twice**:
`DECLARED_UNDOCUMENTED` (`crates/prikk-cli/src/commands/tests.rs`) and
`RFC114_ADMITTED_BUT_UNWRITTEN`. Both exist so that the gate's honesty lives in the requirement that
every entry state a real reason rather than a placeholder.

- **Acyclicity: absolute after grandfathering.** `active ↔ refs` is declared with its reason; **a
  second cycle fails the build, full stop.** No amendment needed here — the external review's version
  is right.
- **Middle-hubs: declared, not bounded.** Today's four are declared with reasons. A module newly
  crossing the threshold **fails until someone adds an entry saying why it is consolidation and not
  sprawl.** The gate forces a recorded decision; it does not adjudicate one.

### 4.2 Open design questions for the implementing increment

1. **The threshold.** K = 8 is a starting proposal, not a ruling. It should be chosen so today's four
   are the declared set and the next accretion trips it — derived from the measured distribution,
   with the derivation written down.
2. **Where it runs.** `boundary-check` is the natural home; it already runs under `cargo test`.
3. **Edge extraction is the load-bearing detail.** Two independent implementations differed ~2% on
   import-form handling (`use crate::{a, b}` grouping, re-exports, `crate::x::y` paths). The gate's
   own definition of an edge must be stated and tested, or the allowlist drifts against it.
4. **Test files must be excluded**, and that exclusion must be part of the tested definition —
   `fsutil`'s only outward edges are test-only, and a gate that counted them would report the
   cleanest module in the crate as coupled.

## 4a. CORRECTION 2026-09-06 — §2.1 was false when written, and §4.1 rejects RFC 122

**Found by the dev team's re-derivation** (the handoff required §2 to be re-measured rather than
copied) **and its provenance established by the architect.** Verified at source, not counted by
script: `.git-exclude/reviewed/coupling-gate-graph-contradiction-review-v1.md`.

### 4a.1 There are four cycles, not one, and the crate has a six-module cyclic core

`active`, `refs`, `trust`, `worktree_patch`, `patch_replay` and `lifecycle_cache` form **one
strongly-connected component**:

| Cycle | At `04e9391`, the commit §2 measured |
|---|---|
| `active ↔ refs` | present — known and grandfathered |
| **`trust ↔ refs`** | **present, and missed** |
| `lifecycle_cache ↔ patch_replay` | absent — one leg only |
| `active → worktree_patch → patch_replay → active` | absent — the re-export leg did not exist |

**§2.1's "Both derivations found it, and found no other" was false at the moment it was written.**
`trust ↔ refs` is production on both legs (`refs.rs:8`'s `mod evidence;` carries no `cfg`; `trust.rs`
contains no `cfg(test)`) and is spelled with literal `crate::trust::` / `crate::refs::` — **visible to
the simplest possible extractor**, so the re-export blind spot §1 warns about does not explain the
miss.

**§2.3's structural conclusion is therefore wrong** — the crate is not "a layered DAG spoiled by four
middle-hubs and one cycle". It has a six-module cyclic core containing three of the four named hubs.
Everything else in §2 reproduced: `fsutil`, `layout` and `byte_cursor` match within one edge, and the
named hubs are still the top four by fan-in.

### 4a.2 The other two cycles were closed by `7a01168` — this RFC's own counter-example

`git log -S` names one commit for both new legs: **RFC 122, thirty-two minutes after `04e9391` and
not an ancestor of it.**

**§4 uses that exact commit to prove a bare degree bound wrong**, because K = 8 would have rejected
*"a correct fix for a High-severity defect."* **The same commit completes `lifecycle_cache ↔
patch_replay`. So §4.1's rule — "a second cycle fails the build, full stop" — would have rejected
RFC 122 as well.**

**§4 found the right counter-example and applied it to one of its two rules.** Its judgement that on
acyclicity *"the external review's version is right and needs no amendment"* was made believing there
was exactly one cycle and that no correct increment had created one. **Both beliefs are measured
false, by the commit §4 was already examining.**

### 4a.3 §4.1 is re-opened; the gate is not implemented

**The question is no longer how many cycles to grandfather.** It is whether **absolute acyclicity is
the same mistake as the bare degree bound, one rule down** — §4's own reasoning, that consolidation
and hub-reduction pull in opposite directions so a structural rule can reject a correct
consolidation, applies verbatim to acyclicity and was never applied to it.

Three options were surfaced by the dev team and **none is taken**: grandfathering three more cycles
freezes a core nobody has evaluated; landing red ships a broken build; investigating which cycles are
accidental presumes acyclicity is the right invariant, which is the presumption now in question.

**Escalation to the external architect is recommended and is the project owner's call** — this
contradicts their central conclusion on evidence they held.

**Also unresolved before any threshold is derived:** the re-derivation counts 61 production top-level
modules against §2's 68. That is a methodology difference (eight `#[cfg(test)]`-gated top-level
modules), not drift, and both numbers cannot seed a threshold.

## 4b. RULED 2026-09-06 — acyclicity becomes an allowlist, for §4.1's own reason

**§4a re-opened the invariant; this rules it.** The question was whether absolute acyclicity is the
same mistake as the bare degree bound, one rule down.

### 4b.1 The analogy is imperfect, and it does not need to be perfect

**A degree bound punishes a good outcome by measuring a proxy.** §4's counter-example is exact: RFC
122 replaced two duplicate baseline derivations with one, and consolidation *raises* the consolidated
module's degree. Degree is a correlate of coupling, not coupling.

**A cycle is not a proxy — it is the thing.** *"What breaks if I change this?"* is unanswerable inside
a cycle because the answer includes the question. So acyclicity is not wrong in the way a degree bound
is wrong.

**But §4a's fact stands regardless: RFC 122 — a correct fix for a High-severity defect, found by an
external audit and required by the architect — created a cycle.** An absolute rule would have blocked
it. That cannot be argued away, and it is not a reason to keep the rule and hope.

### 4b.2 The real defect is that §4 applied its own amendment to one rule and not the other

§4.1 states the principle in the sentence immediately below the one being amended:

> **The gate forces a recorded decision; it does not adjudicate one.**

**That is the right principle and §4.1 applied it to hubs only.** The same paragraph then made
acyclicity absolute — a gate that adjudicates. **The inconsistency is internal to §4, and correcting it
is not a weakening: it is finishing the amendment.**

**Ruled: a new cycle fails the build until it is declared with a reason**, the same idiom as hubs,
`DECLARED_UNDOCUMENTED` and `RFC114_ADMITTED_BUT_UNWRITTEN`. RFC 122 would have passed with one entry.

### 4b.3 A cycle's entry carries more than a hub's, because a cycle is worse

A hub entry says *why this is consolidation and not sprawl* — a judgement about the present.

**A cycle entry must additionally state what would have to change to remove it.** That makes the
allowlist a **ledger of structural debt** rather than a list of permanent excuses, and it is the one
place this ruling is stricter than §4.1's treatment of hubs. An entry that cannot say what would
remove the cycle is an entry nobody understands, and that is a finding rather than a formality.

### 4b.4 The gate's first act is four evaluations nobody has done

The four cycles are declared, not grandfathered wholesale:

| Cycle | Standing |
|---|---|
| `active ↔ refs` | the only one anyone has ever evaluated |
| `trust ↔ refs` | **present at `04e9391` and missed by two derivations** — never evaluated |
| `lifecycle_cache ↔ patch_replay` | created by `7a01168`; never evaluated |
| `active → worktree_patch → patch_replay → active` | created by `7a01168`; never evaluated |

**Writing three of those reasons *is* the evaluation that has never happened.** If a reason cannot be
written — if `trust ↔ refs` turns out to be accidental coupling nobody intended — **that is the gate
paying for itself before it has run once**, and it is a stop-and-report, not an entry to invent.

**Per-cycle entries, not one per-SCC entry.** The six-module component is the *symptom*; the four
cycles are the *causes*, and three of them are independently removable. One entry covering the cluster
would record the symptom and lose every cause.

### 4b.5 This makes the gate more urgent, not less

**Two cycles formed thirty-two minutes after the measurement and nobody noticed for five days**, in a
crate an external architect had just reviewed for exactly this. §4a is not an argument that the gate
was a bad idea; it is the strongest available argument that it is needed — and that its **absolute**
form would have been unshippable, which is a different claim.

### 4b.6 What this does not change

**§4.2's four open design questions stand**, including the threshold — and the 61-versus-68 module
count is a **methodology difference to settle before any threshold is derived**, not drift (§4a).
**§5 (line and module counts not gated), §6 (no crate split) and §7 (RFC 131 after) are untouched.**

**Escalation to the external architect is no longer blocking.** §4a recommended it and the owner has
not ruled; this ruling does not wait on it. Telling them that §2.1 was false at `04e9391` remains
worth doing — for their practice and because they may have a view — but it is now informational.

**Handoff v2 issued 2026-09-06:** `rfcs/handoffs/130-module-coupling-invariant/coupling-gate-handoff-v2.md`. v1 stays in the tree marked do-not-work-from, as the record of what was ruled out and why.
`rfcs/handoffs/130-module-coupling-invariant/coupling-gate-handoff-v1.md` instructs an implementer to
build §4.1's absolute rule. **It must not be worked from as written.**

## 5. What must not be gated

**Line count and module count.** They are the numbers that prompted this work and the least
diagnostic of everything measured (§2.4). Gating them would institutionalise watching the wrong
thing, and would fire on healthy feature growth while a second cycle formed unwatched.

## 6. Explicitly ruled out: splitting `prikk-store` into crates

**Recorded here because the decision was taken on evidence and must survive the git-excluded reviews.**

`fsutil` is the crate's one genuinely clean seam — **0 production out-edges, 28 in-edges**, a trait
contract with per-platform implementations and a conformance suite. **And extracting it is still not
worth doing**, because its production code is *already* decoupled: a crate boundary would remove no
coupling while adding a ninth crate to the fixed per-release publish order, forcing the fixed-size
`[(&str, &str); 8]` registries in `boundary.rs` and `placement.rs` to grow, and converting 28 files'
worth of `pub(crate)` to `pub`.

`patch_algebra` is **not** a clean seam (6 production out-edges into `node_lifecycle`,
`patch_replay`, `path`, `text_span`, `lifecycle_cache`, `object_store`) — correcting the external
audit of 2026-08-31, which had listed it beside `fsutil`. `refs` is the worst candidate: 16 in, 12
out, and it is in the cycle.

**The comparative argument.** gitoxide fragmented early into ~30 crates for two reasons — reuse as a
product, and compile parallelism at scale. **Neither applies here**: `prikk-store`'s internals are
`pub(crate)`, pre-1.0, and the CLI is their only consumer, and there is no compile-time problem.
Fragmenting without those motives imports their costs and none of their benefits.

**Revisit triggers, none of which is a line count:** a leaf gains a production edge back into the
crate (e.g. `fsutil` stops being extractable); a new cycle or a new middle-hub survives review; or
incremental rebuild after a leaf change exceeds ~5 s (10× today).

## 7. Why this is separate from RFC 131, and lands first

RFC 131 (module grouping and `pub(in ...)` visibility) is the larger change and the one that touches
files. **This RFC touches none.** Two consequences:

- **Grouping without the invariant is the failure the external review named**: work that looks like
  progress on side-effect predictability while changing nothing about it. A gate that lands first
  makes the property checked before the layout moves.
- **They collide differently with work in flight.** RFC 131 moves files across the modules RFC 123
  (schema-3 authoring) and RFC 125 (decoder hardening) are editing. **This RFC adds one check and
  conflicts with nothing**, so it can land during band 1 while RFC 131 cannot.

## 8. Outstanding

The external architect offered **the coupling-gate script and the full 68-node edge list**. Taking
that offer is cheaper than rebuilding both, and their extraction is the one the measurements in §2
came from.

**RESOLVED 2026-09-05 by not waiting.** Requested 2026-09-01
(`.git-exclude/tasks/architect/020-20260901-02-coupling-gate-artifact-request.md`); nothing arrived in
four days. **The handoff is issued on the basis that rebuilding is better, not merely acceptable**:
§4.2 item 3 requires the gate's edge definition to be stated and tested, and an allowlist derived from
a *different* extractor than the gate uses would drift from it silently. Deriving both from one
definition is the stronger arrangement, and it removes an external dependency from a gate this project
has to maintain.

**§2's table must be re-measured, not copied.** It is from `04e9391` on 2026-09-01; RFC 122 has since
moved two hubs (`patch_replay` 12/6 → 13/8 by §4's own diff), and 0.32.0 has shipped. **The four hubs
named there may not be today's four.**

**Handoff issued:** `rfcs/handoffs/130-module-coupling-invariant/coupling-gate-handoff-v1.md`.

**STOPPED 2026-09-06 at the re-derivation, correctly and by the handoff's own instruction. See §4a: §2.1's cycle count was false when written, and §4.1's absolute-acyclicity rule would have rejected RFC 122 — the same commit §4 uses to prove the degree bound wrong. No gate is implemented; §4.1's invariant is re-opened.** **RULED 2026-09-06 in §4b: acyclicity becomes an allowlist-with-reasons, the same idiom §4.1 already chose for hubs — the gate forces a recorded decision rather than adjudicating one, which §4 applied to one of its two rules and not the other. A cycle's entry must also state what would remove it. **Handoff v1 is superseded and must be reissued.**

**AMENDED 2026-09-10 — qualified module names.** RFC 131 §6b.2 established that this gate's top-level
node granularity forecloses RFC 131 §3's own target. **The design amending this gate is recorded in
RFC 131 §6c**, in the RFC the gap blocks, rather than reopening this closed one: a node becomes every
production module at every depth, the edge vocabulary (`crate::` paths) is unchanged, and newly-visible
cycles are measured before any allowlist decision.

## 9. Non-goals

No file moves. No visibility changes. No crate split. No change to any product behaviour, object
format, or gate other than adding this one.

## AMENDED 2026-09-08 — the scanner dropped `#[cfg(test)]` before a visibility-qualified `mod`

**A correctness defect in this gate's own scanner, found by an implementing round and fixed at
source.** `find_mod_declarations` reset its pending `cfg` on "any other real code token" — and
`pub(crate)` between an attribute and its item is such a token. So **`#[cfg(test)]` followed by
`pub(crate) mod tests;` lost the attribute, and that module's entire `tests/` subtree was aggregated
as production text.**

`crates/prikk-store/src/patch_replay.rs:570-571` is exactly that shape, and `patch_replay` is the
graph's **top hub** (`min(fan_in, fan_out)` = 11) — so the most connected node in the coupling graph
was being scanned with its test code included.

**The damage was nil, and that is measured rather than hoped.** With the scanner fixed,
`boundary-check` stays `valid` and no `DECLARED_CYCLES` or `DECLARED_HUBS` entry goes stale. **No
declared edge was resting on test-only code; the recorded structural debt is real.** The bug was
latent — nothing under `patch_replay/tests/` had referenced a not-yet-declared cross-module target
until RFC 143's round did, which manufactured a spurious `patch_replay -> block_state` edge and
failed the gate.

**Fixed:** a visibility qualifier is stepped over rather than treated as a resetting token, with
`find_matching_paren` added for the parenthesised forms. **Two regression tests**, one asserting all
four visibility shapes are excised under `cfg(test)`, one asserting a `pub(crate) mod` with *no*
pending cfg is still counted — guarding the fix against over-reaching into "skip anything after
`pub`". **Perturbation-checked: disabling the fix fails exactly the first test.**

**The implementing round was right not to patch this itself** — it is architect tooling, and it
reported the diagnosis precisely enough that the fix was ten minutes' work. **It routed around the
bug instead**, moving a fixture into `test_gates/` where the scanner already behaved, and said so.

## 8. MEASURED 2026-09-12 — the crate's growth per release, and a direction for the owner to rule on

**The project owner: *"The crate is getting larger and larger at recent releases … not only now but also
for the future."*** Measured across every tag from 0.30.0, `crates/prikk-store/src` only:

| tag | date | production lines | test lines | total | files |
|---|---|---|---|---|---|
| 0.30.0 | 09-04 | 40,474 | 36,510 | 76,984 | 256 |
| 0.35.0 | 09-06 | 41,785 | 37,345 | 79,130 | 257 |
| 0.36.0 | 09-08 | 38,616¹ | 41,912 | 80,528 | 263 |
| 0.37.0 | 09-09 | 39,793 | 44,204 | 83,997 | 271 |
| 0.38.0 | 09-10 | 40,758 | 45,098 | 85,856 | 276 |
| **0.39.0** | **09-12** | **40,959** | **45,178** | **86,137** | **276** |

¹ RFC 131 §6b moved the `#[cfg(test)]` modules under `test_gates/`, which this count classes as test —
a reclassification, not a shrink. Like-for-like production growth over the window is roughly +9%.

**Total +11.9% in eight days. Tests +23.7%; production roughly +9%. Tests are now 52% of the crate.**
Top-level entries 125 → 102 (RFC 131 §6b); declared coupling cycles **8 → 8**. Three production files
exceed 1,200 lines (`verify.rs` 1,677, `node_authoring.rs` 1,415, `bundle.rs` 1,411); one more exceeds
1,000 (`foundation/layout.rs`).

**§6's ruling against a crate split stands on its own evidence** — `fsutil` is still the one clean seam
and is already decoupled; a split would remove no coupling. **But §6 answered "split now?", not "how is
growth controlled?"** — and the trend answers that nothing controls it. Every feature lands here
because this crate holds the only store, and the test volume per feature is roughly double the
production volume.

**Direction proposed, for the owner to rule on — three controls, all evidence-shaped like RFC 133:**

1. **Record it.** RFC 141's release evidence gains production lines, test lines and file count for
   `prikk-store` per release. A trend nobody records is a trend nobody rules on; this section had to
   be computed from tags because no release carried the number.
2. **Bound the file, not the crate.** A `size-check` in release-policy: a production file over
   **1,200 lines** fails unless allowlisted with a reason and a `what_would_split_it`, exactly as
   `DECLARED_CYCLES` treats a cycle. Three files enter the allowlist today, each with its reason; the
   next file to cross the line is a decision, not a drift. Production-line delta per release is
   **reported**, not failed — a budget the owner sets later if the report shows one is needed.
3. **Build the instrument a split ruling would need.** RFC 145 §8c found the read surface is not a
   clean facet — `history.rs` imports `RefStore`, which carries `publish`. A **`read-only` cargo
   feature** that `#[cfg]`s the write paths out is the cheapest way to *measure* read/write
   separability: the compiler names every entanglement. §6's "no split" was ruled without that
   evidence because it did not exist. It should exist before the question is asked again.

**What is deliberately not proposed:** relocating tests away from their code (RFC 131 §4 forbids it,
rightly); a line budget on the crate as a whole (it would be met by compressing tests, which is the
wrong thing to reward); a split now.

**Owner rulings:** adopt 1, 2, 3 or a subset; the 1,200 threshold. Nothing is handed off until ruled.

**AMENDED 2026-09-12, same day — the three controls are interim.** The owner asked for the direction to be
reconsidered rather than gated. **RFC 149** measures a one-directional layer cut above the six cycle
members — zero core→surface edges, including through root re-exports — and proposes moving the 26
surface modules, where all recent growth landed, into their own crate. §6 is not contradicted: it ruled
on extracting a module; RFC 149 rules on cutting a layer. Controls 1 and 2 above stay useful after the
cut; control 3 is subsumed by it.
