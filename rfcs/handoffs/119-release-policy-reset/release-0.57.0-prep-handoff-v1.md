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

**P3 ACCEPTED 2026-11-13** (review `release-0.57.0-prep-P3-review-v1`). The candidate `ee13618e` is pushed. **CI on it
is RED** (run `43325384765`): `stable`, macOS and Windows mutation suites; MSRV green.

## Part P3b, FIRST: `main` is red on stable, because Cargo 1.100.0 moved test executables (45 min, stop at 90; report `release-0.57.0-prep-P3b-report.md`)

**Cause (read by the architect):** `tools/test-support/src/lib.rs:56-65` (`locate_prikk_binary`) takes the test
executable's parent's parent and joins `prikk`. Cargo 1.100.0 puts test executables at
`<profile>/build/prikk/<hash>/out/<test>` (reproduced here with `cargo test -p prikk --no-run`), so that path is the
directory `<profile>/build/prikk`. The binary is still at `<profile>/prikk`. 1.85.0 keeps `<profile>/deps/`. Your P3
gates passed on rustc 1.99.1; this machine now has 1.100.0, so the gates fail locally too.

1. **Locate the binary by Cargo's contract, not its layout.** Measure first, and report:
   - whether `CARGO_BIN_EXE_prikk` is set at **run time** for `prikk`'s integration tests on 1.85.0 and on 1.100.0
     (Cargo documents it at compile time);
   - how each of the 23 test binaries could hand `env!("CARGO_BIN_EXE_prikk")` to the harness once. The
     harness is used only by `prikk-cli`'s tests.

   **The architect leans to `env!("CARGO_BIN_EXE_prikk")`,** resolved in the test crate, because it is Cargo's
   documented contract and does not depend on any layout. Walking up the ancestors to find the profile directory is a
   second layout guess, and this break shows why a guess fails. If neither fits, stop and ask.
2. **`harness_self_test/support_binary_identity.rs`** keeps proving the identity on both toolchains. Add a red control:
   a path that names a directory, not a file, must fail with a sentence naming what was expected, not `NotFound` at
   `init`.
3. **`tools/corpus/tests/support/mod.rs:128`** has its own `locate_prikk_binary`. Say whether it reads the layout or
   Cargo's messages, and fix it the same way if it reads the layout.
4. **The 15 gates on 1.100.0, and the `test-msrv` stage on 1.85.0,** on the final commit. Read CI's macOS and Windows
   jobs on it, and say so.
5. **CHANGELOG:** nothing. This is development tooling, and no user-visible behaviour changes.

That commit becomes the candidate for the external review. The release commit `ee13618e` stays below it, unchanged.

**P3b ACCEPTED and pushed 2026-11-13** (`74984253`, CI green `43335179101`, 17/17; review
`release-0.57.0-prep-P3b-review-v1`). **The candidate for the external review is `74984253`.** The team has nothing
until the review answers. P4 (the re-date) comes on the tag day.

**External review 050 answered 2026-11-13: nothing blocks the tag** (`.git-exclude/upstream/external-architect/receive/050-review-of-the-0-57-0-candidate-74984253/`;
reading `.git-exclude/reviewed/external-050-reading-v1.md`). **Owner, 2026-11-14:** *"Both your recommendations are
accepted"*:
- the `doctor` cache write is fixed in 0.57.0, as P3c below;
- `--yes` stops clearing an *unknown* holder in 0.58.0's step 0, not in this release.

## Part P3c: plain `doctor` writes nothing again (45 min, stop at 90; report `release-0.57.0-prep-P3c-report.md`)

**Measured by the reviewer** (`receive/050-…/reproduce/i4.sh`, `i4.txt`). With the current branch's worktree record
absent, damaged or empty, or with a queued commit, plain `doctor` run three times writes three different
`.prikk/cache/lifecycle-state.v1` files; `steps_since_reanchor` goes 1, 2, 3. 0.56.0's `doctor` left it untouched, and
`platform-support.md:223` says plain `doctor` is "Read-only".

**One path, read by the architect:** `doctor.rs:1203` → `branch_switch::worktree_record_restorable_by_switch` →
`plan_worktree_record_restore` (shared with `branch switch`, a writer) → `worktree_status::worktree_status_over_sound_replay`
→ `resolve_folded_worktree_baseline` with `CacheWrite::Refresh`. **The queued-commit shape is a second path the
architect has not traced.** Find every path, not only this one.

