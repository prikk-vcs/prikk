# RFC 131 §6f — grouping increment 1: `rollback_{draft,preview,verify}` → `rollback/`

**Scheduled:** 0.41.0. **Live** once the RFC 148 v3 fix has landed and CI is green on Windows.
**Ruled by the census** (`rfc131-grouping-census-report-v1.md`, review v1): candidate A — 3 modules,
6 files, 1,443 lines, 4 `crate::<mod>::` rewrites in `prikk-store`, 0 in `prikk-cli`, no allowlist
entry renamed, no cycle gained or hidden, gate green — measured by you in a worktree and confirmed by
the architect's collapse of the emitted graph.

## 1. The change

`rollback_draft`, `rollback_preview`, `rollback_verify` become `rollback::draft`, `rollback::preview`,
`rollback::verify` under one `rollback/mod.rs` that says what the family is (the read-or-stage steps
of one user-facing verb) in two sentences. `lib.rs` root exports keep **exactly the same names** —
this is grouping, not API change: the root-export diff against `0.39.0` after this round must be
empty for these three modules (RFC 131 §6d.6's before/after `use` probe on every re-exported name).
Tests move with their modules. No behaviour changes; no file is edited beyond the path rewrites, the
`mod` declarations, and the new `mod.rs`.

## 2. Controls

1. `boundary-check` green; `boundary-check --graph` before and after: **129 nodes** (one new parent),
   483 edges, hubs unchanged, `subtree_cycles` unchanged — paste the diff.
2. Root-export name diff `0.39.0`→this commit: no name added or removed by this round (the RFC 102
   and RFC 148 rounds' additions are already accounted for).
3. RFC 130's 1,200-line file gate and the coupling gate unchanged in their verdicts.
4. Full gate set, verbatim. Cross-target: state from the diff (a pure move should touch no `cfg`; say
   so and run both targets anyway — the census round did not, and this round has a product diff).
5. `rfcs/accepted/131-…` §6f gains the "done" line with the measured numbers; CHANGELOG: an internal
   line under the library section only if a path a consumer could name changed — it should not.

## 3. Not in this round

Candidate C (`merge/`) — second, its own handoff after this lands. Candidate B (`worktree/`) —
**refused by measurement**, recorded in RFC 131 §6f; do not attempt.
