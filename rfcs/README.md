# Prikk RFCs

This directory stores Prikk design and implementation decision records.

The lifecycle policy is tracked as [RFC-000](./done/000-rfc-lifecycle-policy.md). RFC-000 is the
authority for what the `proposed/`, `accepted/`, `done/`, `archive/`, optional `draft/`, and
`handoffs/` directories mean.

## Lifecycle Summary

Prikk uses RFC-000's 5-folder variant:

- `proposed/` contains RFCs under design review; implementation should not start from these records.
- `accepted/` contains reviewed designs that may be implemented but have not yet released.
- `done/` contains implemented/released RFC records.
- `archive/` contains withdrawn, superseded, or historical umbrella RFCs that are no longer live
  implementation authority.
- `draft/` may be added later if shared pre-review drafts become useful.
- `handoffs/` contains companion execution/FDD handoff material. Handoffs do not define an independent
  lifecycle; their state follows the related RFC.

  **Every handoff should have a related RFC, and one does not.** `handoffs/consolidation/` holds
  `dead-surface-consolidation-handoff-v1.md` for work that was designed, implemented and merged with no
  governing RFC anywhere in `proposed/`, `accepted/`, `done/` or `archive/`. Found while deriving RFC
  105's allowlist, where it is entered for that reason rather than as a legacy-naming exemption. Recorded
  here because it is a **process** observation, not a naming one: a handoff's state is defined as
  following its RFC, so a handoff without one has no defined state. No retroactive RFC is being invented
  for it — that would assert a decision record that never existed.

RFC-000 says folder location is lifecycle authority. The status text inside each RFC should be kept
consistent with its folder.

### Naming (RFC 100)

New RFCs are `NNN-slug.md` — zero-padded number, lowercase hyphenated slug — **numbered sequentially
from 100**. New handoffs use the matching `handoffs/NNN-slug/`.

`100` rather than `001` because `001` is already `PR-001` and RFC-000 forbids reusing a number. The gap
at 096-099 is deliberate: it marks where the scheme changed.

**Legacy names are frozen.** `DC-*` and `PR-*` RFCs keep their filenames permanently — `DC-09`-`DC-30`
duplicate `PR-009`-`PR-030` numerically, so de-prefixing them would collide, and resolving that means
renumbering, which RFC-000 names as an anti-pattern. The rule a reader needs: **a prefix means legacy, a
bare number means current.**

## Proposed

These records are under design review. All proposed RFCs must respect the dependencies in
[`MILESTONES.md`](../MILESTONES.md).

