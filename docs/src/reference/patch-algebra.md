# Patch Algebra and Merge Evidence

This page is the authoritative current-state reference for Prikk's patch algebra and merge-evidence
concepts. It describes the current implementation and is grounded in the code, released RFCs, and
implementation status records listed in the anchor table at the foot of the page.

For command syntax and examples, see the [merge evidence](../guide/merge-evidence.md) and
[merge plan](../guide/merge-plan.md) guides.

## Core Caveats

- Prikk is early implementation software and is not a production Git replacement.
- Patch algebra and merge evidence are currently read-only analysis surfaces.
- `prikk merge-evidence` and `prikk merge-plan` require explicit baseline, left target, and right
  target inputs. They do not infer merge bases or branch merge intent.
- Current confluence results apply only to the supported operation subset and the selected explicit
  candidate sequences.
- `Confluent` and `ConfluentSubset` alone do not create a merge commit — `prikk merge` (DC-74) is the
  separate, explicit command that executes a confluent merge; see the [merge guide](../guide/merge.md).
- Active-WAL merge drafts, worktree conflict materialization, conflict-resolution UI, persisted
  proof/witness/plan objects, JSON output, same-node text operational transforms, path-scoped
  analysis, and public stable Rust APIs remain deferred.

## Patch Operations and Ordering

A Patch contains ordered operations. The evidence displays use `op_seq` to show the one-based operation
sequence recorded by a Patch operation, while bracketed indexes such as `left[0]` and `right[0]` show
the zero-based position in the derived left or right candidate sequence.

The current evidence model summarizes operation kind, optional node id, and a safe repository-relative
path when one is available. It does not expose raw operation payloads. Preconditions and evidence
facts are checked through the store-backed patch-algebra evidence boundary; malformed required sealed
evidence is an evidence failure, not ordinary unsupported algebra.

## Pair Classification

Internal pair classification currently uses four categories:

| Pair class | Meaning |
|---|---|
| `Independent` | The classifier sees no ordering or conflict relation for the pair, subject to later replay proof. |
| `OrderedDependency` | The pair has a required order, such as create-after-delete relations that can only be considered in one direction. |
| `Conflict` | The pair has a concrete conflict witness, such as same-path creation, live-state mismatch, mode/blob mismatch, or delete/mutation conflict. |
| `Unknown` | The relation cannot be safely classified, either because the operation/relation is unsupported, evidence is insufficient, or the design is intentionally deferred. |

These Rust categories are implementation details, not stable public API. Public commands surface the
separate merge-evidence outcomes described below.

Intent metadata is advisory. It does not override replay, lifecycle, preimage, evidence, or
commutation proof requirements.

## Commutation

Prikk treats a pair as commuting only when both conditions hold:

- the classifier reports `Independent`; and
- replaying the pair in both orders produces the same lifecycle state.

If the classifier reports an ordered dependency or conflict, the pair does not commute. If required
evidence is missing or malformed, the analysis fails closed as an evidence problem. If a relation is
not supported or is intentionally deferred, it remains unknown rather than being treated as safe.

## Flat Confluence

Current confluence is flat and explicit-input. The analysis receives a sealed baseline state plus two
candidate operation sequences derived from explicit left and right targets.

The current check requires:

- each candidate sequence to replay validly enough for the supported subset;
- cross-pairs between left and right to commute;
- replay of left-then-right and right-then-left to succeed; and
- final lifecycle states to be equal.

This is not automatic branch merge semantics. It does not choose a merge base, publish a result,
materialize a worktree, create a merge commit, or create multi-parent Blocks.

## Evidence Outcomes

`prikk merge-evidence` prints the public DC-21/DC-23 outcome vocabulary:

| Outcome | Meaning |
|---|---|
| `Confluent` | The selected sequences are proven confluent under the current supported analysis. This is scoped evidence, not execution readiness. |
| `Conflict` | A concrete conflict witness was found. |
| `OrderedDependency` | A relation requires ordering policy that the current public merge surface does not execute. |
| `Unsupported` | The operation kind or relation is outside the supported algebra subset. |
| `Deferred` | The relation is known but intentionally deferred, such as same-node text transforms or sequence-internal dependency handling. |
| `NotConfluent` | Replay or final-state comparison failed after otherwise supported analysis. |
| `EvidenceFailure` | Required sealed evidence is missing, malformed, unreadable, wrong-type, or identity-invalid. |
| `InvalidCandidate` | Candidate input is malformed or insufficient before analysis can produce usable evidence. |

`EvidenceFailure` is distinct from `Unsupported` or `Deferred`: required sealed evidence failures must
not be hidden as unknown algebra.

