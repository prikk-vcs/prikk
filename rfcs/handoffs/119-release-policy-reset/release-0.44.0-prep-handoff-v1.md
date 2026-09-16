# Release 0.44.0 preparation — the dev team's part

**Issued 2026-09-16.** Follow `release-prep-handoff-template.md`; this file fills in the blanks. **The owner's
decision to release these fixes promptly stands; the number is 0.44.0, not 0.43.1**, because in v0 any runtime
change is a minor bump (`docs/src/reference/release-compatibility.md`). **The architect's reading, stated so it can
be corrected: the cut is authorized; crates.io publication is not yet** — it needs the owner's own word, as
every release.

| | |
|---|---|
| version | **0.44.0** |
| theme | **"refusals that change nothing, and every kind of entry"** — the round in the private addendum; RFC 147 §2g (a directory at a declared destination resolves `deletion`, and `commit` says so) and §2h (a declared destination that is not a regular file is never read — `worktree-status` no longer hangs on a FIFO — and reports `null` differences) |
| CHANGELOG date | the day the release commit is prepared; if it slips past midnight JST, stop and ask |
| last tag | `0.43.0` at `f3860e28` |
| memory ratio (template §1.6) | against `.git-exclude/measurements/rfc133/node-count-memory-measurement-3e5bb3c03e83.md` (0.43.0's figure) |

## 0. Before the sweep — CI, and where the commits live

Template step 0 applies to `origin/main` (`350df17e`), which is green on every job. **The release's own commits
are on local `main` and stay there**; CI runs on them when the architect pushes on release day, and **the tag waits
for that run**. Do not push, and do not move them to another branch.

Read the private addendum the architect gave you before writing the CHANGELOG:
`.git-exclude/security/release-0.44.0-prep-addendum-v1.md`.

## 1. Readiness sweep — template §1, all eight steps

Particular attention:
- **Two Rust breaks in RFC 147 §2h's round, already in the CHANGELOG.** The export-name diff must show
  `DeclarationResolution::DeletionDirectory`, `DeclarationDisclosureReason::DestinationIsDirectory`, the
  `Option<bool>` fields, and **nothing removed**. Check whether the private round's commits changed any exported
  name or type, and say so either way.
- **The JSON change** — `content_changed`/`mode_changed` `null` for a non-regular destination — documented where
  `worktree-status` is.
- **Smoke script:** extend 0.43.0's with a directory at a declared destination (`deletion` plus the disclosure), a
  FIFO destination under a timeout (returns), a symlink destination (`null` booleans), and the scenarios the
  addendum lists. Keep `smoke.sh` in `.git-exclude/review-request/`.

Report: `.git-exclude/review-request/release-0.44.0-prep-report-v1.md`.

## 2. The release commit — template §2

Exactly three files; `## Unreleased` → `## 0.44.0 — <date>`. Message:
`Release 0.44.0: refusals that change nothing, and declarations that read every kind of entry`.
Full gate set on that commit, stated verbatim. **Never push.**
