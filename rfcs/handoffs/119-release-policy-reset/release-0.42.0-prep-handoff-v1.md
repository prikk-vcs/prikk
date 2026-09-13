# Release 0.42.0 preparation — the dev team's part

**Authorized by the owner 2026-09-13; LIVE.** Follow `release-prep-handoff-template.md`; this file fills in the blanks.

| | |
|---|---|
| version | **0.42.0** |
| theme | **"the current branch"** — `.prikk/current-branch` as every `--ref` default and `prikk branch switch` (RFC 151); a queued patch's message in `status` JSON and `show` on a queued patch (RFC 142 §7a / RFC 140); `unsupported-path` entries refused with `commit`'s reason under a root-relative path (RFC 147 §3f); `doctor` reports an unresolvable pointer; `status` names the current branch. Tooling shipped alongside: `size-check`, the layer rule, the lifecycle gate, the CI policy job with required steps, the derived crate order. |
| CHANGELOG date | **2026-09-13** — the day the release commit is prepared; if it slips past midnight JST, stop and ask the architect for the new date |
| last tag | `0.41.0` at `32bf0006` |
| memory ratio (template §1.6) | first release with the step — record the two-size ratio; there is no prior figure, so this one is the baseline |

## 0. Before the sweep

Nothing pending from prior rounds. RFC 149 left the store's exports at 0.41.0's plus the three RFC 151
names (`current_branch`, `switch_branch`, `BranchSwitchReport`) — the export diff must show exactly those
added, nothing removed.

## 1. Readiness sweep — template §1, all seven steps

Particular attention:
- `--help` synopses and descriptions for `branch switch`, `status`, `show` (queued), `worktree-status`;
  the `branch` help no longer carries the old note anywhere; `commands.md` agrees (the inventory check
  runs, but read the descriptions too).
- CHANGELOG `## Unreleased`: RFC 151's two entries, RFC 142 §7a's `### Added`/`### Changed`, RFC 147
  §3f's `### Fixed`/`### Changed`; nothing for the groupings or gate work beyond what is already there;
  say whether the layer rule and lifecycle gate deserve a line (they are tooling; the architect's default
  is no).
- Docs messages against the binary: the four `branch switch` refusals, the `no object … in the object
  store or the active WAL` precondition, the `unsupported-path` refusal, the pointer refusals.
- Smoke script: bare `setup` → commit/seal/verify with no `PRIKK_*`; `branch create` + `branch switch`
  both ways with `verify` clean; a dirty-worktree refusal; `status --format json` `current_branch`; a
  queued patch's `message` and `show --format json` on it (`"queued": true`); `key status` ready state;
  `--help` carries `branch switch`.

## 2. The release commit — template §2

Three files; message `Release 0.42.0: the current branch — branch switch, queued patches with their
messages, and refusals that say why`.

## 3. Report

`.git-exclude/review-request/release-0.42.0-prep-report-v1.md`, one file, with `smoke.sh` beside it.