## Reason Codes and Proof Phases

Evidence output also prints `reason:` and item-level `phase:` fields. Reason codes explain why an
outcome was produced; phases say which proof stage produced the item.

Current public reason-code names include:

| Reason code | Meaning |
|---|---|
| `proven_confluent` | The selected pair or sequence passed the current confluence proof. |
| `pair_conflict` | A cross-side pair produced a conflict witness. |
| `ordered_dependency` | A cross-side pair requires a specific order. |
| `unsupported_operation` | The operation or relation is outside the current supported subset. |
| `same_node_text_transform_deferred` | Same-node text operational transforms are intentionally deferred. |
| `sequence_internal_dependency_deferred` | A sequence-internal dependency blocks flat confluence analysis. |
| `pair_replay_failed` | Replaying a pair in both orders did not prove commutation. |
| `final_state_mismatch` | Final lifecycle states differed after composed replay. |
| `missing_required_evidence` | Required sealed evidence was absent. |
| `malformed_required_evidence` | Required sealed evidence was present but malformed. |
| `wrong_type_required_evidence` | Required sealed evidence had the wrong object kind. |
| `unreadable_required_evidence` | Required sealed evidence could not be read. |
| `invalid_unsealed_candidate` | Optional unsealed candidate evidence was malformed. |
| `insufficient_unsealed_candidate_evidence` | Optional unsealed candidate evidence was insufficient for analysis. |

Current public proof phases include:

| Phase | Meaning |
|---|---|
| `classification` | Pair classification or evidence validation produced the item. |
| `replay-both-orders` | Pair replay in both operation orders produced the item. |
| `flatness` | Candidate-sequence flatness checks produced the item. |
| `final-state-comparison` | Final lifecycle-state comparison produced the item. |

`composed-replay` exists only behind test-only display code and is not a current public phase.

## Conflict Witness Kinds

`reason_code: pair_conflict` names the outcome bucket, not the reason: all twelve conflict-witness
kinds `patch_algebra` distinguishes internally reported as the same generic `pair_conflict` code,
which told a reader nothing about *why* a specific pair conflicted (conflict-witness-presentation
handoff v1). `MergeEvidenceDisplayItem` now additionally carries the specific kind, path, and node
identity a cross-side (`Conflict`/`OrderedDependency`) item's witness recorded:

| Field | Meaning |
|---|---|
| `witness_kind` | The specific reason this pair conflicts or orders, as a stable kebab-case label. `None` for items with no underlying witness. |
| `witness_path` | The repository-relative path the witness recorded as the reason for conflict, when it recorded one. Distinct from each side's own `operation.path`/`peer_operation.path`: derived from whichever operand's own path field or live baseline path resolves (conflict-witness-path-derivation handoff v1), so it is present even for node-identity conflicts where neither side's own operation carries a path at all — as long as the shared node has a live entry in the baseline. `None` when the two operands resolve to two different paths (a single field cannot pick one) or when the node has no live baseline entry to derive from. |
| `witness_node_id` | The shared node identity, typed (not a rendered string — no stable human-facing node-identity rendering exists in this codebase to freeze into this field). The only correlating signal for the conflicts that stay genuinely path-less: a node-identity mismatch against a node with no live entry in the baseline at all. |

Current `witness_kind` labels:

| Label | Meaning |
|---|---|
| `same-path-create` | Both sides create a node at the identical path. |
| `rename-destination-conflict` | Both sides rename the *same* node to two *disjoint* destinations. Distinct from `same-path-create`: one node, two paths, versus `same-path-create`'s two nodes, one path. |
| `node-id-reuse` | A node identity is reused across unrelated creations. |
| `live-state-mismatch` | A side's precondition does not match the state the other side's operation requires. |
| `kind-mismatch` | The two sides disagree about the node's kind (file, symlink, etc.). |
| `mode-mismatch` | The two sides set different, incompatible permission modes. |
| `blob-mismatch` | The two sides replace binary content with different, incompatible results. |
| `text-span-overlap` | Both sides edit the identical text span. |
| `text-anchor-stale` | A text edit's anchor no longer matches the state it was planned against. |
| `delete-mutation-conflict` | One side deletes a node the other side mutates. |
| `unsupported-operation` | The operation kind is outside the currently supported algebra subset. |
| `malformed-operation` | The operation itself failed to decode into usable facts. |
| `unknown-relation` | The pair's relation does not match any more specific classification. |

These labels are **an external interface**, the same footing `reason_code` and proof-phase strings
are already on (Privacy and Output Limits, below): renaming, removing, or reusing one is a breaking
change to any tool reading merge evidence.

## Conflict Resolution Is Refused By Design

