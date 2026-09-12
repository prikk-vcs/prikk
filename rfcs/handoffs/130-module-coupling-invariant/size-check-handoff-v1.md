# RFC 130 §8 — `size-check`: a production file over 1,200 lines is a decision, not a drift

**Ruled 2026-09-12** (owner, via RFC 149 §7.3: the gate lands *before* the crate cut). **Live; first of
0.42.0.** Controls 1 and 2 of RFC 130 §8; control 3 is subsumed by RFC 149.

## 1. The gate

`prikk-release-policy size-check [--format json]`, beside `boundary-check`, same conventions: exit 0
valid, 1 findings, 2 usage; JSON opens `"schema_version": "release-policy-size-v1"`.

- **What counts as a production file**: the same definition the coupling gate's scanner already uses —
  `src/**/*.rs` minus `tests/` subtrees, `tests.rs`, and `#[cfg(test)]` modules (RFC 130's amended
  scanner). Reuse that walk; do not write a second one. Lines are physical lines of the file.
- **Rule**: a production file over **1,200 lines** fails unless it is in `DECLARED_LARGE_FILES` with
  `reason` and `what_would_split_it`, exactly as `DECLARED_CYCLES` treats a cycle. A stale entry (file
  now under the line, or gone) fails too — the allowlist may not outlive its cause.
- **Three enter today**, each with its own reason written from reading the file, not from its size:
  `prikk-store/src/verify.rs` (1,677), `commit_boundary/worktree_patch/node_authoring.rs` (1,415),
  `bundle.rs` (1,411). `foundation/layout.rs` (1,054) is under the line and is reported, not failed.
- **Reported, not failed**: per crate, production lines, test lines, file count, and the delta since
  the last tag (read the tag from `git describe --tags --abbrev=0`; if unavailable, report `unknown`
  rather than failing). This is control 1's number; RFC 141's release evidence gains the same line —
  say in the report where it lands and show one evidence document with it.

## 2. Controls

1. Perturb the threshold to 1,000 → `layout.rs` is named and the gate fails; restore.
2. Remove one allowlist entry → its file is named; add a bogus entry → stale, named.
3. A `#[cfg(test)]` module in a large file is not counted — assert with a fixture the scanner tests
   already own.
4. JSON valid and identical in verdict to prose; the oracle (`release-policy check`) gains cases for
   the new subcommand in the established shape.
5. Full gate set. **`EXECUTION-ORDER.md` §6 rule 9 gains `size-check`** — the architect writes that
   line at acceptance; say in the report that the command is ready to be listed.

## 3. Not in this round

No file is split. No threshold other than 1,200. No test-line budget (RFC 130 §8 refuses it: it would
reward compressing tests).
