# Release 0.41.0 preparation — the dev team's part

**Authorized by the owner 2026-09-12.** First use of `release-prep-handoff-template.md`; follow it, this
file fills in the blanks. **Live.**

| | |
|---|---|
| version | **0.41.0** |
| theme | **"signing readiness"** — `prikk key status` for front-ends (RFC 150), the retired seed variables unread (RFC 148 rule 1's window closed), `setup` reuses keys with a true synopsis, and two internal groupings (`rollback/`, `merge/`) |
| CHANGELOG date | **2026-09-12** — the day the release commit is prepared; if it slips past midnight JST, stop and ask the architect for the new date rather than guessing |
| last tag | `0.40.0` at `e7279edc` |
| moved out to 0.42.0 | RFC 149 (crate cut) and RFC 130 §8 (file-size gate) — not in this release, do not wait for them |

## 0. Before the sweep: one test move

Move `control4_a_retired_seed_variable_is_unread` (or a copy) into `rfc150_key_status.rs`, which runs
on Windows, with the mode step a no-op there — so Windows CI asserts the variables are unread. Its own
commit, reviewed with the prep report.

## 1. Readiness sweep — template §1, all six steps

Particular attention this release:
- `key status`'s synopsis and description; `setup`'s description (fixed at `2212f7f9`, re-read it);
  every `troubleshooting.md` entry that quotes a 0.40 message that changed.
- CHANGELOG `## Unreleased` must contain: `### Added` `key status` (naming stikk); `### Changed` the
  window closing; the `setup` description line if user-visible (it is: `--help` text) — judge and say.
  The two groupings have **no** entry, by the rulings in RFC 131 §6f.
- Root-export diff from `0.40.0`: expect exactly `AuthorKeyBinding`, `author_key_binding` added,
  nothing removed. State it.
- The smoke script (template §1.6) must cover: bare `setup` → commit/seal/verify with no `PRIKK_*`;
  second project reuses; `key status` prose and `--format json` in the ready state and in one
  not-ready state; a stale `PRIKK_AUTHOR_SEED` with a different seed changes nothing; `--help` carries
  `key status`. Runnable as `sh smoke.sh <path-to-prikk>`; put it under `.git-exclude/review-request/`
  beside the report, not in the tree.

## 2. The release commit — template §2

Three files, one line, `Release 0.41.0: signing readiness — key status for front-ends, the seed-variable
window closed, and two groupings`. Full gate set on that exact commit, exit codes not greps.

## 3. Report

`.git-exclude/review-request/release-0.41.0-prep-report-v1.md`. Anything the sweep finds and fixes is
its own commit before the release commit, so the release commit stays three files.
