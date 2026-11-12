# Release 0.57.0 preparation, as a candidate — the dev team's part

**LIVE: owner-authorized 2026-11-12** (*"Authorized."*). **The architect's reading:** this authorizes release prep
P1–P3, which ends in a candidate commit prepared locally. It does not authorize the tag, crates.io or the letters;
each still needs the owner's word. 0.57.0's planned work is complete as of U5 (`c74d979e`): theme
*"what we promised, and what reads wrong"*, owner-approved 2026-11-08 and re-scoped the same day. The history of
every part is in its handoff:
- **step 0:** `147-refusal-visibility/0.57.0-step-0-what-reads-wrong-handoff-v1.md`;
- **step 1** (doctor reports every `verify` finding, each with a true way out):
  `…-step-1-readers-report-what-gates-check-handoff-v1.md`;
- **step 2, L** (lock liveness, and the `unlock` refusal): `…-step-2-L-lock-liveness-handoff-v1.md`;
- **step 3** (the stikk and inklkunn promises, `uncommit`, `bundle import --adopt`, the count line):
  `…-step-3-what-we-promised-handoff-v1.md`;
- **(5), the notices:** `107-release-distribution-surface/0.57.0-item-5-third-party-notices-handoff-v1.md`;
- **(6), help:** `121-cli-boundary-contract/0.57.0-item-6-help-that-reads-handoff-v1.md`;
- **(4), U, the Unicode paths:** `DC-72-path-safety-conformance/0.57.0-U-unicode-repository-paths-handoff-v1.md`.

**The architect's reading, stated so it can be corrected:**
- **This prepares a candidate, not a cut.**
- **The external review is planned at the candidate** (ROADMAP 0.57.0 row): the Unicode path admission (U) and lock
  liveness (L). The architect drafts the letter after P2, and the owner sends it.
- **No tag and no crates.io** without the owner's word.

Follow `release-prep-handoff-template.md`. This file fills in the blanks.

| | |
|---|---|
| version | **0.57.0**: a minor (v0 policy: runtime changes). P1 says whether the public API also breaks: `prikk-error`'s `NonAsciiRepoPath` became `RefusedPathCharacter`, and there are new variants |
| theme | **"what we promised, and what reads wrong"** |
| CHANGELOG date | **the day of the tag** (the owner's rule), set in the last commit before it |
| last tag | `0.56.0` on `808126bd`. The published Linux binary is at `~/.pgtmp/rel-0.56.0/prikk` (sha256 `76d8a5dd…`) |
| memory ratio | against **0.56.0's release-gate profile, ≈ 1.92** (`release-0.56.0-prep-P2-review-v1`). The last measurement on disk is `.git-exclude/measurements/rfc133/node-count-memory-measurement-release-gate-36544bf3c96a-*.md` |
| smoke base | `.git-exclude/review-request/smoke-0.56.0.sh` (1074 ok). **It asserts format 7 for a new repository, so 0.57.0 needs its own `smoke-0.57.0.sh`** (P2) |
| gates | **15** (`notices-check` joined in 0.57.0) |

## The working rule

As in 0.56.0's prep:
- one part per sitting, one report each, and no file in `review-request/` until a part is complete;
- `date` at the start and the end;
- every run in an R1 scope;
- keep every output the report cites;
- build before running anything that reports on a commit;
- alternate every timing, and state the filesystem;
- `df` the scratch filesystem before the gates;
- no `rm -rf` on a variable.

## Parts

| part | items | budget (stop at ×2) | report |
|---|---|---:|---|
| **P1: the sweep and the wording** | **0.** CI on `origin/main`, every job by name. **1.** `--help` against 0.56.0's binary: every changed line, named. The top-level help is new (item 6), so compare per command. **2.** CHANGELOG `## Unreleased` in RFC 161's shape (Security, Upgrading, Output changes, then Fixed, Changed, Added). Every user-visible commit since `808126bd` has an entry. **The `### Output changes` block lists every JSON field, id, exit code and listing change since 0.56.0**, because the consumer letters are drafted from it (template §1.2). **3.** Docs currency for every changed message. **4.** Root-export diff from `0.56.0`, struct-shape changes, `#[non_exhaustive]` on new report types: say whether the public API breaks. **5.** `cargo package --list -p prikk`. | 90 min | `release-0.57.0-prep-P1-report.md` |
| **P2: measure and smoke** | **1.** The release-gate memory profile against ≈ 1.92. Start it first, and run nothing else on the machine meanwhile. **2.** Re-confirm, on the candidate, every figure the CHANGELOG states, by the method that produced it. **3.** **`smoke-0.57.0.sh`,** from 0.56.0's script: format 8 for a new repository, and a section for each feature of this release (format upgrade, NFC/NFD/ZWJ refusals, a collision at `seal`, `uncommit`, `bundle import --adopt`, the `unlock` refusal, `prikk help`, `cat --output`'s mode, `log` and `verify` JSON fields, THIRD-PARTY-NOTICES in the archive). Run it on the release build of the candidate, and the sections that must pass on 0.56.0 on 0.56.0 (`passes-on-0.56.0.md`). | 120 min | `release-0.57.0-prep-P2-report.md` |
| **P3: the candidate** | After the architect's rulings on P1 and P2: **1.** The CHANGELOG in RFC 161's shape, with `### Security` only as ruled. **2.** The version bump to 0.57.0 and `Cargo.lock`. **3.** **On that commit:** `cargo test --workspace`, the 15 gates, the smoke on its release build, and the release binary's sha256. **The commit is prepared locally, not pushed.** | 60 min | `release-0.57.0-prep-P3-report.md` |

