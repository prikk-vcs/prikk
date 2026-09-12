# RFC 149 — Layering `prikk-store`: a core that stops growing, and a surfaces crate that may

**Status.** **ACCEPTED 2026-09-12** — the owner ruled §7.2 (`prikk-operations`) and §7.3 (gate before the cut) the same day; handoffs for §6 steps 1–2 and for RFC 130 §8's gate are live. Originally: **PROPOSED 2026-09-12**, on the project owner's instruction to reconsider the bloatedness
direction *"not only now but also for the future."* Measured first; the design follows the measurement.

**Direction APPROVED by the project owner 2026-09-12** — *"it is approved to split the crate into
multiple crates (aka cutting out). It is not forced. Just reference. I want 'finally clean, safe and
secure, robust and sophisticated design'."* **That sentence is this RFC's acceptance criterion**, and
"not forced" is why §5's four decision criteria stay as hard stops: a cut that fails one is not made.
**What keeps this in `proposed/`: the surfaces crate has no name yet** — the first move commit needs
one. The architect recommends **`prikk-operations`** (literal, no coinage, no borrowed frame — the layer
*operates on* the store: checkout, merge, rollback, sync, verify, show), with `prikk-ops` as the short
form if preferred. One word from the owner moves this to `accepted/`. Steps 1–2 of §6 (the graph
emission and the public-API census) are already live under the RFC 131 census handoff and need no
name.

**Author-review independence gap:** the architect authored this and will review its implementation.

---

## 1. What RFC 130 §6 answered, and what it did not

RFC 130 §6 ruled out **extracting a module into its own crate**: `fsutil` is the one seam with zero
production out-edges, and extracting it removes no coupling while adding a crate to the publish order.
That ruling stands on its evidence. **But "can a clean module be extracted?" is not "does a
one-directional layer cut exist?"** — and the second question is the one that decides where growth
goes.

RFC 130 §8 measured the growth: **+11.9% in eight days, tests +23.7%, production ≈ +9%**, eight declared
cycles unchanged. Every feature since 0.36.0 — RFC 142, 143, 144, 146, 147 — landed in `prikk-store`,
and almost all of it landed in **read and operation surfaces**, not in the store.

## 2. The measurement — three layers on today's import graph

Every top-level module of `prikk-store` (52), classified by what it imports and what imports it,
production files only, `crate::<module>` references:

| layer | modules | count |
|---|---|---|
| **core** — members of `DECLARED_CYCLES` | `commit_boundary`, `lifecycle_cache`, `patch_replay`, `refs`, `trust`, `wal` | 6 |
| **surfaces** — nothing in the core imports them | `show`, `history`, `verify`, `doctor`, `bundle`, `checkout`, `patch_checkout`, `patch_inverse`, `rollback_draft`, `rollback_preview`, `rollback_verify`, `merge_evidence`, `merge_execute`, `patch_algebra`, `patch_exchange`, `patch_set_digest`, `sync_negotiation`, `seal_from_accepted`, `tag_travel`, `recognition_claim`, `worktree_status`, `worktree`, `block_state`, `state_root`, `compact`, `memory_store` | 26 |
| **below** — imported by the core, in no cycle | `foundation` and 20 others (`object_store`, `author`, `node`, `path`, `lock`, `format`, `text_span`, `trust_index`, …) | 20 |

**The two facts that make this a design and not a diagram:**

1. **Zero edges from core to surfaces.** Not one of the six cycle members imports any of the 26. The
   31 surface→surface edges (e.g. `patch_exchange → verify`, `merge_evidence → patch_algebra`) stay
   inside the surfaces layer.
