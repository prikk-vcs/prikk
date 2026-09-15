# A declaration reports what `commit` will do with it, and a refusal names a way out that works

**Live 2026-09-16**, from stikk's letter 013
(`.git-exclude/upstream/stikk/receive/013-a-declared-rename-cannot-say-whether-its-content-changed.md`).
The rulings are RFC 147 §2f (`rfcs/accepted/147-refusal-visibility.md`), which closes ROADMAP §C row G1.

**Order:** start after RFC 136 increment 3's report is reviewed. **0.43.0 does not cut without it.**

## 1. Measured by the architect

**Setup:**
- a binary built from `47fb6def` (later commits touch no code);
- fresh repositories, with `a.txt` = `alpha` and `keep.txt` sealed on `heads/main`;
- then `prikk mv a.txt b.txt`.

| Then | `worktree-status --format json` | `prikk commit` |
|---|---|---|
| nothing | `missing a.txt`, `untracked b.txt`; declaration `a.txt → b.txt` | `rename-path a.txt -> b.txt` |
| `b.txt` edited | byte-identical to the row above | `rename-path` and `edit-text b.txt` |
| `rm b.txt` | `missing a.txt`; declaration still listed | `delete-file a.txt`; "declaration a.txt -> b.txt: destination is gone; recorded as a deletion, not a rename" |
| shell `mv b.txt c.txt` | `missing a.txt`, `untracked c.txt`; declaration still listed | `delete-file a.txt`, `create-file c.txt`, the same disclosure |
| `a.txt` recreated | `untracked b.txt` `"authoring": "authored"`, `refused_count: 0`; declaration listed | refused: "a.txt -> b.txt: the source is present in the worktree again; … move b.txt back to a.txt to clear the declaration before committing" |
| shell `mv b.txt a.txt` | `clean: true`, `changes: []`, `refused_count: 0`, exit 0; declaration listed | refused, same message |

**Routes, measured:**
- **Source back, destination gone:**
  - `prikk mv b.txt a.txt` prints "declaration a.txt -> b.txt -> a.txt: nets to no move, dropped". Status
    then lists no declaration; commit says "worktree has no node-addressed changes to commit".
  - `prikk mv a.txt b.txt` moves it again, and commit authors `rename-path`.
  - A shell move back clears nothing: that is the loop.
- **Both present:** `prikk mv` refuses in both directions ("both paths exist, and which one is the tracked
  node is not prikk's to guess"). Removing the recreated `a.txt` by hand let commit author `rename-path`.
- **In every route, `a.txt`'s bytes survived.**

The prose status note (`crates/prikk-cli/src/output/worktree.rs:82-84`) gives the same shell-move advice.

## 2. Required

1. **One classifier.** Extract the per-declaration resolution from commit's declaration loop
   (`crates/prikk-store/src/commit_boundary/worktree_patch/node_authoring.rs:461-538`) into one function
   that commit and `worktree-status` both call.
   - It evaluates the whole batch: keep the two-node swap tolerance at `:452-460`, and the occupied
     destination check at `:528-535`.
   - It returns a resolution for **every** declaration.
   - Commit refuses on the first refused declaration in store order, with exactly that declaration's
     `refusal` text. Commit's outcomes do not change.
2. **Report (additive within `worktree-status-report-v1`).**
   - Each `declarations` entry gains `resolution`: `"rename"` | `"deletion"` | `"deletion-ignored"` |
     `"never-tracked"` | `"refused"`, and `refusal`: `null` or commit's message.
   - Top level gains `refused_declaration_count`.
   - Prose gains `refused declarations: N`, and each declaration line shows its resolution, with a
     `[refused: …]` suffix in the §2e style.
   - `refused_count` and `clean` keep their meanings.
3. **Messages name commands that work.** Rewrite both `DeclarationContradicted` messages, and the prose
   note, so that every command a message names works in that state.
   - **Source back, destination gone:** name `prikk mv <new> <old>` (drop) and `prikk mv <old> <new>`
     (redo).
   - **Both present:** say both exist, and that one copy must be set aside first. Keeping `<new>` then
     commits the rename; keeping `<old>` needs `prikk mv <new> <old>`.
   - **Occupied destination:** measure a route first. If none works without losing data, say so rather
     than invent one, and report it.
   - The status note must not suggest a shell move.
4. **Content and mode.** On `resolution: "rename"`, add `content_changed` and `mode_changed` (booleans;
   `null` otherwise). Take them from the comparison commit uses to author the accompanying `edit-text` or
   mode operation. If that comparison is not reachable from the classifier without a second read path,
   **stop and report** rather than duplicate it.
5. **Rust callers.** `RenameDeclaration`, or whatever type carries the new fields, gets
   `#[non_exhaustive]` if it is a public struct that gains fields. Name the break in the CHANGELOG.

## 3. Controls (each must be able to fail)

1. **Parity.** One test drives `commit` and `worktree-status` against each state in §1's table and
   compares them to each other: resolution matches authored outcome; a refused declaration's `refusal` is
   byte-for-byte commit's stderr after `error: `. Perturb: status computing its own resolution for one
   case.
2. **The loop test.** For each refusal, parse the backticked `prikk mv …` commands out of the message, run
   each in that state, and assert the declaration is gone, or commit succeeds, and no file's bytes were
   lost. Show it failing against the current message text.
3. **Swap.** A two-node swap still resolves `rename` for both declarations, with no false refusal.
4. **Clean with a refusal.** Shell move back: `clean: true` and `resolution: "refused"` together, and
   `refused_declaration_count: 1`.
5. **Content and mode.**
   - Rename only: `false`/`false`. Rename plus edit: `content_changed: true`.
   - Rename plus chmod: `mode_changed: true`, in a Linux-gated test.
   - Perturb: hard-wire `false`.
6. **Additive.** A consumer written against the 0.42 field set reads the new document unchanged.

## 4. Docs, CHANGELOG, report

**Docs:**
- `docs/src/guide/worktree-status.md`: replace the "One refusal is not yet reported here" paragraph
  (lines 57-62) with the new fields, and say a refused declaration can sit in a clean worktree.
- `docs/src/guide/patches/declared-move.md`: the "Contradicted" bullet gains the measured ways out, and
  the review section shows the resolution.

**CHANGELOG, naming stikk's letter 013:**
- `### Fixed`: the advice that looped;
- `### Added`: `resolution`, `refusal`, `refused_declaration_count`, `content_changed`, `mode_changed`;
- `### Changed`: the messages, and any Rust break.

**Discipline:** fmt, clippy with 0 errors, workspace tests with 0 failed, and the staged list before every
commit; the full gates on the final commit, including both cross-target clippies. Never push, tag or
publish.

**Report:** `.git-exclude/review-request/declarations-say-what-commit-does-report-v1.md`.