## Not the team's

- pushing;
- **the external-review letter** (the architect drafts it against the candidate; the owner sends it);
- the tag, and re-running any CI job on the tagged commit that was cancelled for runner capacity;
- verifying the shipped artifact;
- **the release run's jobs:** the AUR jobs (and both AUR pages), and `windows-install-from-release`. The installers
  changed this release (5b–5e);
- crates.io;
- **the consumer letters** (the owner sends them after the release):
  - **stikk:** S1–S4, the new refusal ids, `bundle-preview` in `refusal-v1`, the `worktree-status` shapes, and the
    top-level help;
  - **inklkunn:** `sealed_by`, `uncommit` (newest only; ask whether their orphan can sit mid-queue, for Option M),
    the Unicode paths with ZWJ/ZWNJ refused, and that `sync seal --claim` then needs a clean worktree;
  - **planeter:** the format-8 cut, `bundle import --adopt`, and the archive sentence that is no longer worded as
    damage.

**P1 ACCEPTED and pushed 2026-11-12** (`0dfe3be4`; review `release-0.57.0-prep-P1-review-v1`). No `### Security`.

**P2 item 0, before items 1–3** (one commit, or one per file):
- **(F1)** `architecture.md` names `prikk-ffi`'s `libc` (macOS) and `windows-sys` (Windows), and `placement.rs` as the
  gate's list.
- **(F2)** `foundation/index.rs:634`'s format-6 refusal says format **8**. Fold it into the Output changes
  refusal-text line, and fix `test_support.rs:296`'s comment.
- **(F3)** "two places" becomes three, in the CHANGELOG and `release-compatibility.md:124`: add `verify`'s and
  `doctor`'s one warning in a format-6/7 repository that holds a non-ASCII path.
- **(F4)** P2's smoke runs the ASCII-only format-8 bundle into both published 0.55.0 and 0.56.0, and the Upgrading
  line cites each measurement. 0.56.0 is already in `.git-exclude/logs/0.57.0-U4/smoke-release.txt:29-30`.
- **(F5)** `tag create`'s help line carries `branch create`'s rule.
- **(Ruling 2)** `#[non_exhaustive]` on `BlockStateOutcome`, `HistoryEntry`, `PublicationTrustIssue`, `HeldLock`,
  `ObjectVerification`. **Not** on `RepositoryFormat`, `BlockStateStatus`, `GatedOperation`, which stay exhaustive so
  the CLI's own exhaustive matches still fail to compile on a new variant. Update the Upgrading line to match, and state
  the rule there in one clause.
- The 15 gates on the final commit.

**P2 ACCEPTED and pushed 2026-11-12** (`2a94904f`; review `release-0.57.0-prep-P2-review-v1`). The memory ratio is
unchanged: 1.865 against 0.56.0's 1.869, same day, alternated. The handoff's "≈ 1.92" was one review sample, not
0.56.0's figure.

**P3 item 0, before items 1–3:**
- **(F6)** `architecture.md`: macOS `libc` serves a process's start time and the machine's identity (`proc_pidinfo`,
  `gethostuuid`), not "the boot time".
- **(F7)** `crates/prikk-ffi/src/lib.rs:1-5`: the crate doc says what the crate holds on each platform. It is no longer
  "nothing on non-Windows targets".