| ID | Title | Milestone |
|---|---|---|
| 109 | [Agent-Native Interface](./proposed/109-agent-native-interface.md) | Unscheduled. Owner concept, 2026-08-18. Depends on criterion 3 (now met) and criterion 2 (open — it gates the AST-sealing question). **No design exists** |
| 110 | [Agent Safety and Provenance](./proposed/110-agent-safety-and-provenance.md) | Unscheduled. Owner concept, 2026-08-18, **direction not yet established** and recorded as such. Pillar 1 largely redistributes into RFC 108; Pillar 2 needs criterion 5 as a floor, since provenance sealed over unverified authorship manufactures the appearance of verification. **No design exists** |
| 113 | [History Import Foundations](./proposed/113-history-import-foundations.md) | Unscheduled. Owner direction 2026-08-19: migrate from Git, Subversion and CVS with history preserved. **Amended the same day — the decoder/encoder tooling becomes its own project**, because prikk's whole third-party runtime surface is five crates and a Git decoder needs `gix` or `libgit2`. **This RFC is therefore prikk's import *contract***, not an importer plan. Records the shared problem (prikk records node identity these systems never had; imported commits cannot carry valid prikk AUTHOR signatures — **DC-53's `Unverifiable` is already the right vocabulary**) and §3.1's three IR questions: what a record is, what must be preserved, what may be omitted. Depends on criteria 2 and 1. **No design exists** |
| DC-43 | [Release Security and Distribution Controls](./proposed/DC-43-RELEASE-SECURITY-CONTROLS.md) | M2 / **0.20.0** (retargeted from 0.19.0, owner ruling 2026-08-08). **Release-blocked** — inherits key lifecycle from DC-35, which needs amendment, and blocked behind criterion 4's signer bootstrap |

**Blocked, not available:** **DC-43** waits on a release-lane event, and additionally
inherits DC-35's unamended key lifecycle and criterion 4's signer bootstrap, which only the project owner
can begin. **DC-44 is no longer here** — closed 2026-09-01 and moved to `done/`.

**From the `prikk-store` structure review of 2026-09-01:** **130** and **131** answer the owner's question
about the crate's growth. An independent external architect measured the coupling graph, and the architect
re-derived it: the finding is that **line count is the least diagnostic metric available** and the cost the
owner named lives in dependency cycles and middle-of-graph hubs. **The counts in RFC 130 §2 and §4b.4
are superseded** — the gate built from them re-derived the graph and found more than either named; its
own `DECLARED_CYCLES`/`DECLARED_HUBS`, pinned by test against the real repository, are ground truth. **A crate split is ruled out on evidence**,
including `fsutil`, the one clean seam — see RFC 130 §6. Both source reviews live under `.git-exclude/` and
are invisible to a fresh clone, so RFC 130 §2 restates the measurements rather than citing them.

**`accepted/` is where settled-but-unshipped designs live, and it is not listed here.** RFC-000: an
RFC moves out of `proposed/` the moment review is complete and an implementer may start. **123, 130,
131 and 137 moved there 2026-09-05**, and **123 moved on to `done/` the same day, shipped in 0.32.0**, **130 on 2026-09-06, shipped in 0.34.0** after the project owner caught that they had been left in
`proposed/` with "ACCEPTED" written in their status text — the folder-versus-status inconsistency
this file warns about two paragraphs above. The same sweep repointed **35 files** whose handoffs and
docs still cited an RFC at a folder it had long since left; retiring an RFC had never included that
step.

**This table is a reading aid and may lag. `ROADMAP.md`'s open-work index is the authoritative
list of what is proposed** — it is the one bound in both directions by RFC 120's gate
(`tools/release-policy/src/boundary/open_work_index.rs`), so a file in `rfcs/proposed/` that is
missing from it fails `boundary-check`, and an entry naming no real file fails too. Nothing
asserts this table is complete. **126 and 128 were closed 2026-09-04** and moved to `done/`,
along with 132 and 134.

**From the external audit of 2026-08-31:** **121** through **128** carry the substantive findings of an
independent architecture audit of 0.27.1. The architect's review of that audit — every load-bearing claim
re-derived against a freshly built binary — is at `.git-exclude/reviewed/external-audit-20260831-review-v1.md`,
and the schedule is `ROADMAP.md`'s "Corrective Program After the 2026-08-31 External Audit". **123, 126 and
128 were ruled by the owner on 2026-09-01** — 123 and 126 accepting the architect's recommendations, 128
going against it to advisories-only as project policy. RFC 121's exit-code question was ruled by the
architect the same day: `release-compatibility.md:53` already names exit behavior as a compatibility
surface and already states the pre-1.0 change policy, so the policy question was settled and only the
contract's shape remained — design work. **No ruling is now outstanding on RFCs 121-129.**

**Recorded, not scheduled:** **113** is the owner's migration direction of 2026-08-19, recorded with the
architect's assessment before any importer exists to set precedent. **108, 109 and 110** are the project
owner's concepts of 2026-08-18, written
down with the architect's assessment so the direction is reviewable before anyone designs against it.
**None has a design, and implementation must not start from these records.** RFC 110's own direction is
explicitly not yet established.

**Superseded history.** DC-42 was split on 2026-07-29 into DC-56, DC-57 and DC-58 after design review
found it bundled three unrelated increments; DC-56, DC-57, DC-58, DC-59, DC-60, DC-62 and DC-63 are all
complete or accepted. DC-45 through DC-48 are accepted preparatory work already landed, not competing
future increments. **DC-52 moved to `archive/`** and is listed there.

See [`EXECUTION-ORDER.md`](./EXECUTION-ORDER.md) for what each is blocked on and what to hand developers.

## Accepted

These reviewed designs may govern downstream work but have not yet released.

| ID | Title | Milestone |
|---|---|---|
| 108 | [Workspace Concurrent Sessions](./accepted/108-workspace-concurrent-sessions.md) | Accepted 2026-08-27 on the owner's unsealed ruling. **Mechanism complete and unreleased-as-a-feature**: `active/<name>/` is general, every diagnostic surface handles more than one active session, and repair is per-active and independent (increments 1-3d, `f623c6e`). No command creates a second workspace yet; §D5 leaves naming, CLI surface, and sharing unsettled |
| DC-34 | [Publication and Identity Authority](./accepted/DC-34-PUBLICATION-IDENTITY-AUTHORITY.md) | M0 complete; governs DC-38 through DC-40 |
| DC-35 | [Release Compatibility and Status Correction](./accepted/DC-35-RELEASE-COMPATIBILITY-STATUS-CORRECTION.md) | M1 / 0.18.0; implementation accepted; signer bootstrap pending separately |
| DC-36 | [Existing-Object Publication Integrity](./accepted/DC-36-EXISTING-OBJECT-PUBLICATION-INTEGRITY.md) | M1 / 0.18.0; implementation accepted |
| DC-37 | [Required Filesystem Durability](./accepted/DC-37-REQUIRED-FILESYSTEM-DURABILITY.md) | M1 / 0.18.0; implementation accepted |
| DC-38 | [Ref Publication Crash Recovery](./accepted/DC-38-REF-PUBLICATION-CRASH-RECOVERY.md) | M1 / 0.18.0; implementation accepted |
| DC-41 | [Integrity Evidence Campaign](./accepted/DC-41-INTEGRITY-EVIDENCE-CAMPAIGN.md) | M2 / 0.19.0; all four stages implemented and accepted (`fb4153c`, `d5bd096`, `540d4db`, `2824695`); descoped platform matrix tracked as DC-49 |
| DC-50 | [First-Party SHA-256 ROI Decision](./accepted/DC-50-FIRST-PARTY-SHA256-ROI-DECISION.md) | M2; accepted by the project owner 2026-07-28 with the performance question and DC-51 allowlist collision folded in. **Closed at `4005efb` with a replace decision**; produces no code, so it stays here rather than moving to `done/`. Authorized DC-55 |
| DC-51 | [Product Dependency Placement Gate](./accepted/DC-51-PRODUCT-DEPENDENCY-PLACEMENT-GATE.md) | M2; accepted by the project owner 2026-07-28 after the author's re-examination folded in the `[target.*]` and dependency-renaming amendments. Implementation complete at `d3e939b`, post-commit review accepted with one blocking finding, repaired at `4c8b7a3` |
| DC-54 | [Operation Path Validation Symmetry](./accepted/DC-54-OPERATION-PATH-VALIDATION-SYMMETRY.md) | M2; accepted by the project owner 2026-07-28 after the author's design-completion self-critique. Implementation complete at `e8f780a`, architect post-commit review accepted 2026-07-28, no repair required. Opened by the DC-41 stage-4 campaign finding |
| DC-45 | [Release Policy Tooling Consolidation](./accepted/DC-45-RELEASE-POLICY-TOOLING-CONSOLIDATION.md) | M2 / 0.19.0; Rust command authoritative, later stability and Python retirement pending |
| DC-57 | [Active-Patch Thresholds](./accepted/DC-57-ACTIVE-PATCH-THRESHOLDS.md) | Product **M3**. **Complete at `caa2fc2`**, reviewed and accepted 2026-08-02, no findings (handoff v2; v1 withdrawn). "Active patches" defined once — the active WAL's record count — and enforced through one shared comparison every authoring path calls. Warn at 800 extends DC-66's `status` output rather than inventing a second surface; hard block at 1000 fires before any WAL append or object write, proven to leave no partial state. Both configurable via `PRIKK_ACTIVE_PATCH_WARN`/`PRIKK_ACTIVE_PATCH_LIMIT`, per-invocation only, malformed values rejected rather than silently defaulted. `seal` confirmed to remain available at and above the hard bound. NFR-PERF-02 is met; NFR-PERF-03 (merge scope) remains an explicit non-goal, unowned |
| DC-58 | [Source-Structure Audit](./accepted/DC-58-SOURCE-STRUCTURE-AUDIT.md) | Corrective M2 maintainability. **Complete** — batches 1 (`e1d0213`) and 2 (`54a3037`) accepted, N1 report reframing `6f53da3` accepted 2026-07-31. Excludes `frozen_outgoing.rs` by design; its `node_authoring.rs` deferral was pending DC-56, which has now recorded an outcome, so that exception needs re-examining |
| DC-56 | [Commit Scan and Memory Compliance](./accepted/DC-56-COMMIT-FULL-TREE-SCAN-COMPLIANCE.md) | Closes **missed product M1** gate NFR-PERF-01 plus an untracked commit-memory defect. Implemented `8748f00` and **closes partial**: the changed-path index works (content-read phase −20%), but its RFC misidentified NFR-PERF-01's dominant violator. Criteria 1,2,3,6,7 met; 4 and 5 re-scoped and carried to **DC-64**. **NFR-PERF-01 remains missed** |
| DC-63 | [Tag Surface](./accepted/DC-63-TAG-SURFACE.md) | §6.6 **closed. Complete at `6b33a72`**, implementation review accepted with one non-blocking note. Held briefly on two `refs.rs` blockers — `publish` rejected every `tags/` name and `verify` required every ref target to be a `Block` — both fixed in the ref core. First production use of `RefKind::Tag` |
| DC-64 | [Baseline Reconstruction Cost on the Commit Path](./accepted/DC-64-BASELINE-RECONSTRUCTION-COST.md) | Product **M1** — carries **NFR-PERF-01** from DC-56, the requirement DC-56 could not close. Design review discharged its blocking measurements (replay is 97.6% of the phase, ~40 us per operation replayed) and **eliminated the RFC's own leading design option** — a cache keyed on `(baseline_block, horizon)` can never hit, because the one-record WAL cap forces a seal between commits. **Implemented; closes partial**: an incremental baseline cache (`rfcs/handoffs/DC-64-baseline-reconstruction-cost/incremental-baseline-cache-design-v1.md`) eliminates the O(operations replayed) cost the design review measured, but `load`/`persist`/`from_replay` — each a binding condition of the trust-ladder ruling — remain O(live node count), so Axis A is not fully flat. **NFR-PERF-01 remains missed**, on a lower curve |
| DC-65 | [Text-Edit Baseline Content Availability](./accepted/DC-65-TEXT-EDIT-BASELINE-CONTENT.md) | Product **M1**. **Complete at `250ad54`** — reviewed and accepted 2026-07-31. The most serious defect found in this program: editing one text file across two sealed commits failed. Ruled that a node's `blob_id` is a **content identity, not necessarily a stored object**; authoring now materializes on demand as replay always did. Verified independently at N=6 sealed edits |
| DC-66 | [Multi-Commit Queuing](./accepted/DC-66-MULTI-COMMIT-QUEUING.md) | Product **M3**. **Complete at `45af36f`** — reviewed and accepted 2026-08-02; the architect independently rebuilt a four-deep queued edit chain from sealed history and got byte-correct content. One non-blocking note: `rollback-draft` still rejects on a non-empty WAL, deliberately. The active session holds N unsealed patches; `commit` no longer refuses on a non-empty active WAL; `seal` batches the queue into one block. Baseline-for-the-next-queued-patch chain rule stated and implemented; node identity across a queue proven safe; DC-64's incremental cache and DC-65's text materialization both tested at N > 1 for the first time; crash recovery covers a torn queue and a crash during seal with no silent loss; `verify`/`status` report queue health. Unblocked **DC-57**, now also implemented |
| DC-67 | [Ordinary-Use Conformance Suite](./accepted/DC-67-ORDINARY-USE-CONFORMANCE.md) | Corrective assurance. **Implemented — the prediction held.** Nine ordinary sequences at N=3 through the compiled binary (sequence 1, "edit the same text file," kept from DC-65), each ending in a delete-and-rebuild content assertion where the replay path supports it. Two findings, reported not fixed (criterion 4): `checkout --patch-materialize` cannot replay `ReplaceBinary`/`ChangePerm` (blocking criterion-2 verification for two ordinary sequences, not merely adversarial ones), and no working-directory branch-switch command exists for active multi-branch editing. Shared CLI test harness consolidated at `crates/prikk-cli/tests/support/` |
| DC-69 | [Lifecycle-State Retention](./accepted/DC-69-LIFECYCLE-STATE-RETENTION.md) | Design increment, **complete**. §3.2's original architect discharge was withdrawn on review and narrowed to a checkable invariant (a horizon may not sever a `DeleteNode` from a later restoring `CreateFile` of the same node id) — `create_node`'s restoration-equivalence check consumes tombstone content on the commit path via `rollback-draft`'s node-id-reusing inverse patches. **Verdict: prikk does not forget — route (c), established and measured** (Axis D: cumulative history alone costs real, ~linear, tree-size-independent commit time), recorded in `MILESTONES.md`. A bounded-horizon mechanism is conceivable but depends on two decisions outside this increment (bounding `rollback-draft`'s reach; redefining what full replay trusts). DC-64's binding condition 1 unchanged |
| DC-70 | [Prebuilt Binary Distribution](./accepted/DC-70-PREBUILT-BINARY-DISTRIBUTION.md) | Adoption surface. **Closes partial, reviewed and accepted 2026-08-03**, DC-56's precedent for a criterion outside the increment's reach. Targets verified by trial build: Linux (`x86_64`/`aarch64`) only — Windows found not to compile off Linux at all, a new unowned finding, not fixed here. `cargo binstall` and download-surface release-authority statements implemented. **Criterion 3 carried**: the evidence-schema extension (release evidence models a singular archive; per-target binaries are N artifacts) sits inside DC-45's frozen-until-0.19.0-cutover oracle corpus, ruled out of scope rather than edited. One review finding repaired: three `tools/release-policy` allowlist entries (`tar`, `rustc`, `gh`) were unsafe with any arguments and were narrowed to exact-match procedures |
| DC-71 | [Non-Linux Build Conformance](./accepted/DC-71-NON-LINUX-BUILD-CONFORMANCE.md) | Product **M1**. **Implemented 2026-08-04, awaiting architect review**, per the owner's ruling that portable read-only is a requirement. `fsutil/anchored`'s inconsistent cfg-gating repaired, verified on `x86_64-pc-windows-gnu`/`x86_64-apple-darwin`; the read-only/mutation command boundary traced and published; CI now builds and runs it on `windows-latest`/`macos-latest` so it cannot rot silently again. Closes the long-standing public-portability-claim mismatch. Mutation stays Linux-only per DC-37 |
| DC-72 | [Path-Safety Conformance](./accepted/DC-72-PATH-SAFETY-CONFORMANCE.md) | Product **M1/M3** — **NFR-SEC-03 missed**, a stated security guarantee not met. **Accepted 2026-08-04.** No case-collision rejection exists anywhere in `prikk-store`, for ref names *or* repository paths — wider than the ref-name-only finding recorded 2026-07-30 |
| DC-73 | [Node-Model Operation Apply](./accepted/DC-73-NODE-MODEL-APPLY.md) | **Accepted 2026-08-04** — roadmap item A, and the **first increment in this program that adds capability rather than correcting a defect**. Closes rollback refusing `ReplaceBinary`/`ChangePerm` spans and `checkout --patch-materialize` unable to replay `ChangePerm`. Lifecycle-state apply is already complete for all seven operations; the gap is materialization and inverse |
| DC-61 | [Branch Closure](./accepted/DC-61-BRANCH-CLOSURE.md) | §6.5 deletion half, as **closure** — the pointer stays. Redesigned from tombstones 2026-07-30 after review found `doctor` would resurrect deleted branches. **Complete.** Implemented `ca4c044`, reviewed 2026-07-31 — accepted with one non-blocking finding (N1, a fail-open WAL guard), **repaired `2394f1b`**. Open ref-state ObjectIds provably unmoved: the closed vector is the open bytes plus one appended field |
| DC-60 | [Branch Management Surface](./accepted/DC-60-BRANCH-MANAGEMENT-SURFACE.md) | §6.5 list + create. Accepted 2026-07-30; **scope amended the same day** — deletion moved to DC-61 after implementation proved it blocks repository-wide commits at every record count. **Complete at `6c2b7a6`**, implementation review accepted with one non-blocking note |
| DC-87 | [Windows Mutation](./accepted/DC-87-WINDOWS-MUTATION.md) | Product **M3**. Two stages behind six prerequisites. **Stage 2's deferral reason is discharged by RFC 102 (see §0, added 2026-08-16)** — it rested on ref pointer *promotion* being unachievable on Windows, and RFC 102 retired promotion entirely: a publication is now two container appends to `init`-allocated names. §3.2/§3.3 narrow to worktree-only surfaces; §3.4 half-evaporates (`promote` and `publish_immutable` have zero production callers). **§3.1 (G1 anchored resolution) is untouched and remains blocking** |
| DC-93 | [Release Policy Python Retirement](./accepted/DC-93-RELEASE-POLICY-PYTHON-RETIREMENT.md) | **Accepted 2026-08-11.** Retires 18 unused Python files (2,895 lines) and the Rust command scanner's own Python-recognition path. Supersedes DC-52's obligations 3 and 4 |
| DC-94 | [Responsibility Map Executable Binding](./accepted/DC-94-RESPONSIBILITY-MAP-EXECUTABLE-BINDING.md) | **Accepted 2026-08-11.** DC-52's obligations 1 and 2, **decoupled** — they no longer gate the retirement. May already be largely discharged, which is a complete outcome |
| DC-95 | [Verify Coverage and Finding Accumulation](./accepted/DC-95-VERIFY-COVERAGE-AND-FINDING-ACCUMULATION.md) | Product **M1**. **Accepted 2026-08-11.** Two ordered stages: nothing proves `verify` state-checks blocks end to end (the architect disabled the wiring twice and the suite stayed green), and `verify` reports only the first hard error |
| 100 | [RFC naming alignment](./accepted/100-rfc-naming-alignment.md) | **Accepted 2026-08-11.** Aligns new RFC filenames with RFC-000's `NNN-slug.md`, from 100. Legacy `DC-*`/`PR-*` names frozen |
| 102 | [Container-Based Durability](./accepted/102-container-based-durability.md) | **Accepted 2026-08-12**, successor to RFC 101. Moves durability-bearing repository state into fixed-name container files, so no new directory entry sits on the durability path — the only route to Windows parity needing no vendor-specific primitive and no weakened invariant. §4 handles the worktree, which cannot be containerized. §6's six prerequisites precede any design. **1.0-scale**, not 0.20.0 |
| 103 | [Retire Format-1](./accepted/103-retire-format-1.md) | **Accepted 2026-08-13** on the owner's direction to design without concern about migration. Format-1 repositories are rejected at open — not read-only, not auto-upgraded — removing 22 `LegacyV1` sites across 13 files, five pieces of legacy-only machinery, and three checks DC-95 Stage 1 already classified. The `created_at == 0` check survives and becomes unconditional. May relax RFC 102's constraint 6 |
| 112 | [Core Operations in the Binary Crate](./accepted/112-core-operations-in-the-binary-crate.md) | **Accepted 2026-08-18**, from an RFC 111 Stage 2 finding. `seal`, `branch create`/`close` and `tag create` are implemented in the binary-only `prikk-cli` while every comparable operation lives in `prikk-store`, so the published library cannot seal, branch or tag; the nearest symbol it exports is `simulate_one_seal_for_test_support`, a replica built for a benchmark gate. **Acceptance clears the direction only** — §4's five questions precede design, and the increment is unscheduled. Retiring RFC 111's seal simulation and its drift guard is part of the work, not a later tidy-up |
| 114 | [The Format-Stability Contract](./accepted/114-format-stability-contract.md) | **Answers badge criterion 2**, scheduled by the owner 2026-08-19. prikk's de-facto policy was migration-by-bundle, stated only inside five error messages — and severed on 2026-08-18 by the `PBNDL002` bump, undetected by review, nine gates and CI, **because an unstated policy cannot be broken, only found absent**. Proposes: verification-bearing bytes (object-id preimage, per-schema canonical encoding, signature preimage, algorithms) frozen forever; representation (repository format, containers, index, WAL, bundle) free to change behind a *tested* migration path. **`schema_version` is inside the id preimage, so evolving a payload is already safe** — the obligation is keeping every version ever written decodable. **Accepted 2026-08-19, and all five §5 decisions resolved the same day.** Owner ruling: prikk has never been in production, so **formats 1-5 are unsupported and only format 6 onward matters** — the contract was always a forward obligation. Remaining work is small: publish the contract, build the two gates (one historical case, not five), correct `layout.rs`'s five now-false migration messages, and name the format-7 carry-forward operation — **which the existing bundle primitive is not**, since it exports one ref and lands it in `remotes/` |
| 115 | [Sync: what it can mean for prikk](./accepted/115-sync-investigation.md) | **Accepted 2026-08-19 — investigation, not design.** For badge criterion 1. **Corrected the board**: "nothing built" was false — bundle export/import, the `remotes/` namespace, merge-from-received-ref (DC-85) and authorship verification of imported history all exist and are tested. **Owner rulings adopted:** the exchange unit is the **Patch**, not the Block, with block recognition travelling as a claim; the subject is the **data-model interface**, not the network. Divergent blocks lose no verification — only a one-identifier comparison, restored by a patch-set digest. **§5.1's test and security discipline is binding.** Transport (four options) and §6's design decisions remain open |
| 116 | [Sync: negotiation before transport](./accepted/116-sync-negotiation-and-transport.md) | **Accepted 2026-08-20 — investigation with a recommendation, not a design.** Follows RFC 115 closing badge criterion 1's first gap (Stages 1-4 merged, `07d8a47`). **Finds the remaining gap is negotiation, not transport**: `PEXCH001` needs the sender to know which patches to send, and while `patch_ids_reachable_from_block` and the patch-set digest both exist and are exported, **nothing exchanges either** — a digest says *that* two repositories differ, never *how*. **Recommends** digest short-circuit → per-ref narrowing → exact patch-id lists, shaped as messages so sync ships over any channel with **no network code**, keeping `prikk-store` bytes-in/bytes-out and the trust-bearing core free of a listening socket. Set reconciliation deferred pending measurement. **Owner ruled 2026-08-20:** negotiation-as-artifacts, **not** a network protocol; and criterion 1 permits sync-over-any-channel, so `prikk-store` stays bytes-in/bytes-out and prikk stays off the network. Confidentiality is the stated limit forgone |
| 117 | [Tags across repositories](./accepted/117-tag-sync.md) | **Accepted 2026-08-22 — investigation with a recommendation, not a design.** Follows criterion 1 closing with "branches only" as a stated limit. **A prikk tag names a *block* (`payload/tag.rs:14`), and blocks diverge between repositories by design (RFC 115 §2.4-§2.7) — so a tag names something that exists in exactly one repository, and `v1.0` cannot travel.** Git/Mercurial/jj avoid this only because commit identity is global; **Darcs and Pijul, which also have local state, both answer by naming the patch set** — and prikk already computes `PatchSetDigest`. **Constraint: `TagPayload`'s schema window is CLOSED** (0.22.1 ships `tag`), so unlike the recognition claim this costs a real schema 2 forever. **Recommends** `TagPayload` v2 carrying `patch_set_digest`, with resolution ambiguity refused not guessed. **Owner accepted 2026-08-22**, adopting recommendation (a) and the sequencing ahead of remote-tracking — **knowingly paying a permanent `TagPayload` schema 2**, unlike the recognition claim's two free amendments |
| DC-59 | [Commit Benchmark Harness](./accepted/DC-59-COMMIT-BENCHMARK-HARNESS.md) | Produces NFR-PERF-01's named evidence artifact. **Complete at `a9c2fe0`**, implementation review accepted 2026-07-29 with no findings. Measured the full-tree scan: 4.22 ms at 10 files to 516 ms at 10,000, change set fixed at one |

