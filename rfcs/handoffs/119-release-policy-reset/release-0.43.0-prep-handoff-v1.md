# Release 0.43.0 preparation — the dev team's part

**Authorized by the owner 2026-09-16 ("Authorized."); LIVE.** Follow `release-prep-handoff-template.md`;
this file fills in the blanks. **The architect's reading of that authorization, stated so it can be
corrected: the cut is authorized; crates.io publication is not** — that needs the owner's own word every
release, and it has not been given for 0.43.0.

| | |
|---|---|
| version | **0.43.0** |
| theme | **"sealed snapshots"** — a checkpoint at a ref's first block and every 64th after it (RFC 136 §10); checkouts, `--patch-materialize-delete` and `branch switch` start at the nearest **replay-verified** snapshot, read-only reports at the nearest valid one; a worktree written straight from a snapshot is **provisional** until `prikk verify`, and a derivation gate refuses history-deriving commands while it is. Shipped alongside, all consumer-reported or measurement-found: a refused checkout writes nothing (stikk 012), every rename declaration says what `commit` will do and every refusal names a route that works (stikk 013), export/sync/rollback of a history that deletes a previously edited file, and the DC-75 two-edits merge fix with its refusal classes. |
| CHANGELOG date | **2026-09-16** — the day the release commit is prepared; if it slips past midnight JST, stop and ask the architect for the new date |
| last tag | `0.42.0` at `edcdbf76` |
| memory ratio (template §1.6) | compare against `.git-exclude/measurements/rfc133/node-count-memory-measurement-37c7f9be9b59.md` (0.42.0's figure). A ratio that moved stops the cut until explained — this release changed the commit and seal paths, so read it carefully rather than glancing. |

## 0. Before the sweep — two parity rows owed from the last review

From `.git-exclude/reviewed/declarations-say-what-commit-does-review-v1.md`, ruling 2. The status side of
two resolutions is uncontrolled, and the fields ship in this release:

- `never-tracked`: `prikk mv` an untracked file, then `worktree-status` reports `never-tracked` and
  `commit` discloses "source was never a tracked node".
- `deletion-ignored`: `prikk mv` a tracked file into an ignored directory, then `worktree-status` reports
  `deletion-ignored` and `commit` authors the deletion with its own disclosure.

Add both as rows in the existing parity control (`rfc147_declaration_resolution.rs`), so status and commit
are compared for **all five** resolutions. Perturb one to show it can fail. The architect measured both
correct on `e1f7a412`, so this is regression protection, not a fix — its own commit, reviewed normally.

## 1. Readiness sweep — template §1, all eight steps

Particular attention for this release:

- **Two Rust breaks, both already in the CHANGELOG.** The export diff (template §1.4) must show
  `DeclarationOutcome` and `DeclarationResolution` **added**, `WorktreeStatusReport.declarations`'s element
  type changed, `SnapshotMaterializationReport` `#[non_exhaustive]`, and **nothing removed**. Any new report
  type carries `#[non_exhaustive]`.
- **New machine-readable fields, across three commands.** `status`: `provisional_worktree`,
  `interrupted_materialization`. `worktree-status`: per declaration `resolution`, `refusal`,
  `content_changed`, `mode_changed`, plus top-level `refused_declaration_count`. Check each is documented
  where the command is documented, and that `worktree-status-report-v1` and `status-report-v1` are
  unchanged as version strings.
- **Messages this release adds or rewrites — quote them from the binary, not from memory:** the three
  declaration refusals and their named routes; the checkout refusals that name every conflicting path;
  `PRIKK-DOCTOR-INTERRUPTED-MATERIALIZATION`; the provisional-worktree refusal from the derivation gate;
  the merge precondition classes (DC-75); `bundle import`'s note; `bundle export`'s
  "could not be derived because this history's block chain could not be walked".
- **Step 8 (absence claims) matters more than usual this release.** Twelve `### Added`/`### Changed`
  entries. Grep `docs/src`, `README.md` and `SECURITY.md` for sentences saying any of these is missing or
  not implemented — `git-mapping.md` is the known offender family, and the snapshot pages,
  `backup-restore.md` and `sync.md` were all edited mid-theme.
- **Smoke script (§1.7)** — this release's own surface, on a fresh fixture with a clean environment:
  1. a history deep enough to carry a checkpoint; `checkout --snapshot-materialize` reports
     `provisional: yes`; `commit` refuses while provisional; `verify` clears it; `commit` proceeds;
  2. `checkout --patch-materialize` onto a conflicting file: refused, **nothing written**, and the next
     `commit` still works;
  3. `prikk mv`, then each of: edit the destination (`content_changed: true`), delete it (`deletion`),
     undo the move with a shell `mv` (`clean: true` with `refused`) — and run one route the refusal names;
  4. create → edit → delete a text file: `bundle export`, import into a fresh repository, `verify`, and
     `show` renders the deleted content; `sync build`/`accept`; `rollback-draft --append-inverse` then
     `seal`;
  5. a merge whose side edits one file twice after the branch point (the DC-75 case) succeeds.

Report: `.git-exclude/review-request/release-0.43.0-prep-report-v1.md`, with `smoke.sh` beside it, and
anything found-and-fixed as its own commit, separated from what remains.

## 2. The release commit — template §2

Exactly three files: `Cargo.toml` (workspace `version` **and the seven internal pins**), `Cargo.lock`
(`cargo update --workspace --offline`, member versions only — paste the diff stat), `CHANGELOG.md`
(`## Unreleased` → `## 0.43.0 — 2026-09-16`, em-dash). One-line message:

`Release 0.43.0: sealed snapshots — checkpoints, anchored checkouts, and refusals that say what to do`

Full gate set on that commit, stated verbatim.

## 3. Not the team's

Pushing, tagging, publishing, `release-signers.toml` — never. **crates.io is not authorized for this
release** (see the reading above); do not prepare anything that assumes it.

## 4. What the architect does at the cut, listed so nothing waits on a guess

RFC 136 moves from `accepted/` to `done/`; ROADMAP row 15 closes; the gates run again on the exact release
commit; push, CI, signed tag, Release workflow, asset verification with the team's `smoke.sh`; then the
release letters to stikk (letters 012 and 013 both land in this release) and planeter.

## 5. The release commit, take two — 2026-09-16, after the Windows hold

The sweep (`9253dee4`, `3e5bb3c0`) is accepted and pushed. The first release commit `dee02867` was never
pushed and was dropped while the Windows mutation suite was red; that suite is now **green on
`353c7464`** (all 16 CI jobs), so the cut proceeds.

**Redo the release commit, unchanged in shape:** exactly three files — `Cargo.toml` (workspace version and
the seven internal pins), `Cargo.lock` (`cargo update --workspace --offline`, member versions only),
`CHANGELOG.md` (`## Unreleased` → `## 0.43.0 — 2026-09-16`, em-dash; **still today's date**, since the
hold was resolved the same day).

One line to add first: the round that lifted the hold shipped a user-visible fix
(`### Fixed — on Windows, re-checking out an unchanged file was reported as a write`), which is already in
`## Unreleased` — check it sits under the right heading before the date line moves.

Message: `Release 0.43.0: sealed snapshots — checkpoints, anchored checkouts, and refusals that say what to do`

Full gate set on that commit, stated verbatim, and nothing pushed. Report:
`.git-exclude/review-request/release-0.43.0-prep-report-v2.md` — short: the three files, the gates, and
anything the re-run turned up.