**Automation may not author a conflict resolution.** This is a settled architectural position
(conflict-witness-presentation handoff v1), not an unscoped feature waiting to be built, and it
follows directly from two decisions already made elsewhere in this project:

- **DC-35**: *"Automation may verify evidence but cannot occupy either accountable approval
  identity."*
- **DC-74** applied that at the patch layer already, and its own reasoning is why arbitration can't
  be added later as an ergonomics feature: in a context-dependent (Darcs-style) model, merging a
  patch *transforms* it — its canonical bytes change, its `ObjectId` moves, and the original AUTHOR
  signature no longer covers the result, so whoever performs the merge must re-sign content they did
  not write. That is DC-35's "automation cannot occupy an accountable approval identity," arriving at
  the patch layer. **Prikk's merge design avoids this entirely**: `prikk merge` (DC-74) adopts the
  other side's patches verbatim, unmodified, under their original AUTHOR signatures — nobody
  re-signs content they did not write.

**An automatic conflict arbitrator would reintroduce exactly the problem DC-74 was built to avoid**:
resolving a conflict means producing new content — a patch — and a patch must be authored and signed
by whoever is accountable for it. So conflict arbitration is refused by the architecture itself, not
merely unscoped or deferred to a future increment.

**Resolution already exists, and needs nothing further built**: `prikk merge` refuses on any
conflict, and a person reconciles the two sides by authoring ordinary patches under their own key —
the same committing every other change in this repository already goes through.

## Merge Plan Mapping

`prikk merge-plan` preserves the underlying evidence outcome and maps it to a non-executable planning
status:

| Evidence outcome | Plan status | Action |
|---|---|---|
| `Confluent` | `ConfluentSubset` | Review the evidence, then run `prikk merge` (DC-74) to execute. |
| `Conflict` | `BlockedConflict` | Inspect evidence; conflict resolution is not implemented. |
| `OrderedDependency` | `BlockedOrderedDependency` | Inspect ordering evidence; execution ordering policy is not implemented. |
| `Unsupported` | `BlockedUnsupported` | Inspect unsupported operation evidence. |
| `Deferred` | `BlockedDeferred` | Inspect deferred design evidence. |
| `NotConfluent` | `BlockedNotConfluent` | Inspect replay/final-state mismatch evidence. |
| `EvidenceFailure` | `BlockedEvidenceFailure` | Repair or verify repository evidence before planning. |
| `InvalidCandidate` | `BlockedInvalidCandidate` | Select valid sealed candidates before planning. |

`ConfluentSubset` is intentionally narrow. It means the selected candidates are proven confluent only
for the currently supported subset. It is not a whole-merge guarantee and does not mean Prikk can
create a merge commit.

## Privacy and Output Limits

Evidence and plan output are intended for human diagnostics, not as durable machine-readable schema.
The current display model avoids raw replacement text, raw text spans, blob bytes, absolute host
paths, `.prikk` private paths, signer secrets, key material, arbitrary object debug dumps, and raw
operation payloads. Displayed paths are repository-relative when available and safe.

## Deferred Work

**`prikk merge` (DC-74) executes confluent merges** — see the [merge guide](../guide/merge.md). Still
deferred: automatic merge-base discovery, branch merge semantics beyond a two-sided confluent merge,
active-WAL merge drafts, worktree conflict materialization, conflict-resolution UI, persisted
proof/witness/merge-evidence/merge-plan objects, same-node text operational transforms, path-scoped
analysis, display-path filtering, JSON output, patch-algebra crate extraction, and public stable Rust
APIs for replay, patch algebra, merge evidence, or merge planning internals.

