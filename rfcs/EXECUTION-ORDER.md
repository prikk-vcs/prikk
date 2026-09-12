# Prikk Execution Order

Single ordered view of all open work, for developers to follow in sequence.

This file does not create authority. `MILESTONES.md` remains the schedule authority, `ROADMAP.md` the
backlog narrative, and each RFC its own scope authority. `rfcs/IMPLEMENTATION-STATUS.md` is retired (see
its own banner); citations to it below are historical. This file answers only one question the others
do not: **what do I pick up next, and what is it waiting on?**

**Last reconciled: 2026-08-18.** Since the previous reconciliation the **Windows programme completed and
shipped** — DC-87 (mutation), DC-96 (anchor identity), DC-97 (durability evidence), DC-98 (crash
injection), DC-99 (capability parity) — along with RFC 102 (container durability), RFC 105 (RFC naming
gate), RFC 106 (anchor race control) and RFC 107 (release distribution surface). **Four releases shipped:
0.19.0, 0.20.0, 0.21.0, 0.22.1.** Windows tests went 0 → 956; mutation now works on all three platforms
and is verified there rather than assumed.

**The development lane is now empty of scheduled work.** What remains open is the badge board's five
criteria plus three concepts recorded 2026-08-18 as RFCs 108, 109 and 110 — see "The current picture"
below, which is the authority for what to pick up next.

Historical, from the 2026-07-31 reconciliation after DC-56's scope finding (`8748f00`). **Complete:**
DC-59, DC-60, DC-62, DC-63. **Implemented, awaiting architect review:** DC-58's N1 (`6f53da3`),
DC-61 (`ca4c044`).
**DC-56 closes partial** — its index works, but NFR-PERF-01's dominant violator was misidentified in its
RFC and is carried to **DC-64**, which **also closes partial** (implemented; eliminates the O(operations
replayed) cost but not the O(live node count) remainder — see §1 row 4). **DC-57 held** — handoff withdrawn. Its premise is no longer unreachable: the owner **decided 2026-08-02 that multi-commit queuing is a scheduled capability**, so DC-57 is blocked on that increment rather than on a decision. **There is now no outstanding owner decision** in the development lane. The release-lane decision
point sits after the performance work per the owner's 2026-07-29 direction.

## The two lanes

Development priority and release readiness are separate. **The release lane is `parked`** — no signer
bootstrap, hold, or release candidate exists, and `release-signers.toml` is empty and fail-closed.
Everything in §1 proceeds regardless. Nothing in §1 activates the release lane; activation requires the
three-authority commit described in `MILESTONES.md`, and neither implementation completion nor an
architect recommendation is authoritative for it.

## The current picture — themes and recommended order (2026-08-18)

Derived from `MILESTONES.md`'s status-claim criteria and this file, not from recollection. **This section
is what to read first**; §1 below is largely a completed record.

### The three themes

**Theme A — the verification promise itself.** Two of the four open criteria, and they are one story (criterion 3 closed 2026-08-18 and is kept below as the third entry, struck, because its history is the reason this section exists):

- **No sync** (criterion 1). Two machines cannot exchange sealed history. Recorded as *"nothing built,
  unowned"* and **the largest single gap**. DC-78 delivered the trust groundwork — adopted-key sets,
  per-block signature attribution — and `bundle export`/`import` move objects, but there is no
  machine-to-machine path. **A distributed VCS that cannot distribute.**
- **`verify` checks author signatures, but only where key material exists** (criterion 5, **half
  delivered 2026-08-18**, DC-53 Stage 1 at `970bc27`). A forged AUTHOR signature over recorded key
  material now fails `verify`'s exit status; a Patch whose signer this repository never observed is
  reported *unverifiable* rather than sound. **The remaining gap is "by anyone"**: key material is
  recorded at authoring time and does not travel with a bundle, so a received Patch is permanently
  unverifiable on the receiver's side. Stage 2 must deliver transport *and* pinning together.
- **~~`verify` is roughly O(N³)~~ — criterion 3 is MET (2026-08-18).** DC-92 took it O(N³)→O(N)
  (`b718623`); RFC 111 found and fixed a regression that had taken it back to ~O(N^1.8) for seven days
  unnoticed (`13f7a4b`, `ffaab08`). Now **27.04 ms at N=160, ratio 1.97**, and held by a decode-count
  gate rather than a measurement. **`seal` is not covered by that criterion and still performs O(N) reads
  per call** — a residual owned by no increment.

**Theme B — whether the promise survives time.** Criterion 2, unanswered: *what minimum must never change
for a verification claim made today to hold in ten years.* 0.20.0 moved the format five times with no
migration path. Until this is settled, the badge's *"future releases may require migration"* is
load-bearing.

**Theme C — release authority.** Criterion 4: the signer bootstrap has never happened.
`release-signers.toml` is empty and fail-closed, and every release to date — including 0.22.0 and 0.22.1 —
went out on one person's key without DC-35's two-person authority transaction. It also blocks **DC-43** and
**DC-49** in §2.

**Outside the themes**, two capability gaps DC-67's conformance suite found and left unowned:
`checkout --patch-materialize` cannot replay `ReplaceBinary`/`ChangePerm`, and there is **no
working-directory branch switch** — the second being the kind of thing a new user meets in the first ten
minutes.

### Why the new concepts do not come first

RFCs 108, 109 and 110 each depend on criteria that are open:

| Concept | Depends on | Why |
|---|---|---|
| **RFC 110** — agent safety and provenance | **Criterion 5** | Its audit, compliance and IP-lineage value has no floor while `verify` never checks who signed anything |
| **RFC 109** — agent-native interface | **Criterion 3**, then 2 | An agent-native tool multiplies commit volume, and at O(N³) three times the history is roughly twenty-seven times the verification cost. Criterion 2 gates its AST-sealing ruling |
| **RFC 108** — Workspace | **Criterion 3**, then 1 | Sealed Workspaces multiply history the same way; and its §6/§7 topology answer decides whether Workspace transfer *is* sync |

**So the badge board is these concepts' foundation, not their competitor.** Scheduling them first would
build an audit story on unverified authorship, a high-volume tool on superlinear verification, and
permanent metadata on an unsettled format.

### Recommended order

| # | Work | Why here |
|---|---|---|
| 1 | **DC-53** — repository-wide author trust verification (criterion 5) | Cheapest of the five; closes the only place the README promises what the code does not do; unblocks RFC 110's second pillar entirely |
| 2 | **Criterion 3** — `verify` cost | Prerequisite of RFC 108 and RFC 109. Measured, fix named — **it needs an owner more than it needs a discovery** |
| 3 | **Criterion 2** — format stability | Gates every sealing decision in RFCs 109 and 110 |
| 4 | **RFC 108 topology** (§6/§7) | Decides whether Workspace transfer *is* sync, and therefore whether criterion 1 is attacked directly or arrives as a by-product |
| 5 | **Criterion 1** — sync | The largest gap; its shape is determined by item 4 |

**The project owner may reorder by product value.** The dependencies above are what constrain order; the
sequence within them is a judgement.

### The one item that is not engineering work

**Criterion 4 — the signer bootstrap — requires two distinct natural persons** under DC-35's authority
transaction. Neither the architect nor the developer can perform it, and no increment unblocks it. **Only
the project owner can start it**, it can run in parallel with everything above, and it independently
unblocks DC-43 and DC-49.

## 1. Development lane — available now

Ordered by recommended sequence. The project owner may reorder by product value; the **Blocked by** column
is what actually constrains order.

Hand the developer the handoff, not the RFC — the RFC is scope authority, the handoff is what they work
from. **DC-57's handoff v1 is withdrawn — issue v2 only.**

