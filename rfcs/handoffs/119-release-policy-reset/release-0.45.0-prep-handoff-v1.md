# Release 0.45.0 preparation — the dev team's part

**Drafted 2026-09-17. NOT LIVE until the owner authorizes the cut.** When authorized, this line is replaced by the
owner's words and the architect's reading of them. Follow `release-prep-handoff-template.md`; this file fills in the
blanks. crates.io publication needs the owner's own word, separately, as every release.

| | |
|---|---|
| version | **0.45.0**: a minor, because in v0 any runtime change is (`docs/src/reference/release-compatibility.md`) |
| theme | **"working together"**: one object, several signers (RFC 156) on repository format 7 with an explicit `prikk format upgrade`; distinct default key ids; merge of branches containing renames; refusals that say whether a ref is absent or received |
| CHANGELOG date | the day the release commit is prepared; if it slips past midnight JST, stop and ask |
| last tag | `0.44.0` at `20711642` |
| memory ratio (template §1.6) | against `.git-exclude/measurements/rfc133/node-count-memory-measurement-76e0089a9f99.md` (0.44.0's figure) |

## 0. Before the sweep — CI

Template step 0 applies to `origin/main`: every job of the latest `ci.yml` run, read job by job. `main` was red on
macOS and Windows from `cb1951d5` to `6eb102b7` this cycle. **Read those two jobs by name**, not the badge.

## 1. Readiness sweep — template §1, all eight steps

This is the largest release since the format-6 cut. Particular attention:

1. **Format 7 is breaking once, and the CHANGELOG must lead with it.** Order `## Unreleased` so the first section is
   `### Changed — repository format 7; breaking once, and the upgrade is explicit`. A reader must meet "0.44.0 and
   older refuse an upgraded repository; there is no downgrade" before anything else. Check that
   `release-compatibility.md` § Repository Format Transitions, `repository-layout.md` and `data-model-lifecycle.md`
   agree with the binary.
2. **Rust breaks.** Run the root-export name diff against `0.44.0` and list every change. It must show at least:
   - `RepositoryFormat::V7`;
   - the new `BundleImportReport`/`AcceptReport` fields and their `#[non_exhaustive]`;
   - `DroppedSignature`, `DroppedSignatureReason`, `MAX_COUNTED_SIGNATURES_PER_OBJECT`;
   - `upgrade_repository_format`/`FormatUpgradeOutcome`;
   - `RenamePath.author_key_ids`;
   - `require_existing_ref`/`ReceivedRefs`.

   Every one must be under a `### Changed — breaking once for Rust callers` or `### Added` entry, and **nothing may be
   removed from the exports without an entry**.
3. **JSON and text contracts consumers read** (stikk reads the CLI only; planeter drives it as a subprocess):
   - `show-report-v1` gains `author_key_ids`;
   - `key-status-v1` `key_id_source` gains `key-file` and `reason` gains `key-id-file-mismatch`;
   - refusal prefixes change (`integrity error:` and `invalid name:` → `precondition not met:`);
   - `log`/`worktree-status` exit 1 for an explicit absent ref;
   - `unsupported_operation` no longer means "a rename is present".

   Each must be in the CHANGELOG with the words a text-matching consumer needs.
4. **Docs currency for every changed message:** `troubleshooting.md`, `commands.md`, and the `first-run`,
   `security-setup`, `sync`, `merge`, `backup-restore` and `show` guides.
5. **Absence claims (template §1.8):** in particular any page still saying renames do not merge, a second signer is
   refused, or `setup` names every key `maintainer`.
6. **The smoke script** extends 0.44.0's with, each asserting its outcome:
   - **format:** `init` creates format 7; a repository made by the **released 0.44.0 asset** (download and checksum
     it) opens, verifies, and `format upgrade`s in place with container bytes unchanged; `format upgrade` is
     idempotent; the 0.44.0 asset refuses the upgraded repository;
   - **several signers:** the same block sealed on both sides merges, and `verify` exits 0; an import above the limit
     of 4 refuses with nothing written;
   - **key ids:** two fresh `setup`s get distinct `ed25519-…` ids and exchange a bundle that verifies; a 0.44.0-made
     installation keeps `author`/`maintainer` and still commits and seals;
   - **merge with renames:** rename against an unrelated edit merges with correct content, and rename against delete
     refuses;
   - **refusals:** an absent ref, a received ref, and a path with no repository, each with its class prefix.

   Keep it at `.git-exclude/review-request/smoke.sh`. The architect runs it against the published asset **on Linux**;
   say which assertions are path-shaped, and compare those by identity (`support::assert_same_path`'s rule).

Report: `.git-exclude/review-request/release-0.45.0-prep-report-v1.md`.

## 2. The release commit — template §2

Exactly three files; `## Unreleased` → `## 0.45.0 — <date>`. Message:
`Release 0.45.0: working together`.
Full gate set on that commit, stated verbatim. **Never push.**

## 3. Not the team's

- consumer letters (stikk, planeter) and the advisory's related-limitation note for GHSA-px5q-233r-6hq5 are the
  architect's;
- pushing, tagging, publishing and `release-signers.toml`: never.