**A rename pairing that conflicts is classified, but a sequence containing any rename cannot yet
prove full confluence.** RFC 144 §4o.5's `rename-destination-conflict` and a rename now reaching
`same-path-create` both classify correctly at the pairwise level. Whole-sequence flatness checking
(`prikk merge-evidence`'s own real entry point) still defers on any individually-unreplayable
operation — a category `RenamePath` is in — before that pairwise classification ever runs, so a
merge evidence request touching a rename currently reports `Unsupported`, not the specific
conflict, unless the pairwise classifier is reached directly. A known gap, not fixed here.

**Conflict resolution itself is not on this list** — see Conflict Resolution Is Refused By Design,
above. "Deferred" means eventually built; automatic conflict resolution is refused by the
architecture and will not be built at all. A conflict-resolution *UI* remains deferred in the sense
above, but only for the human-authored-patch workflow this page already describes, never for an
automatic resolver.

## Claim-to-Source Anchors

| Claim | Source anchors |
|---|---|
| Pair classification uses `Independent`, `OrderedDependency`, `Conflict`, and `Unknown`. | [`types.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/patch_algebra/types.rs), [`classify.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/patch_algebra/classify.rs), [DC-16](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-16-PATCH-ALGEBRA-FOUNDATION.md) |
| Commutation requires classifier independence plus replay-both-orders proof. | [`commutation.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/patch_algebra/commutation.rs), [DC-18](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-18-PATCH-ALGEBRA-COMMUTATION-CONFLUENCE.md) |
| Flat confluence checks individual sequence validity, cross-pair commutation, composed replay, and final lifecycle-state equality. | [`commutation.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/patch_algebra/commutation.rs), [`analysis.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/patch_algebra/report/analysis.rs), [DC-18](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-18-PATCH-ALGEBRA-COMMUTATION-CONFLUENCE.md) |
| Required sealed evidence failures are reported separately from ordinary unsupported algebra. | [`evidence.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/patch_algebra/evidence.rs), [`error.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/patch_algebra/report/error.rs), [DC-17](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-17-PATCH-ALGEBRA-EVIDENCE-CONTRACT.md) |
| Merge-evidence public outcomes are `Confluent`, `Conflict`, `OrderedDependency`, `Unsupported`, `Deferred`, `NotConfluent`, `EvidenceFailure`, and `InvalidCandidate`. | [`types.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/patch_algebra/report/types.rs), [`display.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/merge_evidence/display.rs), [DC-21](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-21-MERGE-CONFLICT-EVIDENCE-CONTRACT.md) |
| Reason-code and proof-phase strings are display vocabulary, not persisted object schema. | [`display.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/merge_evidence/display.rs), [`mapping.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/patch_algebra/report/mapping.rs), [DC-21](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-21-MERGE-CONFLICT-EVIDENCE-CONTRACT.md) |
| `merge-evidence` is read-only and requires explicit baseline plus left/right targets. | [`merge_evidence.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/merge_evidence.rs), [DC-22](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-22-PUBLIC-MERGE-EVIDENCE-UX.md), [merge evidence guide](../guide/merge-evidence.md) |
| `merge-plan` maps evidence outcomes to `ConfluentSubset` and `Blocked*` statuses without adding merge execution. | [`merge_plan.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/merge_evidence/merge_plan.rs), [DC-25](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-25-MERGE-PLANNING-SURFACE.md), [merge plan guide](../guide/merge-plan.md) |
| Evidence and plan output avoid raw text spans, replacement text, blob bytes, absolute host paths, and arbitrary object debug dumps. | [`display.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/merge_evidence/display.rs), [DC-21](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-21-MERGE-CONFLICT-EVIDENCE-CONTRACT.md), [DC-23](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-23-MERGE-EVIDENCE-UX-STABILIZATION.md) |
| Patch algebra, merge evidence, and merge plan internals are not public stable Rust APIs. | [DC-20](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-20-REPLAY-BOUNDARY-STABILIZATION.md), [DC-25](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-25-MERGE-PLANNING-SURFACE.md), [implementation status](https://github.com/prikk-vcs/prikk/blob/main/rfcs/IMPLEMENTATION-STATUS.md) |
| Conflict-witness kinds (thirteen) are generated with their labels from one macro invocation, the same discipline `VerificationStage` uses. | [`types.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/patch_algebra/types.rs), [conflict-witness-presentation handoff v1](https://github.com/prikk-vcs/prikk/blob/main/rfcs/handoffs/DC-21-merge-conflict-evidence-contract/conflict-witness-presentation-handoff-v1.md) |
| `MergeEvidenceDisplayItem` publishes `witness_kind`/`witness_path`/`witness_node_id`, but never the underlying `left_op_seq`/`right_op_seq` pair or raw `text_span` bytes. | [`display.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/merge_evidence/display.rs), [`mapping.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/patch_algebra/report/mapping.rs) |
| Automatic conflict resolution is refused by design (DC-35 applied at the patch layer, per DC-74). | [DC-35](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/DC-35-RELEASE-COMPATIBILITY-STATUS-CORRECTION.md), [DC-74](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-74-MERGE-EXECUTION.md), [`merge_execute.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/merge_execute.rs), [conflict-witness-presentation handoff v1](https://github.com/prikk-vcs/prikk/blob/main/rfcs/handoffs/DC-21-merge-conflict-evidence-contract/conflict-witness-presentation-handoff-v1.md) |

## Provenance

This reference consolidates released records through DC-25 and follows the DC-26 documentation-home
model: current-state references live in the published mdBook, while RFCs retain design history and
gating material. It does not change code, schema, CLI behavior, merge semantics, or public API
stability.
