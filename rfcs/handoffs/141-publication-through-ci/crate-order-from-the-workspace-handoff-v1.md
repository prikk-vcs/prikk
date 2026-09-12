# RFC 141 §7a — `CRATE_ORDER` derived from the workspace, and the oracle cases that pinned seven

**Live 2026-09-13.** Independent of RFC 149's pending decision; owed since 2026-09-06. `CRATE_ORDER`
in `policy/evidence.rs` has seven entries and lacks `prikk-ffi`; the workspace publishes eight today
(nine if RFC 149 path A is chosen — this handoff must not care which).

## 1. The change

Replace the literal `CRATE_ORDER` with a derivation from the workspace — the way
`release_evidence.rs::publish_levels` already computes publish levels from `Cargo.toml` — so the
expected crate set is *the workspace's publishable members in dependency order*, never a list that
can go stale. `tag_or_artifact_invalid`'s count check compares against that derivation. Keep one
literal only if the RFC's §7a trade is argued for it in the report; the architect's preference is the
derivation.

## 2. The oracle

Thirteen `release-evidence` cases assert observations that encode the seven-crate expectation. Each
must be **updated deliberately, one by one, with the reason in the manifest's case description** — the
precedent is `tighten-the-evidence-schema-handoff-v2` (one case updated, reason recorded). No
regeneration script that rewrites all 57; `python_baseline_commit` provenance stays as it is unless the
report explains why it must move. `release-policy check` must be green at the end **on a genuine
eight-crate evidence document** — produce one from the tree and validate it; that is the control §7a
said was unreachable.

## 3. Controls

Perturb: remove a member from the workspace list the derivation reads → the count check fails naming
it; add a bogus one → likewise. Full gate set; cross-target from the diff (tooling, expect none — say
so). Report: `.git-exclude/review-request/rfc141-crate-order-report-v1.md`.