## Done

These records currently live under `done/`.

| ID | Title | Status |
|---|---|
| RFC-000 | [RFC lifecycle policy](./done/000-rfc-lifecycle-policy.md) | — |
| DC-10 | [Rollback Draft Identity and AUTHOR Signing](./done/DC-10-ROLLBACK-DRAFT-SIGNING.md) | — |
| DC-11 | [Publication Signing and Minimal Trust Store](./done/DC-11-MAINTAINER-TRUST-STORE.md) | — |
| DC-12 | [Arbitrary-Span Text Edits](./done/DC-12-ARBITRARY-SPAN-TEXT-EDITS.md) | — |
| DC-13 | [Non-Default Ref Genesis](./done/DC-13-NONDEFAULT-REF-GENESIS.md) | — |
| DC-14 | [Arbitrary-Span Text Direct Inverse and Rollback Exposure](./done/DC-14-ARBITRARY-SPAN-TEXT-INVERSE-ROLLBACK.md) | — |
| DC-15 | [Active-Session Integrity and Verification Hardening](./done/DC-15-ACTIVE-SESSION-INTEGRITY-HARDENING.md) | — |
| DC-16 | [Patch Algebra Foundation](./done/DC-16-PATCH-ALGEBRA-FOUNDATION.md) | — |
| DC-17 | [Patch Algebra Evidence Contract](./done/DC-17-PATCH-ALGEBRA-EVIDENCE-CONTRACT.md) | — |
| DC-18 | [Patch Algebra Commutation and Confluence Contract](./done/DC-18-PATCH-ALGEBRA-COMMUTATION-CONFLUENCE.md) | — |
| DC-19 | [Replay/Lifecycle Crate Boundary and Extraction Plan](./done/DC-19-REPLAY-LIFECYCLE-CRATE-BOUNDARY.md) | — |
| DC-20 | [Replay Boundary Stabilization](./done/DC-20-REPLAY-BOUNDARY-STABILIZATION.md) | — |
| DC-21 | [Merge Conflict Evidence Contract](./done/DC-21-MERGE-CONFLICT-EVIDENCE-CONTRACT.md) | — |
| DC-22 | [Public Merge Evidence UX Boundary](./done/DC-22-PUBLIC-MERGE-EVIDENCE-UX.md) | — |
| DC-23 | [Public Merge Evidence UX Stabilization](./done/DC-23-MERGE-EVIDENCE-UX-STABILIZATION.md) | — |
| DC-24 | [Data Model and Trust/Threat Documentation](./done/DC-24-DATA-MODEL-TRUST-THREAT-DOCS.md) | — |
| DC-25 | [Merge Planning Surface](./done/DC-25-MERGE-PLANNING-SURFACE.md) | — |
| DC-26 | [Documentation Home Correction](./done/DC-26-DOCUMENTATION-HOME-CORRECTION.md) | — |
| DC-27 | [Patch Algebra and Merge-Evidence Concepts Reference](./done/DC-27-PATCH-ALGEBRA-MERGE-EVIDENCE-CONCEPTS.md) | — |
| DC-28 | [Durability and Crash-Recovery Reference](./done/DC-28-DURABILITY-CRASH-RECOVERY-REFERENCE.md) | — |
| DC-29 | [Verify and Doctor Integrity/Recovery Reference](./done/DC-29-VERIFY-DOCTOR-INTEGRITY-RECOVERY-REFERENCE.md) | — |
| DC-30 | [Key Management and Signing Setup Guide](./done/DC-30-KEY-MANAGEMENT-SIGNING-SETUP-GUIDE.md) | — |
| DC-31 | [Repository Layout and Authority Reference](./done/DC-31-REPOSITORY-LAYOUT-AUTHORITY-REFERENCE.md) | — |
| DC-32 | [Path and Worktree Safety Reference](./done/DC-32-PATH-WORKTREE-SAFETY-REFERENCE.md) | — |
| DC-33 | [Concurrency and Locking Reference](./done/DC-33-CONCURRENCY-LOCKING-REFERENCE.md) | — |
| PR-001 | [Implementation Handoff](./done/PR-001-IMPLEMENTATION-HANDOFF.md) | — |
| PR-002 | [CI Fix Handoff](./done/PR-002-CI-FIX-HANDOFF.md) | — |
| PR-003 | [Persistent Store Handoff](./done/PR-003-PERSISTENT-STORE-HANDOFF.md) | — |
| PR-004 | [WAL Handoff](./done/PR-004-WAL-HANDOFF.md) | — |
| PR-005 | [CI Fix Handoff](./done/PR-005-CI-FIX-HANDOFF.md) | — |
| PR-006 | [Verification Handoff](./done/PR-006-VERIFY-HANDOFF.md) | — |
| PR-007 | [Ref Publication Handoff](./done/PR-007-REF-PUBLICATION-HANDOFF.md) | — |
| PR-008 | [Commit Scaffold Handoff](./done/PR-008-COMMIT-SCAFFOLD-HANDOFF.md) | — |
| PR-009 | [Seal Scaffold Handoff](./done/PR-009-SEAL-SCAFFOLD-HANDOFF.md) | — |
| PR-010 | [Verify Hardening Handoff](./done/PR-010-VERIFY-HARDENING-HANDOFF.md) | — |
| PR-011 | [Doctor Diagnostics Handoff](./done/PR-011-DOCTOR-HANDOFF.md) | — |
| PR-012 | [Doctor Repair Handoff](./done/PR-012-DOCTOR-REPAIR-HANDOFF.md) | — |
| PR-013 | [Ref Recovery Handoff](./done/PR-013-REF-RECOVERY-HANDOFF.md) | — |
| PR-014 | [History Inspection Handoff](./done/PR-014-HISTORY-HANDOFF.md) | — |
| PR-015 | [Checkout Plan Handoff](./done/PR-015-CHECKOUT-PLAN-HANDOFF.md) | — |
| PR-016 | [Snapshot Path-Safety Handoff](./done/PR-016-SNAPSHOT-PATH-SAFETY-HANDOFF.md) | — |
| PR-017 | [Snapshot Materialization Handoff](./done/PR-017-SNAPSHOT-MATERIALIZATION-HANDOFF.md) | — |
| PR-018 | [Worktree Status Handoff](./done/PR-018-WORKTREE-STATUS-HANDOFF.md) | — |
| PR-019 | [Worktree Patch Draft Handoff](./done/PR-019-WORKTREE-PATCH-HANDOFF.md) | — |
| PR-020 | [Patch Replay Handoff](./done/PR-020-PATCH-REPLAY-HANDOFF.md) | — |
| PR-021 | [Patch Materialization Handoff](./done/PR-021-PATCH-MATERIALIZATION-HANDOFF.md) | — |
| PR-022 | [Patch Deletion Handoff](./done/PR-022-PATCH-DELETION-HANDOFF.md) | — |
| PR-023 | [Text Anchor Scaffold Handoff](./done/PR-023-TEXT-ANCHOR-HANDOFF.md) | — |
| PR-024 | [Conservative Text Replay Handoff](./done/PR-024-TEXT-REPLAY-HANDOFF.md) | — |
| PR-025 | [Opt-In Full-File Text Edit Generation Handoff](./done/PR-025-TEXT-GENERATION-HANDOFF.md) | — |
| PR-026 | [Supported Patch Inverse Planning Handoff](./done/PR-026-INVERSE-PLAN-HANDOFF.md) | — |
| PR-027 | [Non-Mutating Rollback Preview Handoff](./done/PR-027-ROLLBACK-PREVIEW-HANDOFF.md) | — |
| PR-028 | [Rollback Draft Handoff](./done/PR-028-ROLLBACK-DRAFT-HANDOFF.md) | — |
| PR-029 | [Rollback Draft Verification Handoff](./done/PR-029-ROLLBACK-DRAFT-VERIFY-HANDOFF.md) | — |
| PR-030 | [Sealed Rollback History Classification Handoff](./done/PR-030-SEALED-ROLLBACK-HISTORY-HANDOFF.md) | — |
| DC-85 | [Merge From a Received Ref](./done/DC-85-MERGE-FROM-RECEIVED-REF.md) | **Accepted 2026-08-09.** Closes the gap DC-78 Stage 3 exposed: `execute_merge` rejects `remotes/`, so received history cannot be incorporated. **The architect's §D4 claim that this used existing machinery was false.** §3's four questions precede design |
| DC-74 | [Merge Execution](./done/DC-74-MERGE-EXECUTION.md) | Product **M3**, roadmap item B. **Accepted 2026-08-08.** Patches are context-free (stable `NodeId` + content anchors), so merged patches transport bit-identically and author signatures survive — a merge **adopts**, never synthesizes; the RFC's own original route was withdrawn on that ground. `parent_block_ids` is already `Vec`, so multi-parent lineage is a replay question deferred to its own increment. **§4 prerequisites must be answered before design**. **Release-conditioned 2026-08-08** — buildable now, not releasable until sealed history structurally records a merge; a one-way door, since history is immutable |
| DC-75 | [Merge Block Lineage and the Structural Merge Record](./done/DC-75-MERGE-BLOCK-LINEAGE.md) | **Discharges DC-74's release condition.** Sized by the developer, verified by the architect: the blocking gate is `block_state.rs:13-26`, which rejects `BlockKind::Merge` outright — greenfield write-side design, not a read-side widening. Carries one open design question (mainline-authoritative vs both-parents-verified). **Accepted 2026-08-08.** §4's read-only prerequisite investigation may run in parallel with DC-74; implementation waits for DC-74 to merge, since both touch the seal path |
| DC-76 | [Filesystem Durability Contract](./done/DC-76-FILESYSTEM-DURABILITY-CONTRACT.md) | **0.20.0, item 1.** Accepted 2026-08-08. Enabling increment for cross-platform mutation: states DC-37's guarantees as one explicit contract with a conformance suite, **Linux as sole implementation, no behaviour change**. Soundness proof is that Linux still passes unchanged. **Complete at `d568438`**, accepted 2026-08-09 |
| DC-77 | [Docs Mermaid Rendering](./done/DC-77-DOCS-MERMAID-RENDERING.md) | **0.20.0**, small, docs and CI only. Accepted 2026-08-08. The two new reference documents' diagrams render as code blocks; enabling the preprocessor requires **one exact entry in the publication-boundary allowlist**, which is the only reviewed part |
| DC-78 | [History Exchange](./done/DC-78-HISTORY-EXCHANGE.md) | **Status-claim criterion 1** — a distributed VCS that cannot distribute. Priority ruled 2026-08-09: next design work after DC-76, ahead of Windows mutation. Proposes separating **exchange** (trust, hard) from **transport** (dependency decision, deferrable). **Accepted 2026-08-09.** §4 investigation only; design gated on it, including whether DC-53 is a prerequisite |
| DC-79 | [sha2 and getrandom Upgrade](./done/DC-79-SHA2-GETRANDOM-UPGRADE.md) | **0.20.0.** Accepted 2026-08-09. `sha2` derives every `ObjectId`; proof is DC-41's vectors passing **unchanged**. Not urgent — audit clean |
| DC-80 | [ed25519-dalek Major Upgrade](./done/DC-80-ED25519-DALEK-UPGRADE.md) | **0.20.0, after DC-79.** Accepted 2026-08-09. Changes the library verifying **every** signature; a compatibility question about sealed artifacts, not a version move. Negative control required in **both** directions |
| DC-81 | [macOS Mutation](./done/DC-81-MACOS-MUTATION.md) | **0.20.0 item 2, status-claim criterion 6.** Accepted 2026-08-09. A port, not a redesign — DC-76 settled that. **G3 uses `fcntl_fullfsync`**; dependency envelope unchanged. Carries a new verification problem: **CI is the only place this can be observed**, and no macOS mutation job exists yet |
| DC-82 | [Mutation Dispatch Collapse](./done/DC-82-MUTATION-DISPATCH-COLLAPSE.md) | **0.20.0, after DC-81, before Windows.** Accepted 2026-08-09. DC-81 moved gates 110 → 135; ten call sites branching per platform does not scale. Collapses dispatch to one selection point, **preserving DC-71's runtime fallback**. No behaviour change |
| DC-83 | [Test Temp-Directory Uniqueness](./done/DC-83-TEST-TEMP-DIR-UNIQUENESS.md) | **0.20.0, ahead of DC-80.** Accepted 2026-08-09. `format_transition.rs`'s temp root uses a nanosecond timestamp alone and collides under parallel execution — **confirmed flaky on `main`**. Test-only. A flaky gate undermines every acceptance resting on "CI green" |
| DC-84 | [Test Helper Uniqueness Sweep](./done/DC-84-TEST-HELPER-UNIQUENESS-SWEEP.md) | **0.20.0, not urgent.** Accepted 2026-08-09 from DC-83's §2 finding. `unique_temp_dir` (backing 580 tests) and thirteen siblings use PID + clock with **no counter** — `monotonic_suffix` is a timestamp despite its name, which misled the architect into citing it as correct. Test-only |
| DC-86 | [Bundle Decoder Hardening](./done/DC-86-BUNDLE-DECODER-HARDENING.md) | **0.20.0.** Accepted 2026-08-09 from the DC-78 Stage 3 review. The bundle decoder is **the only parser in the product consuming bytes from an untrusted party**, and has neither fuzz coverage nor a resource bound. Test and hardening only |
| DC-88 | [Durability Contract Requirement Shape](./done/DC-88-DURABILITY-CONTRACT-REQUIREMENT-SHAPE.md) | **Complete, merged 2026-08-11.** `durable_directory_entry` restated as a single-entry confirmation rather than directory-scoped batching. The architect's scope trade ("this blocks Stage 2") was **mispriced and withdrawn** — DC-38 never calls the method |
| DC-89 | [Platform Claim Documentation Accuracy](./done/DC-89-PLATFORM-CLAIM-DOCS-ACCURACY.md) | **Complete, merged 2026-08-10.** Corrected the Linux-only mutation claim across eight sites in seven pages plus `README.md`. Criterion 1 amended mid-review after the architect's scope wrongly excluded `README.md` |
| DC-90 | [Unsafe Code Boundary and Gate](./done/DC-90-UNSAFE-CODE-BOUNDARY-GATE.md) | **Complete, merged 2026-08-11.** Turns the owner's "`unsafe` allowed under control" ruling into a checked property. The guard is `forbid`, not `deny` — review found a `deny`-level lint can be silently overridden by the very crate it constrains |
| DC-91 | [Publication Record Shape](./done/DC-91-PUBLICATION-RECORD-SHAPE.md) | **Complete (evaluation), 2026-08-11.** Answer: **partial**. A slot record removes one state class with a real detectability gain, but leaves pointer-log joint consistency untouched and **does not unblock new branch/tag creation on Windows at all** |
| DC-92 | [Lineage Replay Memoization](./done/DC-92-LINEAGE-REPLAY-MEMOIZATION.md) | **Complete, merged 2026-08-11.** `verify` O(N³) → **O(N)** (46.4 s → 2.7 s at N=160); `seal` O(N²)-per-call → near-flat; peak memory **599 MB → 15.1 MB**, bounded by lineage frontier |
| 111 | [Object Read Cost Regression](./done/111-object-read-cost-regression.md) | **Complete, merged 2026-08-18** (Stage 1 `13f7a4b`, Stage 2 `ffaab08`). Found while checking whether badge criterion 3 was still open: every `read_object` was O(N) — a full index decode per lookup — so `verify` had silently returned to ~O(N^1.8) seven days after DC-92 fixed it. `verify` 167.85 ms → **27.04 ms** at N=160 (ratio 3.51 → 1.97); `seal` 93.86 ms → **46.96 ms** on real disk, ratios flat. **Two decode-count gates, each written before its fix and observed failing first**, so the curve is now held by a gate rather than by a measurement nobody re-runs |
| DC-53 | [Repository-Wide AUTHOR Trust Verification](./done/DC-53-REPOSITORY-WIDE-AUTHOR-TRUST-VERIFICATION.md) | **Complete, merged 2026-08-18** — Stage 1 `970bc27`, Stage 2 Step 1 `27088c9`, Step 2 `89036bf`. Opened on the finding that AUTHOR signatures were **not cryptographically verified at all**. Closed with verification of every reachable Patch, one `key_id` bound to one public key (enforced at record *and* verify time, with `&ActiveLock` making the write structurally unraceable), and key material transported in a `PBNDL002` bundle so received history verifies instead of reading *Unverifiable* forever. **Badge criterion 5 MET with a stated limit**: continuity of authorship, not authenticity of first contact |
| DC-39 | [Signature and Envelope Authority](./done/DC-39-SIGNATURE-ENVELOPE-AUTHORITY.md) | M1 / 0.18.0; complete at `8f565f2`, post-commit evidence accepted |
| DC-40 | [State Merkle Root and Format Transition](./done/DC-40-STATE-MERKLE-FORMAT-TRANSITION.md) | M1 / 0.18.0; complete at `70c3902`, post-commit evidence accepted |
| DC-55 | [First-Party SHA-256 Replacement](./done/DC-55-FIRST-PARTY-SHA256-REPLACEMENT.md) | M2; accepted by the project owner 2026-07-28 after design review v1's blocking finding and five notes were resolved in revision. Identity-bearing. Implementation complete at `753ebab` (swap `8c84bc4`, fixture repairs `083d6c0`, `753ebab`); implementation review v1 returned one blocking finding, repaired and accepted at re-review v1 2026-07-29, verified by fresh clone with a negative control |
| DC-46 | [Workspace Rust 1.85 Compatibility](./done/DC-46-WORKSPACE-RUST-1.85-COMPATIBILITY.md) | M2 / before 0.19.0 RC; complete at `0d221af`, post-commit evidence accepted |
| DC-47 | [Stable Clippy Gate Alignment](./done/DC-47-STABLE-CLIPPY-GATE-ALIGNMENT.md) | M2 / before 0.19.0 RC; complete at `ea95e92`, post-commit evidence accepted |
| DC-48 | [Legacy Clippy Production Retirement](./done/DC-48-LEGACY-CLIPPY-PRODUCTION-RETIREMENT.md) | M2 / before 0.19.0 RC; complete at `383e503`, post-commit evidence accepted |
| DC-62 | [Commit Benchmark Memory Axis](./done/DC-62-COMMIT-BENCHMARK-MEMORY-AXIS.md) | **Complete at `07b1fc8`** — implemented `963caae`, N1 repaired at `07b1fc8`, both reviews accepted. Measures peak commit memory with no new dependency by sampling `/proc/<pid>/status` `VmHWM`, against a measured 6,144 KB floor. Confirms O(worktree bytes): **9.92x** above-floor growth for 10x repository size where absolute VmHWM shows 2.58x. DC-56's precondition satisfied |
| 122 | [`worktree-status` baseline repair](./done/122-worktree-status-baseline-repair.md) | **Complete 2026-09-01.** The external audit's only High-severity live product defect: the command failed on every repository this CLI can create. Rewired onto the baseline `commit` shares `7a01168`, CI conformance repaired `e6882c7`, two stale claims the fix itself created corrected `bc443e8`. The defect had been recorded in three places and fixed in none, while the README advertised the command uncaveated |
| 127 | [Release-notes history coverage](./done/127-release-notes-history-coverage.md) | **Complete 2026-09-01.** `0.23.0`'s changelog heading had been overwritten rather than preceded by the 0.24.0 bump, misattributing the whole `prikk sync` release; the gate read only the tag being cut, so history regressions passed forever. Restored and gated `1edde4c`, exemption self-guarded `e8a10d5`. Implementation found the architect's own claim of "every released tag" was 8 of 44 |
| 125 | [Format-boundary input hardening](./done/125-format-boundary-input-hardening.md) | **Complete 2026-09-02.** Four decoders more permissive than their encoders: non-canonical file modes (`fchmod` admitted setuid/setgid/sticky, guarded only at seal time in another subsystem), no path length caps, duplicate singular TLV fields, two `unreachable!`s on byte-fed paths. Landed `7bf1279`, extended to the operation-payload decoders `c6fc625`. The class was both wider (2 files named, 7 found) and shallower (29 more fields one level down) than the RFC stated |
| 121 | [The CLI's boundary contract](./done/121-cli-boundary-contract.md) | **Complete 2026-09-02**, five increments. A closed pipe panicked; exit codes were `0`/`1` only; every value-carrying flag in the crate silently took the last value; `status --nonsense` exited `0`; there was no per-command `--help`. Ruled `0` ok / `1` operational / `2` usage — a fourth code for findings was refused, since `verify --format json` already carries that verdict. §6c/§6d carried forward as AUD-09/10 |
| 124 | [Worktree ignore mechanism](./done/124-worktree-ignore-mechanism.md) | **Complete 2026-09-02.** `.prikkignore`, literal path prefixes, bound at the two worktree walks only — discovery never removal. Found that this project's tests had **never committed a file in a subdirectory on any platform**, which had hidden a latent separator defect in the core commit path |
| 129 | [Repository migration to `prikk-vcs`](./done/129-repository-migration-to-prikk-vcs.md) | **Complete 2026-09-01**, retired 2026-09-02. 404 occurrences across 31 files. Three breakages did not depend on GitHub's redirect: the generated `install.sh`'s baked-in slug, `release.yml`'s hard-coded `--repo`, and the Pages URL, which does not redirect. §9 records four corrections the execution forced on the RFC's own inventory |
| DC-44 | [Migration, Backup, and Restore Evidence](./done/DC-44-MIGRATION-BACKUP-RESTORE-EVIDENCE.md) | **Closed 2026-09-01.** Four increments: offline `bundle verify` `d7c180c`, atomic and collision-safe export `fd2424d` (and the four `sync` output sites `1c13ade`), the self-describing `PBNDL003` manifest `c135dd0`, and the backup/restore guide page `a4d875b` corrected at `d487194`. Design goal 4 (format-1 migration) superseded by RFC 114's format-refusal ruling. Multi-ref export and a rehearsed format-migration restore remain open and are named in the shipped documentation, not carried here |
| 126 | [Verification infrastructure coverage](./done/126-verification-infrastructure-coverage.md) | **Closed 2026-09-04.** All four flanks delivered. Its one outstanding item was never work but a ruling — §5a's peak-RSS question — which was tracked here *and* in RFC 133 §6, the way one gets answered once and left open in the other. It now lives in RFC 133 §6 alone |
| 128 | [What an outsider finds before they read any code](./done/128-outward-facing-project-surface.md) | **Closed 2026-09-04.** `SECURITY.md`, `CONTRIBUTING.md` (`f68a5fe`), the crate metadata pass (`bfb84f5`), the Git→prikk mapping page and four pages it found undeclared (`c128572`), `tutorial.md` declared (`3fc3201`). §4's metadata needed a crates.io publish, not only a tag — shipped with 0.31.1 |
| 132 | [`PrikkError` carries less than it knows](./done/132-error-taxonomy-structure.md) | **Closed 2026-09-04.** Increment 1 delivered (`264ba73`); increment 2 ruled deferred with three named re-open triggers (§5). **Closing did not lapse the triggers** — this is the design record to reopen against if one fires. Its two new error prefixes were a compatibility event for the stikk front-end, which matches on message text |
| 123 | [The commit message is validated and then discarded](./done/123-commit-message-and-authorship-metadata.md) | **Complete 2026-09-05, shipped in 0.32.0.** Owner ruling: the message is evidence — an optional `message` field on `PatchPayload`, inside object identity, at `Patch` schema 4. Implementation's lesson: adding a wire tag is not decode-neutral for our own code; every decoder must be taught the new tag |
| 134 | [A text span's identity is not stable under composition](./done/134-text-span-identity-under-composition.md) | **Closed 2026-09-04**, all ruled work delivered (`59fa298`, `906d015`, `6076d52`, `2c8289a`). v2 identity at `Patch` schema 3, plus sequencing-invariant containment. **What remains is a watch, not work:** RFC 113's history import is the named direction that would violate the invariant `text_span.rs` now states |
| 135 | [The entrance: what a new user meets before anything works](./done/135-first-run-entrance-and-configuration.md) | **Closed 2026-09-06, shipped in 0.33.0** (`19f5cea`). `prikk key generate/public` and `prikk setup`. Before it, a new user could not reach a sealed commit at all — it needed two hand-invented seeds and a public key **no command could derive**. Unfamiliar steps 11 → 5. §9.1's deferral of `prikk config` re-opens on one trigger: a first real adopter |
| 130 | [A coupling invariant for `prikk-store`](./done/130-module-coupling-invariant.md) | **Closed 2026-09-06, shipped in 0.34.0** (`f1be4c4`). `boundary-check` now holds the invariant: `DECLARED_CYCLES`/`DECLARED_HUBS` as allowlists with reasons, each cycle entry stating what would remove it, bound in both directions. **It caught a regression from RFC 138 — a round accepted one turn earlier.** §2's and §4b.4's counts are superseded by the gate's own declarations |
| 138 | [Asking what a repository trusts](./done/138-trust-policy-read-surface.md) | **Closed 2026-09-06, shipped in 0.34.0** (`f1be4c4`). `prikk trust maintainer list` and `check --key-id <ID>`, both with `--format json`, opened at the stikk project's request. **`check` exits `0` whichever way the question resolves** — a negative answer is an answer, not an operational failure (RFC 121). Named, not fixed: `read_current_trust_policy_snapshot` folds "never configured" and "first record torn" into one `Ok(None)` |
| 140 | [Which patches, not how many](./done/140-queued-patch-enumeration.md) | **Closed 2026-09-06, shipped in 0.35.0** (`0b5345a`). `status --format json` and queued-patch enumeration, opened at the stikk project's request. **Its own design brief understated the work**: `EditText` is node-addressed, not path-addressed, so paths are not derivable from the queue alone — resolution runs against the folded baseline `commit` and `worktree-status` already share. §7b generalizes: **a read may degrade, but it must say so in a field a machine can branch on** |
| 142 | [The content surface](./done/142-content-surface.md) | **Closed 2026-09-08, shipped in 0.36.0** (`0b749d2`). `prikk show`, prose and `--format json`; **`prikk diff` refused, not deferred** — §3's finding is that a patch is self-describing, so `EditText` carries its before/after span verbatim and no diff engine is needed. §6b corrects RFC 140 §7b's degradation rule at its boundary: **degrade absence, propagate error** — an `Err` from the object store is affirmative damage, not the ambiguity `Ok(None)` is. **Both defects across three rounds were the architect's own**, each the previous one's sign reversed |

`PR-*` files are legacy implementation handoff records retained as historical shipped records. New
design-change records use `DC-*` RFCs plus optional `rfcs/handoffs/DC-*` companions.

## Archive

These records currently live under `archive/`.

| ID | Title | Status |
|---|---|---|
| DC-49 | [Portable-Logic Platform Matrix](./archive/DC-49-PORTABLE-LOGIC-PLATFORM-MATRIX.md) | Closed 2026-08-28 — satisfied by other means; the five crates already run on macOS and Windows via the mutation jobs since 0.21.0 |
| DC-09 | [Phase 4 Node Model and Operation Application](./archive/DC-09-PHASE-4-NODE-MODEL.md) | Superseded / partially implemented historical umbrella. |
| DC-42 | [Performance and Maintainability Gates](./archive/DC-42-PERFORMANCE-MAINTAINABILITY-GATES.md) | Superseded 2026-07-29 into DC-56, DC-57, DC-58. Never implemented; design review found it bundled three unrelated increments. |
| DC-52 | [Python and Oracle Decommissioning](./archive/DC-52-PYTHON-ORACLE-DECOMMISSIONING.md) | Moved to `archive/`. **Unblocked 2026-08-08** — `DC-45:419`'s condition was discharged by 0.19.0's release and its accepted post-release stability rerun. Deletion remains a separate architect-reviewed change, and fixture compaction only after deletion review. Listed under *Proposed* until 2026-08-18 while its file already lived here |
| 101 | [First-Appearance Durability](./archive/101-first-appearance-durability.md) | **Closed 2026-08-12 with a negative result; superseded by RFC 102.** Accepted and closed the same day, no code produced. Its own §5.2 trace disproved its §1 problem statement: prikk is content-addressed, so *every* object write creates a new name, and the fix as scoped would have made Windows worse. Established that no Windows primitive provides new-name durability and that Transactional NTFS is unusable. Its transition table and three `FINDINGS.md` rows survive it. |
| 104 | [Windows Mutation](./archive/104-windows-mutation.md) | **Withdrawn 2026-08-16, the day it was accepted — a duplicate of [DC-87](./accepted/DC-87-WINDOWS-MUTATION.md)**, accepted 2026-08-10, whose §3 poses the same prerequisites. Written after an RFC-number check that could only match numerically-prefixed files and silently dropped every `DC-` one. Its one original contribution, the post-RFC-102 caller inventory, was folded into DC-87 §0 |

## Handoffs

Companion handoff directories currently exist for DC-10 through DC-25 and corrective DC-37, DC-39,
and DC-40:

- [DC-10 rollback draft signing](./handoffs/DC-10-rollback-draft-signing/)
- [DC-11 maintainer trust store](./handoffs/DC-11-maintainer-trust-store/)
- [DC-12 arbitrary-span text edits](./handoffs/DC-12-arbitrary-span-text-edits/)
- [DC-13 non-default ref genesis](./handoffs/DC-13-nondefault-ref-genesis/)
- [DC-14 arbitrary-span text inverse rollback](./handoffs/DC-14-arbitrary-span-text-inverse-rollback/)
- [DC-15 active-session integrity hardening](./handoffs/DC-15-active-session-integrity-hardening/)
- [DC-16 patch algebra foundation](./handoffs/DC-16-patch-algebra-foundation/)
- [DC-17 patch algebra evidence contract](./handoffs/DC-17-patch-algebra-evidence-contract/)
- [DC-18 patch algebra commutation confluence](./handoffs/DC-18-patch-algebra-commutation-confluence/)
- [DC-19 replay lifecycle crate boundary](./handoffs/DC-19-replay-lifecycle-crate-boundary/)
- [DC-20 replay boundary stabilization](./handoffs/DC-20-replay-boundary-stabilization/)
- [DC-21 merge conflict evidence contract](./handoffs/DC-21-merge-conflict-evidence-contract/)
- [DC-22 public merge evidence UX](./handoffs/DC-22-public-merge-evidence-ux/)
- [DC-23 merge evidence UX stabilization](./handoffs/DC-23-merge-evidence-ux-stabilization/)
- [DC-24 data model and trust/threat docs](./handoffs/DC-24-data-model-trust-threat-docs/)
- [DC-25 merge planning surface](./handoffs/DC-25-merge-planning-surface/)
- [DC-37 required filesystem durability](./handoffs/DC-37-required-filesystem-durability/)
- [DC-39 signature and envelope authority](./handoffs/DC-39-signature-envelope-authority/)
- [DC-40 state Merkle and format transition](./handoffs/DC-40-state-merkle-format-transition/)

## Current Reference Docs

Current-state references consolidate implementation facts for public documentation. They are not RFC
lifecycle records; their authoritative home is the published mdBook source under `docs/src/reference/`.

- [Data model](../docs/src/reference/data-model.md)
- [Repository layout and authority](../docs/src/reference/repository-layout.md)
- [Concurrency and locking](../docs/src/reference/concurrency-locking.md)
- [Path and worktree safety](../docs/src/reference/path-safety.md)
- [Trust and threat model](../docs/src/reference/trust-threat-model.md)
- [Durability and crash recovery](../docs/src/reference/durability-recovery.md)
- [Integrity and recovery diagnostics](../docs/src/reference/integrity-recovery.md)
- [Patch algebra and merge evidence](../docs/src/reference/patch-algebra.md)

The old `rfcs/fdds/FDD-00-DATA-MODEL.md` and `rfcs/fdds/FDD-04-TRUST-THREAT-MODEL.md` compatibility
pointers were removed in 0.17.0 after the 0.16.1 transition window. Future `rfcs/fdds/` content is
reserved for genuine gating FDDs.
