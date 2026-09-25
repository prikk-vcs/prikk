# Release 0.47.0 preparation — the dev team's part

**Live 2026-09-25, and it is next. The owner authorized the cut:** *"Yes."* (answering *"Cut 0.47.0?"*).

**The architect's reading, stated so it can be corrected:** the 0.47.0 cut is authorized, with the version, theme and
scope below. **crates.io publication is not authorized**: that needs the owner's own word at publication time, as
every release does.

Follow `release-prep-handoff-template.md`; this file fills in the blanks.

| | |
|---|---|
| version | **0.47.0**: a minor, because in v0 any runtime change is one (`docs/src/reference/release-compatibility.md`) |
| theme | **"depth"**: incoming artifacts refused before they are read; commit and merge-evidence no longer replay whole histories; the current branch named before its first seal |
| CHANGELOG date | the day the release commit is prepared; if it slips past midnight JST, stop and ask |
| last tag | `0.46.0` at `d6d21df0` |
| memory ratio (template §1.6) | against `.git-exclude/measurements/rfc133/node-count-memory-measurement-d6d21df06aef.md` (0.46.0's release-commit figure) |

## 0. Before the sweep

**CI** on `origin/main`, job by job, per template step 0. The last pushed commit is `c10a0230`, which was 16/16 plus
Docs 2/2. Read the macOS and Windows mutation suites **by name**.

## 1. Readiness sweep — template §1, all eight steps

This release changes what users see in **three** ways a consumer can trip on. Look for them first:

- **six commands' size refusals are reworded**, keeping exit 1;
- **`branch list --format json` gains a field**;
- **a new command, `prikk config`, writes a new repository file**, `.prikk/config`.

1. **`--help` versus the surface (§1.1).**
   - New: `prikk config get|set|unset|list`, and `--max-object-bytes N` on `bundle import`, `bundle preview`,
     `bundle verify` and `sync accept`.
   - Check that every synopsis *description* is true against the binary. In particular, `--max-object-bytes`
     measures an object's **encoded** size, so a file of N bytes needs N + 69.
2. **CHANGELOG completeness (§1.2).** Seven `## Unreleased` sections today:
   - the anchored text;
   - the before-reading refusal;
   - the per-object bound and `prikk config`;
   - `--ref`/`--from` naming the unpublished current branch;
   - `branch list`'s `unpublished_current_branch`;
   - `branch switch <the current branch>`;
   - queued-patch paths.

   Check that every user-visible commit since `0.46.0` has an entry. Check that the reworded
   refusals say plainly what a text-matching consumer will see change, and that their exit code did not.
3. **Docs currency (§1.3).**
   - The new size-refusal wording is quoted correctly wherever a page shows a refusal: `commands.md`'s *Size bounds
     on incoming artifacts*, `backup-restore.md`, `sync.md`, `troubleshooting.md`.
   - `git-mapping.md`'s `git config` row.
   - The scale section of `current-state.md` was rewritten this cycle from release figures. **Add the one sentence
     owed from its review:** every 64th seal also writes a checkpoint and costs about twice an ordinary one.
4. **Root-export name diff from `0.46.0` (§1.4).** The architect's list of what it must show. Say whether each is
   there, and find anything missing from it:
   - **added:** `DEFAULT_BUNDLE_MAX_OBJECT_BYTES`, `DEFAULT_EXCHANGE_ARTIFACT_MAX_OBJECT_BYTES`,
     `take_anchor_fallbacks`, and `PrikkError::ObjectOverBound` (the enum is `#[non_exhaustive]`).
     `SnapshotAnchorFallback` keeps its path;
   - **changed:** `BundleImportOptions` and `AcceptOptions`, each gaining a public `max_object_bytes` field. A
     struct literal outside the crate stops compiling. Say so under `### Changed`, as a v0 minor allows.

   `#[non_exhaustive]` on every new report type, proven from a downstream crate as in 0.41.0.
5. **`cargo package --list -p prikk` (§1.5):** the file count, and no unintended file.
6. **Memory ratio (§1.6)**, against 0.46.0's figure above, **started first**, owning the machine while it runs. 2c
   changed the baseline ladder. Peak RSS was measured unchanged there, but this is the gate that says so for
   incremental commit.
7. **The smoke script (§1.7)** extends 0.46.0's. Each assertion states its outcome:
   - **refuse before reading:** each of the six commands, given a sparse file over its bound, refuses with exit 1,
     names its bound and where it came from, and never says "malformed";
   - **the per-object bound:** a bundle imports with `--max-object-bytes` at its largest object, and refuses one
     below. `bundle verify`'s refusal never mentions `prikk config`;
   - **`prikk config`:** `set`/`get`/`unset`/`list` round-trip; the file lands in `.prikk/`; an unknown key refuses;
   - **the current branch before its first seal:** `tree --ref heads/main` equals `tree`, `branch list --format json`
     names it in `unpublished_current_branch`, and `branch switch heads/main` does not name a route that refuses;
   - **a queued patch's paths:** `show <queued id>` names a created file's path;
   - **commits between seals stay cheap:** three `commit`s at one sealed tip succeed. Assert the outcome, not a
     time.

   Keep it at `.git-exclude/review-request/smoke.sh`. The architect runs it against the published asset on Linux.
   Say which assertions are path-shaped, and compare those by identity.
8. **Absence claims (§1.8).** For each `### Added`, grep `docs/src`, `README.md` and `SECURITY.md`. Known shapes:
   *"no config file or command exists"*, *"nothing bounds an object's size"*, *"a branch with no seal cannot be
   named"*.

Report: `.git-exclude/review-request/release-0.47.0-prep-report-v1.md`. Anything found and fixed goes in its own
commit, reviewed normally, separate from what remains.

## 2. The release commit

Template §2 exactly: three files, the message `Release 0.47.0: depth`, and the full gate set on that commit.

## 3. Not the team's

Pushing, tagging, publishing, and `release-signers.toml` — never.

## 4. After the cut, in order (for the record, not for this round)

1. brygge's letter 004 is sent (the owner's call, *"after 1"*).
2. **The `seal` lineage walk: RFC 159's design round** (proposed 2026-09-25), which the owner scheduled.
3. The gate plan (held).

**RELEASED 2026-09-25.** Prep accepted (review `release-0-47-0-prep-review-v1`; 14/14 gates re-run by the architect on
`21895f46`). CI 16/16 and Docs 2/2 on the release commit before the tag. The signed tag `0.47.0` is on `21895f46`
(good signature). The Release workflow passed 5/5 with 16 assets, and the release is marked Latest. The shipped Linux
x86_64 asset was verified: its checksum matches, its build-info names `21895f46` and tag `0.47.0`, it reports
`prikk 0.47.0`, and the smoke script passes 231/231 against the **downloaded** binary. **crates.io: not yet**, pending
the owner's word.