| # | Increment | State | Blocked by | **Handoff to give developers** |
|---|---|---|---|---|
| 1 | **DC-58** — source-structure audit | **Complete.** N1 reframing `6f53da3`, reviewed and accepted 2026-07-31 | nothing | — |
| 2 | **DC-61** — branch closure (§6.5 deletion half) | **Complete.** Implemented `ca4c044`, reviewed 2026-07-31 (accepted with one non-blocking finding), **N1 repaired `2394f1b`** | nothing | — |
| 3 | **DC-56** — commit scan + memory compliance | Implemented `8748f00`. **Closes partial** — criteria 1,2,3,6,7 met; 4 and 5 re-scoped and carried. **NFR-PERF-01 not closed** | ruling applied 2026-07-31; nothing outstanding on the developers | — |
| 4 | **DC-64** — baseline reconstruction cost (NFR-PERF-01, carried) | Implemented; **closes partial** — the O(operations replayed) full-lineage-replay cost (97.6% of the phase) is eliminated on the warm path, but `load`/`persist`/`from_replay`, each a binding condition of the trust-ladder ruling, remain O(live node count), so Axis A is not fully flat. **NFR-PERF-01 remains missed**, on a lower curve. See the design document §9 | none | — |
| 5 | **DC-65** — text-edit baseline content | **Complete at `250ad54`** — reviewed and accepted 2026-07-31, verified independently at N=6 sealed edits, one non-blocking process note (reported gate commands did not match §6 rule 9; architect re-ran the canonical set, all pass). §1's four questions answered before design (`prerequisite-questions-v1.md`); invariant stated and both `plan_edit_text` and `patch_algebra::baseline_text` conform; a fifth site (DC-64's incremental step) found and fixed once the first three unblocked it; N=4/5 sealed-edit tests at store and CLI level; `ReplaceBinary` confirmed unaffected with a regression test; coverage finding recorded | none | — |
| 6 | **DC-66** — multi-commit queuing | **Complete at `45af36f`** — reviewed and accepted 2026-08-02, all eleven criteria met, one non-blocking note (rollback-draft still rejects on a non-empty WAL, deliberately). Architect independently rebuilt a four-deep queued edit chain from sealed history and got byte-correct content | nothing | — |
| 7 | **DC-57** — active-patch thresholds (NFR-PERF-02) | **Complete at `caa2fc2`** — reviewed and accepted 2026-08-02, no findings. Hard block fires before any write; `>=` semantics documented; config fails closed on all four bad inputs. **NFR-PERF-02 implemented** | nothing | — |
| 8 | **DC-67** — ordinary-use conformance suite | **Complete at `d87a542`** — reviewed and accepted 2026-08-02. **The prediction held**: two findings reported, neither fixed here — (1) `checkout --patch-materialize` cannot replay `ReplaceBinary`/`ChangePerm`, (2) no working-directory branch-switch. Both now unowned | nothing | — |
| 9 | **DC-69** — lifecycle-state retention | **Complete at `9279c08`** — reviewed and accepted 2026-08-03. **Route (c): prikk does not forget, established and measured**, not assumed. `seen_ids` and `latest_tombstone_by_id` answered separately (the latter self-prunes on restoration). §3.3 declined with reasons: a horizon needs two decisions this increment does not own — bounding `rollback-draft`'s reach, and redefining what full replay trusts. No production code | nothing | — |
| 10 | **DC-70** — prebuilt binary distribution | **Complete-partial at `2ca02f9`** — reviewed 2026-08-03; B1 (an unsound `inert_head` widening covering `tar`/`rustc`/`gh`) repaired and re-reviewed. Release workflow, checksums, `binstall` metadata and README install surface delivered. **Criterion 3 carried**: release evidence models one archive, not N, and extending it is blocked behind DC-45's frozen baseline until the 0.19.0 cutover | nothing | — |
| 11 | **DC-71** — non-Linux build conformance | **Complete at `7c898f8`** — reviewed and accepted 2026-08-04. Read-only commands now **execute** on macOS and Windows, CI-verified continuously; mutation stays Linux-only per DC-37. Three blocking findings, all real, three of four found by the CI job this increment created. Closes the release-claim-mismatch row standing since the original architecture review | nothing | — |
| 12 | **DC-72** — path-safety conformance (NFR-SEC-03) | **Complete at `40fdc7e`** — reviewed and accepted 2026-08-04. The §2 table found the recorded gap was wider than described and surfaced a fourth surface (maintainer trust key ids, where a collision could silently reduce a maintainer threshold). Collisions now rejected at creation on three surfaces through one shared fold; reserved names rejected for trust key ids. Architect's §3.5 placement ruling was **wrong in detail and corrected** — the named validators are pure and cannot enumerate | nothing | — |
| 13 | **DC-73** — node-model operation apply | **Complete at `18c5df3`** — reviewed and accepted 2026-08-04, no findings. `ReplaceBinary`/`ChangePerm` now materialize and invert; `CreateFile`'s mode threaded through checkout, closing a silent defect where every file rebuilt as `0600`. Wire format untouched. `RenamePath`/`CreateSymlink` left unimplemented with markers corrected to name the real blocker (no authoring path). **Unblocks three tests in DC-65/DC-67 that worked around the old gap** | nothing | — |
| 14 | **DC-74** — merge execution | **COMPLETE at `3464e2a`** — reviewed and accepted 2026-08-08. Adopts patches verbatim (byte-identity asserted, non-vacuously); refuses cleanly on five live-constructed conflict classes. Architect re-ran all nine gates independently (880/880 both toolchains, 180 packages) and constructed two scenarios the submission did not: an **over-old baseline**, which fails closed by construction, and a **negative control** showing 4 of 5 refusal tests do not pin the confluence gate — benign, since `derive_next_state_root` independently refuses with zero object writes. Two non-blocking findings. **Still release-conditioned**: does not ship until DC-75 discharges the structural-record condition | nothing | — |
| 15 | **DC-75** — merge block lineage / structural merge record | **COMPLETE, merged 2026-08-08** at `c79c421`. Merges seal as `BlockKind::Merge` recording both parents, a mainline pointer, and the baseline; `verify_merge_baseline` **re-derives** rather than trusts it. **Discharged DC-74's release condition**, clearing 0.19.0. Existing object ids proved byte-stable — no hash literal moved | nothing | — |
| 16 | **DC-76** — filesystem durability contract | **COMPLETE at `d568438`** — reviewed and accepted 2026-08-09. Nine guarantees stated as one `DurabilityContract` trait with guarantee-named methods; `LinuxDurability` sole implementor; no behaviour change (892 tests unchanged). **Nine negative controls, two of which could not be cleanly demonstrated and were reported as findings rather than dropped** — architect independently confirmed the G9 kernel behaviour and re-ran G1. **One blocking finding (B1): the first submission broke non-Linux CI; the gate gap that allowed it was the architect's, and rule 9 was amended.** Repaired at `d568438` with a *conditional* dead-code allowance, not a blanket one | nothing | — |
| 17 | **DC-77** — docs Mermaid rendering | **COMPLETE, merged 2026-08-08** at `7e90776`. Diagrams render as pictures; one **exact** `procedure.rs` allowlist entry, proved narrow by the architect against four rejected variants. Vendored assets, offline, no CDN | nothing | — |
| 18 | **DC-78** — history exchange | **STAGE 1 COMPLETE, merged 2026-08-09** at `cba0459` after a **green macOS CI run**. Trust store is an adopted-key **set**, TOFU enforced on conflicting re-add, and adopting a second key no longer invalidates the receiver's own history. **Merged once prematurely and reverted**: the architect merged on local gates alone, and macOS CI then found a security regression — APFS case-folding let a colliding key id skip the DC-72 collision check. Fixed by running that check unconditionally; **verified on macOS, where the bug lived**. **STAGE 2 (§D3) COMPLETE, merged 2026-08-09** at `edbc94c` — `verify` now reports `sealed-block <id>: <key_id>` per block, discharging §D3's reporting gap; architect's negative control confirms the test pins per-block attribution, not mere presence. **Merged after a green macOS run — the standing rule applied before the merge for the first time.** **Remaining: D4/D6 + ruling 4 together** | nothing | `handoffs/DC-78-history-exchange/implementation-handoff-v2.md` + addenda 1-4 |
| 19 | **DC-79** — sha2 + getrandom upgrade | **COMPLETE, merged 2026-08-09** at `7ad0af3`. MSRV holds at 1.85; **no digest moved** — DC-41 vectors and the randomized frozen pre-DC-55 differential both pass unchanged, no hash literal edited. `getrandom` → `fill` rename at three sites, ruled in scope. **Lock 180 → 187**: six transient `digest`-stack duplicates that DC-80 collapses, plus `hybrid-array` permanent. Architect's "cosmetic" framing of that was understated and is corrected | nothing | — |
| 20 | **DC-83** — test temp-dir uniqueness | **COMPLETE at `76c8d18`** — accepted 2026-08-09. **The developer correctly refused the architect's instruction**: `monotonic_suffix` is a wall-clock timestamp despite its name, so "mirror `unique_temp_dir` exactly" would have left the bug in place. Fixed with an `AtomicU64`; 214 collisions → 0 across 128,000 samples | nothing | — |
| 21 | **DC-84** — test helper uniqueness sweep | **COMPLETE at `3825867`** — accepted 2026-08-09. Fifteen helpers (a **recount** against the architect's thirteen), one shared `unique_suffix` for prikk-cli plus an independent fix for prikk-store across a genuine crate boundary, `monotonic_suffix` renamed. 896 tests, two zero-collision demonstrations. **First increment committed to a branch rather than local main** | nothing | — |
| 22 | **DC-86** — bundle decoder hardening | **COMPLETE, merged 2026-08-09** after a green macOS run. Proptest coverage on both untrusted-input decoders; import bounded on object count and total bytes, **refused before any write**. Count check precedes allocation and uses `Vec::new()`, not `with_capacity(count)` — no allocation bomb. Architect disabled both bounds: exactly the three bound tests failed. **Honest scope statement** distinguishing the committed 1,024 cases from a one-off 800,000 campaign | nothing | — |
| 23 | **DC-80** — ed25519-dalek 2→3 | **COMPLETE, merged 2026-08-10** after a green macOS run. `verify_strict` differs by one line (`&[message]`, multipart API); **no verification semantics changed** — architect independently reproduced the cross-version probe, including `S+L` malleability with `high-3-bits-set=false`, rejected by both. Mixed-version history (2 blocks under 2.x, 1 under 3.x) verifies clean. Lock **187 → 179**. **The architect's six-package collapse figure was wrong; measured, only `const-oid` collapses** | nothing | — |
| 24 | **DC-85** — merge from a received ref | **COMPLETE, merged 2026-08-10** at `596edfc` after a green macOS run on the reviewed commit `a8f3f61`. `merge --from remotes/<name>` works, gated by §3A.1's mandatory trust check: every adopted Block must carry a **currently**-trusted MAINTAINER signature, checked over the same candidate-block walk `candidate_patch_ids` already does and before `into_ref` advances. `validate_local_branch_ref` not relaxed; `into_ref` stays local-only. **Architect's negative control disabling the gate: the merge did not merely proceed, it completed** — `heads/main` advanced to a Merge block adopting content sealed by a never-trusted key, so DC-78 Stage 3's gap was fully exploitable. Induction exempting local merges re-derived independently across every `RefStore::publish` call site. **The architect's review claimed no existing doc statement had become false; `merge-plan.md:24` had, and the developer found and corrected it** | nothing | — |
| 25 | **DC-81** — macOS mutation | **COMPLETE, merged 2026-08-09** after a green macOS CI run. `MacosDurability` implements the contract; **G3 uses `fcntl_fullfsync`** — measured **180x slower than `fsync`** on the runner, recorded in `FINDINGS.md`. Four platform differences found by running, all in **test fixtures** rather than production primitives. **Two architect errors this cycle** — a premature merge to main, and a docs commit landing on the developer's branch; both corrected | nothing | — |
| 26 | **DC-82** — mutation dispatch collapse | **COMPLETE, merged 2026-08-09** after green macOS CI. `NoDurability` as a third implementor; one gated `ACTIVE_DURABILITY` constant; **all eleven mutation call sites unconditional**; `anchored.rs` 43 → 14 gate attributes, each now scaling with **platform count** rather than call-site count. **Criterion 3 (whole-tree single digits) not met — the target was the architect's to miscalibrate**; the sub-contract layer is per-platform types and primitives, deferred to the Windows increment | nothing | — |
| 27 | **DC-87** — Windows mutation | **ACCEPTED 2026-08-10.** The other half of the owner's cross-platform mutation priority, and the increment **DC-82's unmet criterion 3 deferred by name**. Windows resolves to `NoDurability` today; read-only is already CI-gated, and **DC-72 already rejects the Windows-hostile path forms cross-platform**, so what remains is genuinely the durability backend. The obstacle is that `MutationRoot` holds a `rustix` fd and the whole primitive layer is Unix-shaped — `openat` has no Win32 equivalent, and G1 is defined in terms of it. **Two stages** (seam-neutral refactor, then the backend), per DC-82's own split rationale. **Six blocking prerequisites precede design**, the sharpest being whether `durable_directory_entry` (G3) is implementable on NTFS at all and what that does to DC-38's crash-recovery reasoning. **Round 1 answered and ruled 2026-08-10.** `FlushFileBuffers` does not apply to directories and `REPLACEFILE_WRITE_THROUGH` is documented "not supported", so **DC-38's "format-2 publication never permits an ahead log" does not carry to Windows** — though DC-38's own format-1 clause already defines a bounded recovery for that state, which is a security question, not a portability one. **Three architect corrections**: `ReplaceFileW` is not atomic (three documented partial-completion error codes); `MOVEFILE_WRITE_THROUGH` was unexamined and is now the decisive open question; and G9's framing was wrong — prikk records only the execute bit, and `read.rs`'s `mode = 0` sentinel would silently drop it from sealed history. **`#![forbid(unsafe_code)]` blocks every raw Windows syscall from `prikk-store`** — whether prikk gains its first `unsafe` surface is **escalated to the owner**. **Mode-carrying fix MERGED 2026-08-10** at `d2fcac0` after a green three-platform CI run on `1e10a09` — the `read.rs` `mode = 0` sentinel is gone, `RootFileStat`/`WorktreeFileMeta` carry `Option<u32>`, and an unobservable mode now declines to plan `ChangePerm` instead of silently stripping the executable bit from sealed history; architect's negative control (`unwrap_or(REGULAR_FILE_MODE)`) failed exactly the one test that models the hazard. **Stage 2 BLOCKED 2026-08-11 on an owner scope decision, and Stage 1 held with it.** The transition-durability investigation confirmed DC-38's invariant cannot hold on Windows: step 6's log append is an existing-file content append and *is* achievable, while step 5's pointer promotion needs transition durability and is *not* — the asymmetry reproduces the exact ahead-log state DC-38 exists to eliminate. **The sequencing call paid off**: had Stage 1 gone first it would have built a seam for an implementor that cannot exist. Three options escalated (restructure publication / ship a weaker invariant / do not ship Windows mutation), with **DC-91** proposed to ask the question that decides between them. **DEFERRAL ACCEPTED by the project owner 2026-08-11, explicitly as "deferred but controlled"** — meaning: Stage 2 is not abandoned and not silently parked. It is blocked on one named decision, its unblocking condition is DC-91's answer (or an owner ruling on the three options without it), and this row is the tracking record. **Windows read-only support is unaffected and remains CI-gated throughout.** Nothing about this deferral weakens a shipped guarantee; it declines to add a platform whose guarantee could not be met. **Sequencing ruled 2026-08-11: the Stage 2 transition-durability investigation goes first, Stage 1's seam after it** — the seam exists only to house a Windows authority type, so building it before knowing whether Windows can satisfy `atomic_replace`/`promote`/`durable_append` is speculative. Same principle the architect misapplied against DC-88 (which changed no type) and now applies to the question that genuinely can change what the type must do **Narrow round 2 ruled 2026-08-10:** `MOVEFILE_WRITE_THROUGH` could not be settled from primary sources after three corroboration attempts, and **the architect withdrew the question as the wrong blocker** — `durable_directory_entry` is the one `DurabilityContract` method named after its primitive rather than its requirement, so "Windows cannot fsync a directory" never entailed "Windows cannot publish a ref durably." Stop-and-report fired; **Stage 2 now blocks on DC-88, not on a Windows API fact**. Mode-carrying shape accepted (sentinel `0` becomes `Option`), plus a doc defect the architect found at `commit_index.rs:4` claiming mode is part of a cache-trust condition the code does not check | owner decision on the `unsafe` surface | `handoffs/DC-87-windows-mutation/narrow-round-ruling-v1.md` |
| 29 | **DC-89** — platform claim documentation accuracy | **ACCEPTED 2026-08-10 — small, mechanical, cleared to implement directly.** Since DC-81 merged, the reference docs have told readers mutation is Linux-only, in two distinct false forms — a capability claim (`architecture.md:106`/`:132`, `durability-recovery.md:19`) and an evidence claim ("exercised by project gates on Linux only", six more pages), the latter also false because DC-81 added the macOS mutation CI job. **Most of the incorrect text is the architect's own.** Arises from the DC-87 mode-shape review, where **the architect's accept condition named one file and the claim turned out to live in eight places across seven more** — the second time this cycle the architect checked the file in front of them and generalized. **Criterion 3 requires the affected set be derived independently, not worked from the architect's list** — and it worked: the developer found a twelfth site (`durability-recovery.md:91`) the RFC missed, and flagged that the RFC's prose count ("eight") contradicted its own table (eleven). Seven-file correction **accepted at `b0a66ea`**; **criterion 1 amended by the architect** to cover all user-facing documentation, because it had wrongly excluded `README.md` — the third narrow-scoping error in this chain. **COMPLETE, merged 2026-08-10** at `f8b812d` after green CI. README amendment delivered at `35b965f` (`:105`/`:138` left alone as true prebuilt-binary claims, per criterion 5). **Two stale `ci.yml` comments (`:48`, `:92`) reported and deliberately left** — no risk, so no `FINDINGS.md` row; they go to DC-87 Stage 2, which must touch that file anyway | nothing | `handoffs/DC-89-platform-claim-docs-accuracy/implementation-review-v1-amendment-accepted.md` |
| 32 | **DC-92** — lineage replay memoization | **ACCEPTED 2026-08-11**, on the owner's selection of the O(N³) `verify` cost as the next theme. Re-derived from source: `verify_v2_lineage_roots` loops a block's whole lineage and calls `replay_with_appended_patches` per entry, which itself walks to genesis and replays from scratch — **no cache anywhere on that path** (DC-64's serves the commit path, not this one), giving O(i²) per block and O(N³) summed. **The architect found something the finding does not record**: `derive_next_state_root` has three production callers and two are **not** `verify` — `seal.rs:156` and `merge_execute.rs:165` — so every seal re-verifies the entire ancestor chain, plausibly **O(N²) per seal**. Stated as a control-flow hypothesis requiring measurement, not asserted. If it holds, **NFR-PERF-01's evidence has a blind spot**: DC-59's harness times `commit` and leaves every seal explicitly untimed. **Criterion 3 is the one that matters**: corruption injected at genesis, mid-chain and tip must still be caught, because the failure mode of caching a verifier is that it quietly stops verifying. **Implemented at `d4ecf66`, reviewed 2026-08-11 — accepted on one condition.** Two-layer memo takes `verify` from O(N³) to **O(N)**, architect-reproduced independently (ratios 1.97/1.96/1.95/1.99/1.99 — flat 2x; 2.72 s at N=160 against 46.4 s before), seal from O(N²) to near-flat. Negative controls **verified non-confounded** by disabling shape validation: only the shape test failed, all three root tests still passed. **Condition**: `LineageStateMemo` is never evicted and each entry clones a full `NodeLifecycleState` *plus* a `TextCache` of materialized file contents — unbounded by construction, and the churn harness measures time only on a fixed small tree, so it cannot see it. Before, `verify` was slow on a large repository; after, it may not complete. **Measured at `c0f3734` and ruled 2026-08-11: bilinear O(N × tree_size) — 599 MB at N=160/10,000 files, fitted line north of 37 GB at N=10,000, and the `TextCache` content term excluded because churn never edits text, so that figure is a floor. DC-92 does not merge as it stands**; criterion 7 amended to require a bound. Next round evaluates **topological iteration of `verify`'s outer loop**, which would need only the lineage frontier rather than evicting entries — the architect's observation that lead (a)'s extra pass is O(N) against an O(N³)→O(N) improvement already banked, so the arithmetic is more favourable than the report framed it. Lead (b) is dead as a primary route by the developer's own analysis: it caps only a term the measurement already excludes. **Topological bound implemented at `4bb851d` and the end-to-end control at `ca7ef74`; both accepted — DC-92 is ready for its three-platform run and merge.** `verify` O(N³)→O(N), `seal` O(N²)-per-call→near-flat, peak memory **599 MB → 15.1 MB** bounded by lineage frontier rather than history length. **The architect's probe found that disabling block state verification entirely left the whole suite green — a coverage hole that predates DC-92, proved by the same probe against pre-DC-92 `main`** — now closed for this path by `verify_repository_detects_block_with_state_root_mismatch`, and registered as a wider unowned finding. **CI FAILED at `ca7ef74` 2026-08-11 — does not merge**: `non-linux build` on both macOS and Windows, eight unused-import/dead-constant errors in the benchmark harness, whose `/proc`-based memory axis has carried `#[cfg(target_os = "linux")]` since `c0f3734` while its constants and one import did not. **Rule 9's cross-target clippy reproduces it exactly and was run by neither side** — the developer's gate summaries carried "no `cfg(target_os)` code touched" forward across three rounds after it stopped being true, and **the architect accepted that assertion instead of checking the condition**, having run the same gate on other increments precisely because their reports said it applied. **Fixed at `d2c226c` (10 lines, test-only), architect re-ran the cross-target gate — 8 errors per target → 0 — and CI came back green on all eight jobs. COMPLETE, merged 2026-08-11** at `b718623` | nothing | `handoffs/DC-92-lineage-replay-memoization/ci-failure-report-v1.md` |
| 36 | **RFC 100** — RFC naming alignment | **PROPOSED 2026-08-11**, on the owner's observation that the current names are messy. RFC-000 prescribes `NNN-slug.md`; this project used `DC-N`/`PR-NNN` — two prefixes, two padding widths, and **`DC-09`…`DC-30` numerically duplicating `PR-009`…`PR-030`**. **New RFCs start at 100**, not 001: `001` is taken by `PR-001`, and RFC-000 forbids reuse. **Existing names are frozen** — de-prefixing them would collide in 009–030, and resolving that means renumbering, which RFC-000 names as an anti-pattern that silently breaks external references (this queue, `FINDINGS.md`, handoff dirs, review records, commit history). Rule: **a prefix means legacy, a bare number means current.** Also verifies `rfcs/README.md`'s index against the directory. **ACCEPTED and implemented by the architect 2026-08-11** — the RFC index is an architect-maintained artifact, like this file. **The audit found three divergences, two of them the architect's own from this session**: ten RFCs (DC-87-DC-95, 100) never added to the index, and a dangling `DC-52` link left pointing at `proposed/` after archiving it the same day — RFC-000's "letting cross-references rot" anti-pattern, committed hours after quoting it. Both fixed; index now complete in both directions. **The third is systemic and deliberately not fixed here**: 18 RFCs recorded COMPLETE still sit in `accepted/` rather than `done/`, registered in `FINDINGS.md` as its own increment | nothing | — |
| 35 | **DC-95** — verify coverage and finding accumulation | **ACCEPTED 2026-08-11.** The two verifiability findings from the DC-92 cycle, as one increment in **two strictly-ordered stages**. **Stage 1 (coverage)**: nothing proves `verify` state-verifies blocks end to end — the architect disabled the wiring twice, pre- and post-DC-92, and the whole suite passed both times; DC-92 closed one path, the general question is open. **Stage 2 (accumulation)**: `verify` reports only the first hard error, so a damaged repository takes N runs to enumerate N defects. **Stage 1 precedes Stage 2 and that ordering is the point** — Stage 2 rewires error handling throughout `verify`, and doing that on a suite that cannot detect a check going missing is how a verifier loses a check during a refactor. **The architect ranks this above DC-93/DC-94**: those are release tooling, this is the product's central claim | nothing — **§3's four questions precede either stage** | `handoffs/DC-95-verify-coverage-and-finding-accumulation/prerequisite-handoff-v1.md` |
| 33 | **DC-93** — release policy Python retirement | **ACCEPTED 2026-08-11 — subtraction, on the owner's complexity concern.** DC-45 made Rust authoritative 2026-07-21 and retained the Python as a rollback path; three weeks on **nothing invokes it** (grep-confirmed across workflows and scripts) and the tree still carries **18 files / 2,895 lines**. The sharper cost: the authoritative Rust tool carries its own **Python-recognition path** (`command_scan.rs:124-149` plus four accepted invocation spellings in the command inventory) purely because the Python exists — so a security-sensitive command scanner is Python-aware for no live reason. **Git is the rollback path**; removal is from the working tree, not from history. **Not as pure as it sounds**: five of the 18 are oracle *tooling*, and whether the manifest the Rust tool reads is frozen or regenerable decides whether their generator is in scope — §3.1 | nothing — **§3's four questions precede removal** | `handoffs/DC-93-release-policy-python-retirement/prerequisite-handoff-v1.md` |
| 34 | **DC-94** — responsibility map executable binding | **ACCEPTED 2026-08-11 — DC-52's obligations 1 and 2, decoupled.** Binds a 50-entry prose map to an executed check registry so divergence fails closed, and makes the `defaults.run` invariant explicit rather than assumed. Same shape as DC-51's placement gate and DC-90's unsafe boundary: a rule a document asserts and nothing enforces. **Gates nothing and is gated by nothing** — the coupling that made these preconditions for retirement is withdrawn. `responsibility.rs` already exists (55 lines), so §3.1 may find this largely discharged, which is **a complete and useful outcome** | nothing — **§3's four questions precede design** | `handoffs/DC-94-responsibility-map-executable-binding/prerequisite-handoff-v1.md` |
| 31 | **DC-91** — publication record shape | **ACCEPTED 2026-08-11 — an evaluation, not a commitment.** One question: does a fixed-name, slot-based durable publication record have **independent value on POSIX** (fewer reachable crash states, less dependence on directory-sync ordering, self-describing recovery), or is it purely a Windows tax? If the former, DC-87's option 1 stops being a platform cost and becomes an improvement that also unblocks one; if the latter, Windows mutation is a separate and cheaper decision. **A "no" is a complete, successful outcome** and settles an open question either way. Touches the most safety-critical machinery in the product, so §4.4 asks for blast radius in terms of what would have to be re-proved. **The owner's robustness-over-minimalism direction is the deciding *criterion*, not the answer** — it settles how to weigh the comparison and overrules the architect's original do-not-disturb-proved-machinery bias, but not which design wins, since the comparison has not been made. That distinction is why the increment runs rather than being closed by the direction. **COMPLETE, answered and ruled 2026-08-11: partial — real value, narrowly scoped.** Two on-disk names collapse to one and the `CANDIDATE-DEBRIS` state class disappears with a genuine detectability gain; but `POINTER-LEADS-LOG` (arguably DC-38's central concern) is untouched, and recoverability cannot exceed an already-audited 24/24 ceiling. **The decisive finding is the developer's**: a pointer-only slot record makes routine seals to existing refs Windows-achievable but **does not unblock new branch/tag creation at all**, since the log's first record is created in the same transaction. **Architect's generalisation: any design keeping per-ref files has a first-appearance problem at ref creation.** Architect recommends **against** restructuring publication for Windows' sake — payoff partial, cost full — leaving DC-87 Stage 2 a live owner decision with its unblocking condition now answered rather than outstanding | nothing | `handoffs/DC-91-publication-record-shape/prerequisite-ruling-v1.md` |
| 30 | **DC-90** — unsafe code boundary and gate | **ACCEPTED 2026-08-10.** Turns the owner's ruling (*"`unsafe` is allowed under control with safety and maintainability preserved"*) into a checked property rather than an intention, in the shape DC-51's placement gate already established: one crate may omit `forbid(unsafe_code)`, named in an allowlist, unable to depend on any product crate, with its own third-party allowlist so the exception is not a side-door around DC-51. **Must land before the first `unsafe` line** — a boundary added afterwards documents what happened instead of constraining it, the same ordering argument DC-82 used against DC-81. Does not block DC-88 and is not blocked by it. **Architect's §4.2 baseline measurement, with its own false start recorded**: a first grep for `lints.workspace` returned zero and read as "the lint table is inert" — wrong; the real form is `[lints]` + `workspace = true`, and **all eight members carry it**. One real asymmetry surfaced: `prikk-crypto/src/lib.rs` has no source-level `#![forbid]` and is covered by manifest inheritance alone, so manifest and source disagree in presentation while agreeing in effect — which of the two the gate treats as authoritative is a genuine design decision. **Prerequisites answered and ruled 2026-08-10.** Headline: the expensive half needs no building — `clippy::undocumented_unsafe_blocks` already enforces the SAFETY-comment rule, found by testing rather than reasoning. **Architect's correction**: the gate set runs clippy but **not this lint** — it is `restriction`-group, allow-by-default, and `-D warnings` does not reach it (probed independently, zero matches). **Consequence nobody had drawn**: enabling it in the exception crate's own manifest would let the one crate permitted to write `unsafe` switch off its own guard with a one-line edit. Ruled: the lint goes in the root `[workspace.lints.clippy]` table, and **dropping workspace lint inheritance is itself a gate failure** — that is what makes the arrangement self-guarding. Manifest is authoritative; `prikk-crypto`'s missing source attribute needs no change. **Implemented at `baa4b38` and reviewed 2026-08-10 — accepted on one condition**: the lint was set to `"deny"`, which a source-level `#![allow(...)]` silently overrides, so the exempt crate could satisfy the new gate and then switch the guard off in its own `lib.rs` with every gate still passing. **Architect built the bypass and its fix**: manifest `deny` + source `allow` → lint never fires, clippy exits 0; manifest `forbid` + source `allow` → `error[E0453] ... incompatible with previous forbid`. The increment already knew the principle — it is why `unsafe_code` is `forbid` — and applied it to the guarded thing but not the guard. **Fixed at `f358353` and accepted**: the escape route is now a hard `E0453` compile error, which the architect verified **end to end on a real member crate** rather than on the isolated probe, and reverting the constant to `"deny"` fails eight tests. **COMPLETE, merged 2026-08-11** at `2bad097` after green CI. The escape route is a hard `E0453` on merged `main`, verified end to end against a real member crate | nothing | `handoffs/DC-90-unsafe-code-boundary-gate/implementation-review-v1-condition-accepted.md` |
| 28 | **DC-88** — durability contract requirement shape | **ACCEPTED 2026-08-10, with the scope trade taken as stated — DC-87 Stage 2 waits for this.** Does `durable_directory_entry` state a requirement or a primitive? DC-76's own thesis is "guarantee, not syscall"; this is the one method that misses its own bar, and Windows is where it bites. **Arises from the architect's own error** in blocking DC-87 Stage 2 on an unanswerable Windows API question instead of on prikk's contract. Accepting it blocks DC-87 Stage 2 until it lands; the alternative was shipping Windows with a documented weaker crash invariant; the owner accepted the slower, stronger path. **Prerequisites answered and ruled 2026-08-10, and the result collapsed the increment**: `durable_directory_entry` has exactly two callers (both `worktree.rs`, both wanting per-entry confirmation), and **DC-38 never calls it** — its three durability-bearing steps go to `atomic_replace`/`promote`/`durable_append`, each of which bundles its own directory sync. **COMPLETE, merged 2026-08-11** at `fe6d5e0` after a green three-platform run on `ed04c21` — a parameter restatement plus two one-line caller edits; architect's negative control (reverting the implementor) fails **five** tests, four of them pre-existing caller-level ones, and a repository-root-file probe confirmed the `required_parent` change is behaviour-identical to the old `unwrap_or(Path::new(""))`. **Two architect corrections follow**: the scope trade sold to the owner ("this blocks Stage 2") was **mispriced on a false premise** — the two are orthogonal, so Stage 2 is no longer blocked on this and **Stage 1's seam is released from hold**; and the Windows blocker is real but **located in the three transition methods, not here** — with the contract already permitting any implementation of them, so no amendment is needed to attempt it. §3's two-slot sketch reassigned to Stage 2 | nothing — **cleared to implement** | `handoffs/DC-88-durability-contract-requirement-shape/prerequisite-ruling-v1.md` |
| 37 | **RFC 101** — first-appearance durability | **CLOSED 2026-08-12 with a negative result; superseded by RFC 102. No code produced.** Accepted and closed the same day. Its own §5.2 transition trace disproved its §1 problem statement — **prikk is content-addressed, so every object write creates a new name**, making this a storage-model problem, not a ref-publication one; the fix as scoped would have produced a durable ref pointing at a non-durable object. §5.5 added Transactional NTFS to DC-87 Stage 2's Win32 survey and ruled it unusable: withdrawal would silently void the guarantee rather than break detectably. **Survives its closure:** §5.2's fifteen-transition table and 31-site call index (RFC 102's primary input), and three `FINDINGS.md` rows. Originally proposed 2026-08-12 on the owner's direction:, on the owner's direction of the same date: **Windows mutation at parity with Linux.** DC-87 Stage 2 established that Windows cannot make a *newly-created name* durable, breaking DC-38's invariant at step 5 while step 6 remains achievable; DC-91 then established that **no per-ref file shape avoids this at ref creation**, so the pointer's shape was never the obstacle. The direction: route every durability-bearing transition through a name that already exists, making per-ref pointer and log files **replayable consequences** rather than durable steps. Verified 2026-08-12 and load-bearing for the estimate — the active WAL is a *fixed* path (`layout.rs:161`) and cleanup **truncates rather than deletes** it (`active.rs:147`), correcting the architect's own same-day claim that WAL cleanup would reintroduce first-appearance. **Supersedes the scope of DC-91 §5's recommendation against restructuring**, which was scoped to a partial Windows payoff, not to parity. §5's six prerequisites precede any design, and a stop-and-report on any of them ends the RFC — the same standing DC-91 had. Largest increment in flight; 0.20.0 moves. **Acceptance clears §5's investigations only — no design, no implementation, no production code.** | closed — nothing may begin | `handoffs/101-first-appearance-durability/prerequisite-5.5-ruling-v1.md` (closure), `prerequisite-5.1-5.3-ruling-v1.md` (the disproof) |
| 38 | **RFC 102** — container-based durability | **ACCEPTED by the project owner 2026-08-12**, successor to RFC 101 on the owner's ruling that Windows read-only is not an acceptable permanent state. **Moves durability-bearing repository state into a bounded set of fixed-name container files**, so no new directory entry sits on the durability path — appending to an already-named file needs only content durability, which Windows provides via `FlushFileBuffers`. The only route to parity identified that needs **no vendor-specific primitive, no deprecated API, and no weakened invariant**, and it is uniform across platforms rather than Windows-only. **The worktree cannot be containerized** — §4 proposes a fixed-name unclean-shutdown marker so prikk refuses to infer deletion from absence, converting T12's silent signed data loss into a detected condition. **This is a hypothesis, not a design**, and gets the same treatment that killed 101: §6's six prerequisites first, stop-and-report available. **1.0-scale**; explicitly not 0.20.0. **Acceptance clears §6's investigations only.** | nothing — **cleared to answer §6's six prerequisites** | `handoffs/102-container-based-durability/prerequisite-handoff-v1.md` |

Each handoff for a *proposed* RFC states at its head that implementation may not begin until that RFC is
accepted. Preparing the handoff is not authorization; it removes everything except the design gate.

**DC-41 is complete** — all four stages committed (crash matrix `fb4153c`, hash vectors `d5bd096`, hash
differential `540d4db`, property/fuzz accepted 2026-07-28). Its descoped platform matrix is DC-49 and is
not a DC-41 completion condition.

**DC-54 is complete** — accepted, implemented at `e8f780a`, post-commit review accepted 2026-07-28. It
closed the encode/decode path asymmetry found by DC-41 stage 4's campaign.

**DC-51 is complete** — accepted `d7d49c6`, implemented `d3e939b`, post-commit review accepted with one
blocking finding, repaired `4c8b7a3`. Dependency placement is now mechanically enforced.

**DC-50 is closed** — closed at `4005efb` with a **replace** decision. Its record is at
`handoffs/DC-50-first-party-sha256-roi-decision/decision-record-v1.md`. It stays in `rfcs/accepted/`
rather than `done/` because `done/` means shipped and DC-50 ships nothing; being a decision-only
increment, it will never move. DC-50 produced no code and authorized exactly one successor: DC-55.

**DC-55 is complete** — accepted `a01e628`, swap implemented `8c84bc4`, fixture repairs `083d6c0` and
`753ebab`. Implementation review v1 returned one blocking finding (a fixture depending on directories git
cannot store, which broke `cargo test --workspace --locked` on the committed tree); repaired and accepted
at re-review v1 on 2026-07-29, verified by fresh clone with a negative control. `prikk-hash::sha256` now
runs on `sha2`, with the outgoing first-party implementation retained test-only as the differential's
permanent independent reference.

**DC-42 is superseded** — archived 2026-07-29 into **DC-56** (NFR-PERF-01), **DC-57** (NFR-PERF-02), and
**DC-58** (source-structure audit). Design review found it bundled three unrelated increments against
standing rule 2. Never implemented. See `rfcs/archive/DC-42-PERFORMANCE-MAINTAINABILITY-GATES.md`.

**Why this order.** Rows 1–3 are history now: DC-58, DC-61, and DC-56 are all landed, and the reconciliation
DC-61 needed with DC-63's kind branch was carried out as part of its 18-call-site schema threading.
**DC-64 is the only live development candidate**, and it is where NFR-PERF-01 — a missed product-M1 gate,
outstanding since before 0.17.7 — actually gets closed or is reported as inherent. **Accepted 2026-07-31
and cleared to start.** Design review measured its target to the operation (~40 µs each, 97.6% of the phase
in `replay_lineage`) and eliminated a keyed-cache route that could never have hit, given the one-record WAL
cap. Reporting the cost as inherent is a permitted outcome. DC-52 and DC-43 are **not** in this lane — both are release-blocked, see §2.

**DC-56's owner decision is settled.** Ruled 2026-07-30: NFR-PERF-01 bounds **steady-state** commit cost,
not every commit. That resolves its conflict with NFR-PERF-04 — which blesses rebuildable indexes while a
strict reading would forbid building one — and selects a changed-path index. The ruling carries a binding
obligation: **DC-56 must specify cache validity** (when the index is trusted, what invalidates it, what
bounds rebuild frequency), because an unbounded cold path satisfies the letter and defeats the requirement.
DC-56 is accepted and has no remaining blockers.

**DC-56 grew a second objective.** Design review v2 found the commit path does not merely *traverse* the
worktree — `worktree_files.rs:11-14` stores `bytes: Vec<u8>` per file, so every commit reads the whole
worktree into memory, O(total worktree bytes) regardless of change size. No requirement names that; it is
recorded in `MILESTONES.md` as an untracked scalability defect. The same changed-path index fixes both
objectives, but evidencing the memory one needed a memory axis, opened as **DC-62** rather than folded back
into DC-59 — DC-59 was complete and its criteria all discharged, so adding one would have retroactively
unfinished it. **DC-62 is now complete (`07b1fc8`), so this precondition is satisfied** — including the
floor and delta column DC-56's criterion 5 compares against, so no harness work remains in DC-56's scope.

**Beware the milestone labels.** `MILESTONES.md` § "Two milestone schemes" is required reading before
resolving any `M0`–`M3` gate label in the NFR matrix. The requirements use the product scheme; this file
and `MILESTONES.md` use the corrective one. The collision already caused one architect review to conclude
that overdue work was not yet due.

**On DC-55's review independence.** Its design review was an author re-examination: this project has one
architect, so independent design review is not achievable for a design the architect wrote. That is the
defined process — the organization document's Phase 2 gate assigns design review to the high-capability
model without distinguishing the two — rather than a deviation from it. The limitation is real regardless,
and DC-55 shows both sides of it: the author review found a genuine blocking defect, *and* a second
blocking defect survived into the implementation and was caught only because acceptance criteria had been
written to be reproducible from the repository rather than trusted from a report. Keep that pattern for
identity-bearing increments.

**DC-62 is complete** — accepted `5ff2388`, implemented `963caae`, **N1 repaired at `07b1fc8`** and that
repair reviewed and accepted with no findings. Peak-memory measurement added as a separate pass, no new
dependency, no production code. **DC-56's precondition is satisfied.** The measurement confirms O(total
worktree bytes): above the measured 6,144 KB floor, memory grows **9.92x** for a 10x repository-size increase
at fixed change count, where absolute VmHWM grows only 2.58x. The report now publishes an "Above floor"
column, so that is the figure a reader sees and the one DC-56's criterion 5 compares against.

**And N1 was repaired under DC-62, correctly, against my written instruction.** I had assigned the fix to
DC-56's scope on the DC-59 → DC-62 precedent above ("adding a criterion would have retroactively unfinished
it"). That precedent governs **new scope arising later**; it does not govern **a review finding against the
increment itself**, which is repaired under that increment — as DC-55's fixture blocker and DC-51's
`reference-check` break were. The developers did it under DC-62 and were right. **Rule:** the DC-59 → DC-62
split applies to new scope, not to findings; a finding against an increment reopens that increment.

**DC-63 is complete** — accepted `c7d1691`, held briefly on two `refs.rs` blockers, cleared via handoff v2,
implemented `6b33a72`. Implementation review v1 accepted with no blocking findings; gates re-run by the
architect in a detached worktree (790 tests, 0 failures). **Requirements §6.6 is closed**, and `RefKind::Tag`
has production call sites for the first time since the ref model was built. Both blockers are fixed in the ref
system's core: kind-aware ref-name validation in `validate_coherent_publication`, and one shared
`ensure_ref_target_valid` helper resolving the tag indirection at both verification sites.

**DC-60 is complete** — accepted `994bf32`, scope amended the same day to `list` + `create`, implemented
`6c2b7a6`. Implementation review v1 accepted with no blocking findings; gates re-run by the architect in a
detached worktree (779 tests, 0 failures). Its `branch create` publishes a byte-equivalent DC-13 genesis
shape, asserted by test. Deletion is DC-61.

**DC-58 batches 1 and 2 are accepted** — `e1d0213` and `54a3037`, implementation reviews accepted with no
blocking findings. All four remaining over-500 files were resolved: three split, and `lifecycle_cache.rs`
reduced from 974 to 117 implementation lines by moving 848 lines of already-test-only trust-ladder
scaffolding into a whole-module-gated `cache_ladder.rs`. That reclassification was ruled in scope — a
non-test and a release build both still compile, proving nothing production-reachable moved behind the
gate — but must be reported separately from the three genuine splits, which is the only item outstanding.
Two permanent by-design exceptions stand: `node_authoring.rs` deferred while DC-56 is open, and
`frozen_outgoing.rs` excluded as DC-55's immutable reference.

**DC-59 is complete** — implemented `a9c2fe0`, accepted 2026-07-29. Its report measured the full-tree
scan: 4.22 ms at 10 files rising to 516 ms at 10,000, with the change set fixed at one file throughout.
The scan is now evidence rather than inference, and DC-56's precondition is satisfied.

**DC-57 is HELD** — its premise does not hold and its handoff is **withdrawn**. The active WAL is
structurally capped at one record repository-wide, so 800/1000 is unreachable and its boundary tests
cannot be constructed. NFR-PERF-02 presupposes multi-commit queued active sessions, which
`rfcs/IMPLEMENTATION-STATUS.md:464` records as not implemented. Found by the dev team stopping at handoff
Step 1 as instructed. **Blocked on an owner decision** — see `MILESTONES.md` finding
"Multi-patch active blocks not implemented".

## 2. Blocked on a release-lane event

| Increment | Blocked by | Handoff (written, marked BLOCKED) |
|---|---|---|
| **DC-49** — portable-logic platform matrix | The M1 public portability-claim correction, which `MILESTONES.md` places inside the mandatory hold of an **activated** release. Cannot complete while the lane is parked. | `handoffs/DC-49-portable-logic-platform-matrix/implementation-handoff-v1.md` |
| **DC-43** — release security and distribution controls | Its scope *is* release security and distribution, and `DC-35:255-257` hands it key custody, rotation, expiry/revocation monitoring, attestations, and SBOMs. DC-35 needs a fitness amendment, so designing DC-43 now designs against a foundation about to change. **Moved here 2026-07-30.** | `handoffs/DC-43-release-security-controls/implementation-handoff-v1.md` |

**DC-52 left this section 2026-08-08** — `DC-45:419`'s condition is discharged by 0.19.0 and its
accepted stability rerun. It is available now; deletion is still a separate architect-reviewed change.

**DC-74 does not belong here and must not be moved here.** It carries a *release condition* — buildable
and mergeable now, not releasable until a named condition holds. That is the inverse of this section,
which lists increments blocked *from being built*. See `MILESTONES.md`, "Attached release conditions".

Release stabilization is deferred by owner direction 2026-07-30, so everything in this section is
dormant. Note that **three** increments sit here, not one — DC-52 and DC-43 were previously listed in §1 as
available now, which understated how much of the backlog is release-gated.

This was the one place where a development increment depends on a release-lane event. It was descoped from
DC-41 for exactly that reason. If the owner would rather unblock it sooner, the alternative is a reviewed
decision to move the documentation correction into the development lane — that is an owner decision, not
an implementation one.

## 3. Release lane — only on explicit owner activation

Not startable by a developer. Recorded so the sequence is visible.

1. Activation commit — lane `active` plus exact target version, in all three authorities, atomically.
2. DC-35 signer bootstrap as an isolated public governance transaction.
3. Mandatory public 72-hour hold.
4. During the hold: literal DC-38 stale-pointer/ahead-log reproduction; DC-37-aligned portability/
   requirements correction (this is what unblocks DC-49).
5. Explicit architect/security hold-lift ruling.
6. Combined release candidate: full gates, corrective failpoint matrix, adversarial RC review.

**Gate inheritance:** release conditions attach to accepted-but-unshipped *increments*, not to version
labels. DC-39, DC-40, and DC-41 are on `main` and unshipped, so whichever release ships first inherits the
complete M1 sequence regardless of what it is numbered.

## 4. Scheduled later

These two have **design briefs**, not implementation handoffs — their detailed design does not exist yet,
and their own RFCs defer it to design review. The brief specifies what the design stage must produce, so
design starts from a defined target. An implementation handoff follows once each design is accepted.

| Increment | Milestone | Design brief | Note |
|---|---|---|---|
| **DC-44** — migration, backup, restore evidence | M3 | `handoffs/DC-44-migration-backup-restore-evidence/design-brief-v1.md` | Owns NFR-REL-03; decides what happens to existing format-1 repositories |
| **DC-53** — repository-wide AUTHOR trust verification | Post-M2, unscheduled | `handoffs/DC-53-repository-wide-author-trust-verification/design-brief-v1.md` | Capability gap, not an evidence gap; identity-adjacent, needs a companion design document with vectors |

## 5. Unscheduled, deliberately

- **Key lifecycle** — rotation, revocation, expiration, thresholds above one, hardware signing, remote
  trust distribution. Explicitly out of scope for every current RFC. Needs its own increment before any
  publication-grade trust claim.
- **Cosmetic marker diagnostic** — unknown/malformed `.prikk/FORMAT` reports `unsupported format version:
  0`, where `0` is a sentinel rather than the offending value. Fails closed correctly. A non-blocking
  pre-RC correction candidate; not a prerequisite unless selected.

## 6. Standing rules for every increment

These apply to all work above and are not restated in each handoff.

1. **Design-first.** A proposed RFC is not implementation authority. It must move to `rfcs/accepted/`
   through its own design review first. Requirements → external design → internal design → program design
   are the architect's; implementation and testing are the developers'.
2. **One increment per candidate.** No bundling. Multi-stage increments land one stage per review.
3. **A finding is never a test expectation.** Any behaviour defect discovered opens its own corrective RFC
   with a minimized reproducer. This matters most in DC-41 stage 4, where randomized decoder input is
   where something will plausibly be found — a malformed-input panic is an NFR-SEC-04 defect, and finding
   one is a success for the campaign, not a failure of the stage.
4. **Frozen identities are verified every review.** Current baselines: `Cargo.lock`
   `601d0678b8481a750519e64bb19f66f8532301b4157d8353d8d9211261c5da31` (re-frozen at DC-41 **stage 4**,
   which added `proptest`; this supersedes stage 3's `18a8b40a…`, which itself superseded `0cd51cbd…`),
   oracle manifest `2f0c54ab…`, `release-signers.toml` `f8d56841…`, both command inventories. Any
   intentional change is a reviewed re-freeze whose new hash supersedes the old.
5. **These are review-gated policy/identity artifacts, not refactorable code.** Changing any of them is a
   policy change requiring its own review: `command_scan/procedure.rs` (accepted command productions),
   `command_scan/prefix.rs` (prefix grammar), `reference.rs` (authority descriptors), `format.rs`
   (format-2 schema allowlist), `state_root.rs` (state-root byte grammar).
6. **Never spell out the full command form in prose.** Write "release-policy `check`", not the
   full `cargo run --locked -p prikk-release-policy` invocation with the bare subcommand spelled out
   after `--` — that full form is a recognised policy invocation, so any scanned `.md` file containing
   it must be registered in the command inventory or `reference-check` fails. DC-51's own evidence note
   tripped this. `boundary-check` and `reference-check` are safe; only the bare subcommand word
   triggers it.
7. **Dependency placement is now mechanically enforced.** DC-51's `boundary-check` category
   `dependency-placement` catches a third-party crate misplaced into a product crate's
   `[dependencies]`, including under `[target.*]` and via `package =` renaming. Review-only
   verification is defense-in-depth going forward, not the primary control.
8. **Governed procedure files.** `.github/workflows/ci.yml` and any `.sh`/`.yml` under `.github`,
   `scripts`, or `release` are scanned default-closed. Every `run:` command must match an accepted
   production, or `boundary-check`/`reference-check` fail. Adding a CI command means a reviewed classifier
   amendment in the same increment.
9. **Gate set for every candidate.** `cargo fmt --all -- --check`;
   `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`;
   `cargo test --workspace --locked`; `cargo +1.85.0 test --workspace --locked`;
   `cargo +1.85.0 check --workspace --all-targets --locked` (RFC 126 §5 — added 2026-09-03: the
   MSRV *test* command does not compile `[[bench]]` targets, proved by planting a syntax error in
   `tools/benchmarks/benches/commit.rs` and watching `cargo +1.85.0 test --workspace --locked`
   exit `0`. CI's `msrv-1.85.0` job already ran `--all-targets`, so nothing shipped broken — the
   *local* set was the weaker one, and this closes that gap rather than fixing a live defect);
   `git diff --check`;
   `cargo audit --no-fetch`; `RUSTDOCFLAGS="-D rustdoc::private_intra_doc_links" cargo doc
   --workspace --no-deps` (RFC 126 §4 — added 2026-09-02 after this exact lint sat unchecked at 7
   live warnings, since `cargo doc` had never run anywhere, local gate set or CI, before then);
   release-policy `check`, `boundary-check`, `reference-check`. Use a repository-local `TMPDIR`
   (`.git-exclude/tmp`) where `/tmp` is read-only.

   **Additionally, for any increment touching `#[cfg(target_os)]`-gated code** — added 2026-08-09 after
   DC-76 broke the non-Linux CI job while passing all nine gates above:
   `cargo clippy --workspace --all-targets --all-features --locked --target x86_64-pc-windows-gnu -- -D warnings`
   and the same with `--target x86_64-apple-darwin`. CI's `non-linux build` job runs this natively on both
   platforms, so an increment that skips it can pass every canonical gate and still turn CI red. The nine
   above cannot see platform-conditional dead code, which is the exact failure DC-71 exists to prevent.

   **AMENDED 2026-09-09 — every report must state this rule's outcome, not silently omit it.** The rule
   was skipped by three consecutive RFC 133 rounds and the architect accepted all three, leaving `main`
   red on the macOS and Windows clippy jobs from `11871bfd` to `61da6987`. The first round declared its
   two `#[cfg(target_os)]` uses under its own "checked per this round's own diff" heading and did not run
   the two commands that declaration triggers; the next two **did not mention `cfg` at all** while each
   added ~250 lines to the same gated file. Nothing in any report was false — the skip was simply
   invisible.

   **So: state the two cross-target results explicitly, or state that the rule does not apply and why.**
   "Touching cfg-gated code" includes **adding un-gated code to a file that already contains cfg gating** —
   that is precisely how platform-conditional dead code appears, and it adds no `cfg` line to the diff for
   anyone to notice. A round that says nothing has not established that the rule did not apply.

   **DEMONSTRATED 2026-09-10; and DEMONSTRATED AS INSUFFICIENT 2026-09-12: it compiles platform code, it does not run Windows tests — a change to the key path or to the test isolation seam is confirmed by CI's Windows job on the pushed commit before the next cut (RFC 148 v3: `APPDATA` unredirected, three tests red on `main`) at `72f8c60f` (RFC 131 §6d.8) — the addendum bites, with a number.** Every
   prior report had shown it *passing*, which says nothing about whether it can fail. Breaking one
   narrowing deliberately in a detached worktree — `MacosDurability` one visibility level further —
   produced **3 errors under `--target x86_64-apple-darwin` (`E0364`, `E0432`, `E0603`) and 0 on the
   Linux host.** On a visibility change to `cfg(target_os)`-gated code, the two cross-target runs are not
   redundancy over the canonical set: **they are the only checks that compile the code at all.**
10. **Report counts before and after.** Test counts per touched crate, and locked package count where
    dependencies change, so no silent loss or growth can hide.

    **Baseline, re-measured 2026-08-18 at `aade1cc`, covering all nine workspace members:**
    `prikk-store` **723**, `prikk` (the CLI) **117**, `prikk-release-policy` **124**, `prikk-object` 80,
    `prikk-replay` 44, `prikk-hash` 14, `prikk-crypto` 7, `prikk-error` 0, `prikk-ffi` 0 (Windows-only
    code; 0 is the Linux figure, not an absence of tests); **180 locked packages**. Workspace total
    **1109**.

    > **`prikk-store` fell 738 → 723 since the previous baseline, and no tests were lost.** Reported by
    > the dev team in the DC-53 Stage 1 implementation report, which correctly declined to explain it.
    > Checked rather than assumed: `96d7e3d` measures **738** on demand, so the old figure was accurate
    > when written, and the workspace total rose **1083 → 1109**. The −15 is relocation across DC-87 and
    > DC-95 plus growth elsewhere. **The failure was that this baseline went un-updated across four
    > merged increments and two releases**, so every increment since 2026-08-16 compared against a figure
    > that no longer described `main`.
    >
    > **And the crate list was wrong again.** It named eight members; `prikk-ffi` — added by DC-96, the
    > workspace's only `unsafe` crate — was never entered. **That is the identical failure the blockquote
    > below already records**, recurring in this same paragraph after it was written down. A correction
    > that lives only as prose is not a control; RFC 105 made that argument about RFC 100's naming rule
    > and it applies here unchanged. **Whether this baseline belongs in `release-policy` rather than in
    > memory is an open question, raised 2026-08-18.**

    > **The crate list itself was wrong until 2026-08-15, and that is worse than the figures ever were.**
    > This baseline named **six of eight** workspace members. `prikk` — the CLI, with **113 tests** — and
    > `prikk-error` were never counted, so a rule whose stated purpose is that *"no silent loss or growth
    > can hide"* had an entire tested crate outside it. Found when RFC 102 Stage 6 Step 2 round 4 added
    > six CLI tests and reported, correctly and in passing, that `prikk-cli` *"never has been"* in this
    > table. **I re-measured and rewrote this baseline twice in one day without once asking whether the
    > list of crates was complete** — verifying every element while never verifying the set. Derive the
    > member list from `cargo metadata --no-deps`, not from this line's own history.
    >
    > **Corrected within the hour: `prikk` was written as 113 and is 107 at the commit this line names.**
    > The developer caught it by noticing that 113 matched *their branch* — which carries six CLI tests
    > `main` does not — and re-measured at `4ad0021` in a detached worktree. **I had measured in the
    > shared primary tree without checking which commit it was sitting on**, and labelled the result with
    > `main`'s hash. `prikk-store` 709 and `prikk-error` 0 were taken earlier at the right state and are
    > correct. **Measure baselines in a detached worktree at the named commit — never in the shared tree,
    > whose branch is not yours to assume.**

    > **The obligation below was missed on its first outing, by its own author.** RFC 102 Stage 5 moved
    > `prikk-store` from 688 to 703 across six rounds. Every round reported its own count correctly and
    > **none updated this line**, and I did not require it in any of the six reviews — having written the
    > same-commit rule the day before. Corrected here after the merge. The rule is not self-enforcing:
    > **a reviewer has to ask for it, every time, or it decays exactly as the pre-DC-55 baseline did.**

    **How to measure — this is not incidental, and it has now been got wrong three times.**
    `prikk-store` runs two tests a second time in isolation, so `cargo test -p prikk-store` emits three
    result lines: `1 passed; N filtered out`, the same again, and then the real
    `<count> passed; 0 filtered out`. Only the last is the count. No other crate in the workspace emits a
    filtered run, which is why this shows up nowhere else. Use exactly:

    ```
    cargo test -p <crate> --locked 2>&1 | grep -oE "ok\. [0-9]+ passed;.*; 0 filtered out" | awk '{s+=$2} END {print s}'
    ```

    **Two ways this has actually gone wrong, both mine, both caught by the developer:** summing every
    `test result:` line (counts the isolated runs twice), and then filtering on the bare substring
    `0 filtered out` — which **also matches `690 filtered out`**, re-admitting the very lines it was
    written to exclude. The separator in `; 0 filtered out` is load-bearing. The isolated runs also
    interleave their output with the main run, so the text is not reliably line-oriented; anchor on
    `ok\. N passed` rather than on field position.

    **Whoever changes these numbers must see this line updated.** Not a courtesy — the line *is* the
    comparison point, and a stale one makes the rule unfalsifiable.

    **But the developer cannot do it, and the rule spent a month pretending otherwise.** `rfcs/` is
    architect-only, so an increment that moves a count can *report* the new figure and nothing more.
    **The obligation is therefore the reviewer's: update this line as part of accepting the increment
    that moved it**, at the moment it merges — not at the moment it is reviewed. **Corrected 2026-08-15**:
    the original wording said "in the same review," which cannot be right, because this line describes
    `main` and a reviewed increment is not on `main` yet. Updating at review time would make the baseline
    describe a branch. Accept the increment, record its reported figures in the review, and update this
    line when the branch lands. That is the honest allocation, and it explains the
    decay this rule kept suffering — RFC 102 Stage 5 moved `prikk-store` by 15 across six rounds, every
    round reported its count correctly, and none could have edited this file if it had wanted to.
    Corrected 2026-08-15 after Stage 6 Step 1 flagged the figure rather than editing it, which was the
    only move available to it.

    > **Corrected 2026-08-15, and the correction is the lesson.** The previous baseline —
    > `prikk-store` 543, `prikk-object` 76, `prikk-crypto` 5, `prikk-release-policy` 59, 180 locked
    > packages — had been stale in **five of seven figures** since DC-55. DC-72, DC-75, DC-79, DC-80
    > and DC-90 each changed counts this line records; none updated it. So **+177 tests and a
    > one-package drop accumulated invisibly** — precisely the "silent loss or growth" the rule exists
    > to prevent. The rule did not fail because anyone skipped it; it failed because a hand-maintained
    > figure with no owner and no automated check decays, and every gate stayed green throughout. The
    > earlier `prikk-replay` 4→44 correction (DC-55) was the same decay caught once and patched
    > per-instance rather than structurally.
    >
    > The baseline is pinned to `f2edb11` rather than to `origin/main` because **`origin/main` did not
    > compile when this was measured** (see `FINDINGS.md`) — a count cannot be taken at a commit that
    > has no test binaries.
    >
    > **Corrected again the same day: `prikk-store` was recorded as 690 and is 688.** The replacement
    > baseline was itself wrong, by the measurement error the paragraph above now documents — three
    > result lines summed where two were isolated re-runs of tests already counted. Found by the
    > developer, who re-measured at `f2edb11` in an isolated worktree instead of citing the figure they
    > had been handed. **The lesson is not "baselines decay" but the narrower one: a number is only as
    > good as the command that produced it, and re-deriving a figure is not the same as repeating it.**
11. **Submit a review request per candidate** with the diff, an evidence note, gate output, and an explicit
    statement of what did *not* change.

## 7. Posture

Production suitability, repository-format stabilization, and public-preview readiness all remain
**no-go**. The five blocking findings from the independent architecture review are closed *in
implementation* (DC-36 through DC-40) but not *in release* — they close for a shipped artifact only when
the §3 sequence completes and an adversarial release-candidate review accepts the combined state.