2. **Zero hidden edges through root re-exports.** `lib.rs` re-exports 264 names; a core module could
   reach a surface through `use crate::<name>` without naming the module (RFC 147 §3e's finding). Checked
   by resolving every bare import in the six core modules against the owner of each re-exported name:
   **none resolves to a surface module.**

**Caveat, stated as a caveat:** this is `crate::` text at top-level granularity, not the coupling gate's
resolved subtree graph. `boundary-check --graph` (RFC 131 §6e's step 0, already handed off) is the
instrument that confirms or breaks it. **Nothing moves until it has run.**

## 3. The design

**Cut above the core.** `prikk-store` keeps *below* + *core* — the store, its durability contract, the
WAL, refs, trust, the replay engine, and the commit boundary: the part that is genuinely one thing, and
the part that is cyclic. **The 26 surfaces move to a new crate above it**, depending on `prikk-store`
one-way. The CLI depends on both.

**Why this is the answer to "getting larger", and the gates in RFC 130 §8 are not:**

- **Growth lands where it belongs by construction.** A new read or operation surface cannot be added to
  the store crate without adding a downward dependency the gate already forbids across crates
  (DC-51's placement gate covers tool↔product; the same mechanism extends to the new crate). The store
  stops growing because there is no longer a place in it for the things that grew.
- **The cyclic core becomes small and visible.** Six modules, eight declared cycles, one crate — the
  thing RFC 130's gate watches is then the crate boundary itself.
- **The tests move with their code**, so RFC 131 §4's rule (no test relocation away from its subject)
  is satisfied, not bent: `verify/tests.rs` goes where `verify.rs` goes. **Test growth per feature
  lands in the surfaces crate** — the same 52% ratio, in the crate that should carry it.
- **It is RFC 145's shape D made structural.** The reviewer's read-only-facet question (RFC 145 §8c) is
  answered not by a feature flag but by a crate boundary: a browse view links the surfaces crate,
  which cannot reach `RefStore::publish` except through the store's own public API.

**Naming.** Plain, literal, no coinage (RFC 145 §9a). The surfaces crate is not "read" (it holds
`merge_execute`, `seal_from_accepted`, `rollback_draft`) — **the owner names it**; the architect's
candidates are `prikk-ops` and `prikk-surface`.

## 4. The costs, weighed against RFC 130 §6's list

| RFC 130 §6 named | here |
|---|---|
| publish order grows | 9 → 10 crates; one line in the fixed registries in `boundary.rs`/`placement.rs` — one-time |
| `pub(crate)` → `pub` conversions | **every item the surfaces reach in the core becomes public API of `prikk-store`.** This is the real cost and it is unmeasured: the increment's first step is a census of exactly which core items the 26 surfaces use |
| removes no coupling | **true, and not the goal.** The eight cycles stay inside the core; the goal is that growth stops compounding in the store |

**A widening of `prikk-store`'s public API is a stability question**, and RFC 132's third trigger
(*"any move toward a library stability promise"*) is close by. **This RFC does not promise stability**
— crate APIs stay explicitly unstable — but it does make the store's public surface larger and more
load-bearing. That is the trade, named.

### 4a. Corrections from step 1 (2026-09-13) — the architect's premises, fixed by measurement

- **The core is five top-level modules, not six.** `DECLARED_CYCLES` holds thirteen edges over
  `commit_boundary`, `lifecycle_cache`, `patch_replay`, `refs`, `trust`. `wal` is a declared *hub*, not a
  cycle member; the "six" in this RFC counted qualified nodes.
- **`prikk-store` has 49 production top-level modules**, not 32; after `rollback/` and `merge/`.
- **A surface is a module the core does not reach, transitively.** That definition yields exactly the
  26 this RFC named. The other 18 non-core modules (`foundation`, `path`, `lock`, `node`, `object_store`,
  `wal`, `author`, `blob_access`, `format`, `snapshot`, `text_span`, `commit_index`, `ignore`,
  `maintainer_signing`, `rename_declaration`, `signature_diagnostics`, `trust_index`, `worktree_marker`)
  are the infrastructure the core is built on and stay in `prikk-store` with it. Under the literal
  "every other module" reading there are 88 core→infrastructure edges and the cut is impossible; that
  reading was never the intent.
- **§5.1 holds**: core→surface edges 0 by construction; lower layer (core + infrastructure) → upper
  layer edges 0, re-derived by the architect from the emitted graph; the re-export pass the graph cannot
  make (29 bare `crate::<ident>` imports in core files) finds none owned by a surface.

### 4b. Consumer impact — must be decided before any move

**CORRECTED 2026-09-13 by stikk's letter 009: stikk depends on no prikk crate** — it drives the CLI and parses its output (`CON-1`), so the rename below is invisible to it. The paragraph that follows assumed a library consumer that does not exist; the architect's assumption, not a measurement. **No external consumer of `prikk-store`'s library API is known.** The notice (letter 008) was sent and cost nothing; the `### Changed — breaking once` CHANGELOG entry still ships for any consumer not known.

The 26 surfaces are the modules external consumers use: `show`, `history`, `verify`, `worktree_status`,
`bundle`, `merge`, `checkout`, `patch_*`. After the cut their root exports live in **`prikk-operations`**;
a consumer adds that dependency and renames `prikk_store::` to `prikk_operations::` for those names.
Mechanical, breaking once, and it lands on stikk — the consumer whose API surface RFCs 142–150 built.
**Owner decision:** proceed with a letter to stikk ahead of the move and a `### Changed — breaking once`
release entry naming every moved name; or hold. This RFC's "no stability promise" (§8) does not make the
notice optional.

### 5.2a. The census needs one more pass before the owner rules (2026-09-13)

Step 2 (`0.42.0-round-1-review-request.md` §C2/§D) found 65 distinct core items reached by the 26
surfaces, 14 already `pub` and root-exported, **51 to change — of which 26 are `not-found`** (module
paths, enum variants, and unresolved functions). Step 2b resolves every entry to a declaration and a
visibility and expands module paths to items. **Two recommendations travel with the resolved list:** the
seven test-only helpers (six in `refs`, one in `patch_replay`, all reached by `verify`) go behind the
existing `test-support` cargo feature, never into the public API; and the list shows what a first cut
without `verify` (24 of the items) would need, as a column.

### 5.2b. The census, resolved (2026-09-13) — the list for the owner

Step 2b (`rfc149-step-2b-report-v1.md`, reviewed) resolves every entry to file, line, kind and
visibility; the team re-read all 77 rows at the line named. After the architect moved six rows (three
variants of already-exported enums to "already public"; three proptest strategies declared under
`tests/` to test-support):

| column | items | note |
|---|---|---|
| **become `pub`** | **41** | `patch_replay` 25, `refs` 7, `lifecycle_cache` 8, `commit_boundary` 4, `trust` 0 |
| test-support (behind the existing `test-support` feature, never public API) | 10 | all referenced only from test code |
| already public | 26 | cost nothing |

**Recommendation to the owner:** approve the 41 as one deliberate widening, recorded in `lib.rs` under
an "operations-layer contract" section; `#[non_exhaustive]` on the enums and structs among them (RFC 147
ruling 2). `verify` stays in the first cut — dropping it saves 6 public items and 6 test-support ones,
and the feature gate handles the latter anyway.

**RULED 2026-09-13.** (1) **The 41 become `pub` — approved.** (2) **§4b: a notice to stikk is required; acknowledgement is not a gate** — prikk has no production use, so the move does not wait on a reply. brygge: no library dependency on `prikk-store` is recorded anywhere in this repository (RFC 128 names it as the history-import tooling project); a copy of the notice is the owner's option, not a requirement. The move handoff (§6 step 4) is live: `149-…/move-handoff-v1.md`.

### 5.2c. Increment 1 delivered; five types widened as entries 42–46 (2026-09-13)

`4c7019b4`: 30 contract names root-exported under `// Operations-layer contract`, 7 test-support helpers
behind the feature, the alias retired, `#[non_exhaustive]` on the types; root exports 269 → 306 (299
default). **Hard stop reported, as §5 requires:** the 41 are not closed under their own signatures —
`RefVerification`, `PointerIndexReplay`, `PatchReplaySnapshot`, `FoldedWorktreeBaseline` and
`TextSpanResolutionFailure` (infrastructure, carried by `LifecycleReplayError`) are returned or carried by
seven approved items and were never spelled by any caller, so the reference census could not see them.
**RULED by the architect: widened, as contract entries 42–46**, `#[non_exhaustive]`; the alternative is a
content change to the core inside a move. The owner's approved list is thereby extended by five names of
this kind only; the owner may object before increment 3 reaches the families that use them.

## 5. Decision criteria — the increment stops if any fails

1. `boundary-check --graph` confirms zero core→surface edges on the resolved graph.
2. The census of core items the surfaces use is a list the owner is willing to make `pub`.
3. Every test moves with its module; no test is relocated away from its subject.
4. The eight declared cycles are unchanged in number and membership afterwards — the cut does not
   hide one (RFC 131 §6a/§6c's failure mode).

## 6. Sequencing

1. **`boundary-check --graph`** (step 0 of the census handoff, already live).
2. **The public-API census** — which core items the 26 surfaces reach.
3. **Owner rulings** (§7).
4. **One move commit per surface family**, moves separate from any content change (RFC 131 §5).
5. **RFC 130 §8's file-size gate and evidence line land regardless** — they are interim controls and
   remain useful after the cut.

### 6a. Increment 2 delivered; `CRATE_ORDER` ruled out of scope (2026-09-13)

`0184f7bb` closed the contract (entries 42–46; root exports 269 → 318, 311 default). `73a9a378` created
`prikk-operations` empty and registered it in **six** lists — two of them fixture copies in
`unsafe_boundary/tests.rs` and `publication/tests.rs` that no grep for `PRODUCTS` finds; the hardcoded
member list is five lists, of which only `size-check`'s is self-checking against `Cargo.toml`.
**`policy/evidence.rs::CRATE_ORDER` is not among them, by ruling:** it has held seven entries and lacked
`prikk-ffi` since RFC 141 §7a (2026-09-06), so the evidence validator already rejects every genuine
document this workspace produces; nothing on the live release path consumes it. It stays RFC 141 §7a's
task, now for nine crates. Not a 0.42.0 blocker.

### 6b. Increment 3 stopped and re-planned (2026-09-13): 21 families move, five stay, and the tests need a named surface

The first move could not be made: `memory_store`, `worktree`, `state_root`, `block_state` and
`rfc111_seal_simulation` are referenced by `prikk-store`'s own tests (the core's, the infrastructure's,
the shared fixture tree), and 21 of the surfaces' tests reach `test_gates::test_support`, the `cfg(test)`
failpoints and foundation's `_for_test` helpers — none reachable from another crate. Production edges
from the lower layer into the movable set remain zero; the coupling graph does not see tests.

**RULED.** (1) The five stay in `prikk-store` **because of what they are** — a store, a materialization
primitive, two state authorities, and a simulation for gates — not because the tests need them; the
census's "not reached by the core in production" was true and answered a different question. Movable
families: **26 → 21**. (2) **Increment 3a**: the `test-support` feature becomes the crate's named
test-support surface — `test_gates::test_support` (28 functions), the failpoints, the `_for_test` helpers
the movable tests reach — enumerated in one `lib.rs` block; measured against the coupling graph and
`size-check` before any move; default builds unchanged. (3) A `prikk-operations` dev-dependency in the
store is refused (the layer inverted in test builds); a third crate is refused (it cannot host the
failpoints); duplicated fixtures are refused. (4) The move order is the reference graph's, tests
included: `patch_exchange` + `tag_travel` are one commit. §5.3 reads: *a test moves with its module and
reaches the store's fixtures through the test-support surface.*

### 6c. Increment 3a delivered; the v1 fixture and the 17 items ruled into the surface (2026-09-13)

`d3340a3f`: the module chain is `cfg(any(test, feature = "test-support"))`, the gates stay `cfg(test)`,
24 names under feature-gated re-exports only; graph 130 → 134 nodes (the four fixture modules), 483 → 502
edges all from them, cycles and hubs identical; package unchanged at 286. **Standing rule:** a fixture
module approaching the hub threshold is the signal to revisit a third crate, not to declare it.
**Ruled (3a.2, one commit before 3b):** the v1 fixture's cascade is two `cfg(test)` items widened to the
feature — taken; the 16 production `pub(crate)` items and two `_for_test` helpers (18, not the 17 first reported) that moving tests reach
become `pub` in their private modules and are re-exported **only under the feature**, in the test-support
block — never the operations-layer contract.

**3a.2 delivered 2026-09-13 (`bb43c86d`)**: the 18, plus four closure types the compiler named (`MutationRoot`, `AuthoredTextSpan`, `TextSpanSelectionError`, `TextSpanSpliceError`) and the v1 fixture chain — 25 names, all feature-gated, default exports unchanged, graph and package unchanged. `NodeLifecycleState` is `prikk_replay`'s type behind a waypoint; the entry can go when `prikk-operations` depends on `prikk-replay` directly. `prikk-operations` consumes the surface as a **dev-dependency** with the feature on.

## 7. Owner rulings

1. ~~Accept the direction: cut above the core.~~ **Approved 2026-09-12, as reference, not forced.**
2. ~~The surfaces crate's name — architect recommends `prikk-operations`.~~ **RULED 2026-09-12: `prikk-operations`.**
3. ~~Whether the file-size gate (RFC 130 §8 item 2) lands before or alongside.~~ **RULED 2026-09-12: before.** The gate gives a measured baseline the cut is then checked against. Read with RFC 130 §8: this adopts its controls 1 and 2 at the proposed 1,200-line threshold; control 3 is subsumed by this RFC.

## 8. Non-goals

No change to the format, the object model, or any command's behaviour. No stability promise. No
removal of a declared cycle — that is separate work, scheduled by the owner from the gate's
`what_would_remove_it` list.