1. **Plain `doctor` resolves with `CacheWrite::Never` on every path.** The existing
   `resolve_folded_worktree_baseline_without_cache_write` is the model. The writers (`branch switch`, `commit`,
   `worktree-status`) keep `Refresh`. Put the choice on the call path, not in a rule the writers share: a shared
   check is not always the fix.
2. **A test that fails on any write, not on this one file:** `doctor` run three times leaves every file under
   `.prikk/` byte-identical (path, length, content), in each of the reviewer's shapes:
   - the record absent;
   - the record damaged;
   - the record empty;
   - a queued commit with the record present;
   - a clean repository.

   Red control: put `Refresh` back on the path above, and the test must fail naming the file.
3. **`doctor`'s findings do not change:** the worktree-record notice still appears where it did, and the same test
   output passes.
4. **The docs:** `platform-support.md:223` is true again. Grep the docs for any other sentence about which commands
   write, and say what you found.
5. **CHANGELOG:** nothing. It is a defect of this release's own feature, so it folds in.
6. **The 15 gates** on the final commit, with `test` on 1.100.0 and `test-msrv` on 1.85.0.

**Then P4 (on the tag day, after the owner's word):** re-date the heading (`CHANGELOG.md:3`), and add one sentence to
the format-8 Upgrading entry: a sync artifact declares no repository format, so a format-6/7 receiver's
`sync accept` refuses one only when it carries a non-ASCII path. That is harmless, since nothing stored differs
between format 7 and 8 (review 050 §1). Run the 15 gates.

**P3c ACCEPTED and pushed 2026-11-14** (`40a0362b`, CI green `43530709026`, 17/17; review
`release-0.57.0-prep-P3c-review-v1`). **`40a0362b` is the candidate for the tag.** Acknowledgement 051 to the external
architect is drafted for the owner. **P4 waits for the owner's word on the tag.**

**External review 052 (2026-11-14) clears `40a0362b`: nothing blocks the tag.**
- The `doctor` write is gone in every shape, the discard plan included.
- The matrix is 294 of 297 the same and 3 better (the `worktree.baseline` cells now read `ok`); with `--hexflip`, 4
  better.
- `reproduce.sh` is identical to `74984253`.

**P4 waits only for the owner's word on the tag.**

**Owner, 2026-11-14:** *"Yes. Authorized. You may do the tag and the publish."* **The architect's reading:** the signed
tag `0.57.0`, and the crates.io publication of all eight crates, are both authorized, once P4's commit is green in
CI.

## Part P4, LIVE: the tag-day commit (20 min, stop at 40; report `release-0.57.0-prep-P4-report.md`)

1. **`CHANGELOG.md:3`:** `## 0.57.0 — <the day you make this commit>`, with an em dash (U+2014), the shape
   `release-notes` extracts. Run `date` first and write that day.
2. **One sentence in the format-8 Upgrading entry** (review 050 §1), next to the artifact-declaration sentence: a sync
   artifact declares no repository format, so a format-6/7 receiver's `sync accept` refuses one only when it carries a
   non-ASCII path. That is harmless, since nothing stored differs between format 7 and 8.
3. **Nothing else.** No version change (it is 0.57.0 already) and no other file.
4. **The 15 gates** on that commit (`test` on 1.100.0, `test-msrv` on 1.85.0), tree clean. Then
   `cargo run -p prikk-release-policy -- release-notes 0.57.0` (or the gate's own extraction) prints the section.
   Say its byte count.

**Not the team's:** pushing, the tag, crates.io, the AUR, verifying the shipped assets.

**Correction to P4 item 4 (the architect, the same hour):** `release-notes` takes `<tag> <dist-dir>`, and the dist
directory holds the built release assets, so it cannot run before the release run. Drop that step. Instead, report the
heading line exactly (`sed -n 3p CHANGELOG.md`), with its em dash shown as U+2014 (`od -c`, or the bytes `e2 80 94`).
The architect checks the published notes against the section after the release run.

**P4 ACCEPTED (`149a9e3d`). 0.57.0 RELEASED 2026-11-14:**
- **The tag:** signed `0.57.0` on `149a9e3d`. CI on it was `43538758692`, 17/17, and the local `ci-status-check`
  said "release may proceed".
- **The release run:** `43540695531`, 12/12 (the CI gate, 4 builds, publication, the AUR build and push, 4 Windows
  install cells).
- **The release page:** 20 assets. Its notes begin with the tagged CHANGELOG section byte for byte (57,291 bytes),
  then the standing appendix.
- **crates.io:** all eight crates at 0.57.0.
- **AUR:** `prikk` and `prikk-bin` at 0.57.0-1.

This handoff is complete.
