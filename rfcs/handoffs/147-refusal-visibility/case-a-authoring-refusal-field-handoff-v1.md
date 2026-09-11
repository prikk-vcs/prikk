# RFC 147 §2e — a worktree-status entry says whether `commit` will refuse it, from one classifier

**Ruled:** `rfcs/accepted/147-refusal-visibility.md` §2e.

**LIVE — start.** RFC 147 was **accepted by the project owner 2026-09-12** and sits in `rfcs/accepted/`; the hold this handoff carried is lifted.

## 1. The change, in three parts that land together

**(a) One classifier, in `node_authoring`.** A function that, from a path's baseline kind (if tracked) and
its `symlink_metadata`, returns the authoring refusal `commit` would raise for it, or none. **`commit`'s
own refusal paths call it; nothing in `worktree_status` re-derives the rule.** Today the rules live in
`AuthorError::UnsupportedSymlinkAuthoring` / `UnsupportedKindTransition` and their raise sites; the
classifier is those rules given one name.

**(b) The field.** Every `WorktreeChange` gains `authoring: "authored" | "refused"` and, when refused,
`refusal: <string>`. `kind` and `detail` are unchanged — **`unsupported-path` keeps meaning
"unrepresentable name"**, per its doc comment. The report gains `refused_count`; prose gains
`refused paths: N` beside `unsupported paths: N`, and each refused entry line carries a marker the
handoff leaves to you — **choose one that cannot be mistaken for a change kind**, since a consumer's
parser splits on the first word.

Sites that produce a refused entry today, from what status already reads:
- untracked path that is a symlink or not a regular file (`worktree_status.rs:259-283` — the `Ok` arm
  currently emits plain `untracked`);
- tracked path now a symlink or directory (`:172-173` — currently `modified`, detail "not a regular
  file"; **stays `modified`**, gains `refused`).

**(c) The class.** `node_authoring.rs:121`, `impl From<AuthorError> for PrikkError`:
`UnsupportedSymlinkAuthoring` and `UnsupportedKindTransition` → `Precondition`. The other variants are
yours to judge **by the same test** — is it a caller precondition or damage — and to report, not to
move by momentum. The `refusal` strings and `commit`'s rendered messages come from the same
`AuthorError` display, so they cannot disagree.

## 2. Schema

Additive within `worktree-status-report-v1`, on the `declarations` precedent (0.38.0, no bump). **State
in the report that a consumer on the old shape reads the new one unchanged** — and prove it with a test
that parses the new JSON with the old field set.

## 3. Controls

1. **Both truths at once.** A tracked file replaced by a symlink must report `kind: modified` **and**
   `authoring: refused` in one entry, prose and JSON. That is the case option 2 could not carry and the
   reason this shape was ruled.
2. **Status and commit agree, by construction and by test.** For every refused entry status reports,
   `commit` refuses with the same reason string; for a tree with no refused entries, `commit` proceeds.
   **One test drives both commands on the same tree** — not two tests with their own expectations.
3. **Perturb the classifier once**, not the callers: change its answer for symlinks and show **both**
   status's field and commit's refusal move together. Then restore. **If only one moves, you have two
   classifiers.**
4. **The consumer's own case**: `ln -s`, then `worktree-status --format json` must show `refused` with
   `refused_count: 1` and `unsupported paths: 0` unchanged.
5. **Exit codes**: `worktree-status` keeps RFC 122's dirty-tree `1`; `commit`'s refusal keeps `1` while
   its class changes.
6. **Full gate set**, EXECUTION-ORDER.md §6 rule 9, verbatim; cross-target outcome from this round's own
   diff — `worktree_status.rs` and `node_authoring.rs` read `symlink_metadata`; count `cfg(target_os)`
   at source in both.
7. **Docs and tests that quote `integrity error: worktree authoring:`** move in the same round. Grep the
   tree, including `crates/*/tests/`; last round found a CI-failing assertion the handoff had not.

## 4. Not in this round

- No `commit --dry-run`.
- No change to what `commit` refuses — only to what status says and what class the refusal carries.
- No `unsupported-path` redefinition.
