# Release 0.56.0 preparation, as a candidate — the dev team's part, in three one-sitting parts

**Live 2026-11-07, and it is next.** The owner authorized the cut on 2026-11-07 (*"Yes"*). 0.56.0's planned work is
complete (theme *"what a delivery may change, and what it leaves"*, owner-approved 2026-10-29, extended 2026-10-31 and
2026-11-04). Every part's history is in `rfcs/handoffs/155-repository-complete-artifact/0.56.0-step-3-implementation-handoff-v1.md`:
- **step 0:**
  - `log --ref <block id>` refuses instead of answering an empty history;
  - stale locks are named in the lock-conflict error and in `doctor`;
- **step 1:** `sync pending` (`sync-pending-v1`), and `state` plus pending edit paths in `show`;
- **step 3:**
  - **2A:** delivery classification, with an older or diverged delivery refused by default, in both importers;
  - **2B:** `extract --claim` over a chain of pending claims;
  - **W–W3c-5c:** the worktree follows the current branch, and every in-the-way refusal comes before the first write;
  - **F/F2:** a forged maintainer signature on any admitted Block or RefState is refused, and `bundle preview` reports
    each sealer's outcome;
  - **3g:** what "pending" means, and `unclaimed` in `show`;
  - **3h:** unreached patches and blobs are counted;
  - **3i/3i-b:** `archive verify`'s four status strings;
  - **U–U4:** `uninstall.sh --prefix`, and the PowerShell uninstaller's argument guard;
  - **T:** `trust maintainer add` tells the truth for a removed id, and gains `--format json`;
  - **D:** docs corrections (authority limits, related projects, stale locks).

**The architect's reading, stated so it can be corrected:**
- **This prepares a candidate, not a cut.**
- **The external review is planned, at the candidate** (owner-approved plan, 2026-10-29). Two of this release's
  changes are admission rules over untrusted input (2A and F/F2). The architect drafts the letter after P3.
- **No tag and no crates.io** without the owner's word.
- **`prikk unlock`'s behaviour does not change in 0.56.0.** The owner ruled it into 0.57.0 (part L, on the ROADMAP's
  0.57.0 row). This release's docs describe today's behaviour: the liveness line is advice, and `--yes`/`--force`
  clears.

Follow `release-prep-handoff-template.md`. This file fills in the blanks and splits the work into parts.

| | |
|---|---|
| version | **0.56.0**: a minor, because there are runtime changes (v0 policy). P1 item 4 says whether the public API also breaks |
| theme | **"what a delivery may change, and what it leaves"** |
| CHANGELOG date | **the day of the tag** (the owner's rule), set in the last commit before it |
| last tag | `0.55.0` on commit `511d6dab`. **The published 0.55.0 binary is no longer on this machine** (`~/.pgtmp` was wiped). Download the Linux asset from the `0.55.0` GitHub Release into a fresh, empty scratch directory, check it against the release's checksum file, and paste both the check and `--version` |
| memory ratio | against **0.55.0's candidate runs, 1.8688× and 1.853×** (release-gate profile, `release-0.55.0-prep-P2-review-v1`) |
| smoke base | `.git-exclude/review-request/smoke-0.55.0.sh` (447 ok on the published 0.55.0 binary) |

## The working rule

- **One part per sitting,** in order, with one final report each. The report lists every item with its status.
- **No file in `.git-exclude/review-request/` until the part is complete.** If you must stop, end with *"Continuing — not
  ready for review"*.
- **Paste the `date` output when the work starts and when it ends.**
- **Every run inside an R1 scope.**
- **Keep every output the report cites,** gate logs included (`| tee`).
- **Build before running anything that reports on a commit.**
- **Every timing is alternated** (before and after in turn, order reversed each round). State the filesystem.
- **If a worktree is used,** paste `git worktree list` after removing it. **No `rm -rf` on a variable.**
- **`df` the scratch filesystem before the gates.** If it is full, never delete another session's files.

## Parts

| part | items | budget (stop at ×2) | report |
|---|---|---:|---|
| **P1: the sweep and the wording** | **0.** CI on `origin/main`, every job by name, the Windows and macOS jobs included.<br>**1.** `--help` against 0.55.0's binary: every changed line, named.<br>**2. The `### Security` question: a list to the architect, and no block written yet.** Measure each candidate **on 0.55.0's published binary** and say what it did. The architect expects two entries:<br>• (a) `bundle import` and `archive import` admitted a Block or RefState whose maintainer signature fails under an adopted key (`verify` reported it only afterwards);<br>• (b) an older or diverged delivery moved what the receiver holds, if 0.55.0 shows that on a received pointer or an adopted branch.<br>Say whether you agree, and whether anything else in this release is reachable by untrusted input (a received bundle or an archive).<br>**3. Docs currency and the docs-debt grep:**<br>• **the amendment fold:** the W3b–W3c-5c, F, 3g–3i and U paragraphs in `commands.md` and `guide/patches/worktree-patch.md` become one present-tense statement each;<br>• no step labels or function names in user text;<br>• **the three items the D report found:** `concurrency-locking.md` "a positive result … is reliable evidence to refuse" becomes "for the operator to refuse"; `trust-threat-model.md` (~line 171) "permanently, until an operator removes it out-of-band" is false now that `trust maintainer remove` exists; `worktree-patch.md:7` "Key material is supplied via the environment" (setup writes the seeds to the key directory, and prikk finds them with no variable);<br>• every message this release changes is quoted correctly in `troubleshooting.md`, `commands.md` and the guide pages.<br>**4. The public-API diff from 0.55.0** (`cargo public-api diff 0.55.0..HEAD`, pasted). Name every change, including `PrikkError::MaintainerKeyIdBound` and every new report or refusal type. Put `#[non_exhaustive]` on each new or changed public enum and report type, or write one doc line saying why not. A break goes in `### Upgrading` with what to call instead.<br>**5.** `cargo package --list -p prikk`.<br>**6. The CHANGELOG read through as a user.** `## Unreleased` grew entry by entry (about 495 lines, about 20 of them carrying "step …", "W3c", "3g" or "design review" labels). Fold it into one story per command:<br>• no step labels, round names or function names;<br>• no intermediate behaviour that never shipped (a fix to an unreleased feature folds into the feature's own entry);<br>• each refusal id said once, with its command;<br>• the `### Output changes` block complete. The consumer letters (planeter, stikk, inklkunn) are drafted from it.<br>**8.** Absence claims for every `### Added` entry | 90 min | `release-0.56.0-prep-P1-report.md` |
| **P2: measure and smoke** | **1.** The release-gate memory profile against 1.8688×/1.853×.<br>**2. Re-confirm every figure the CHANGELOG states on the candidate,** by the method that produced it, alternated against 0.55.0's binary. If the CHANGELOG states none, say so. Then measure the two costs this release added on a release build:<br>• the admission signature check, per 20,000 objects, for `bundle import` and `archive import`;<br>• `archive import --adopt`'s early in-the-way check, at 10,000 and 20,000 objects.<br>State them in the CHANGELOG only if P1's fold asks for them.<br>**3. `smoke-0.56.0.sh`,** extending `smoke-0.55.0.sh`, with a section per user-visible change:<br>• every new `refusal-v1` id reachable from the CLI (`forged-publication-signature`, the delivery-class refusals, `maintainer-key-id-bound-to-removed-key`, `maintainer-key-id-adopted-with-different-key`, and the rest P1 finds). Name any that is not reachable;<br>• each in-the-way refusal (`merge --into`, `branch create --from`, `sync seal`, `archive import --adopt`) leaves `.prikk` and the worktree byte-identical, and the retry after moving the file succeeds;<br>• the worktree after `merge --into` the current branch, then an ordinary `commit`, records no deletion;<br>• `trust-add-v1`; `archive verify`'s four statuses; `sync pending` and `show`'s `unclaimed`; `verify`'s unreached patch and blob count;<br>• `uninstall.sh --prefix` refusing an empty value, a missing value and a disagreement with `PRIKK_INSTALL_DIR`, run against a decoy prefix only.<br>Run it on a release build. Then run the new lines on 0.55.0's binary and name each that passes there, and why | 90 min | `release-0.56.0-prep-P2-report.md` |
| **P3: the candidate** | After the architect's rulings on P1 and P2:<br>**1.** The CHANGELOG in RFC 161's shape: Security only as ruled, Upgrading, one Output-changes block, Fixed, Changed, Added.<br>**2.** The three-file release commit: version, lock, and the CHANGELOG heading dated today (re-dated at the tag if needed).<br>**3.** The 14 gates; the RFC 162 matrix; the external `reproduce.sh`; and `matrix.py` v6 at `--compactions 1` and `2`, on the release build **and** on 0.55.0's binary, compared by row. **Name every row that differs, and why.** 2A and F refuse deliveries that 0.55.0 admitted, and the in-the-way checks moved refusals before the first write, so a row may now refuse earlier or write less. Each difference must be explained as that, and never as a write 0.55.0 did not make | 75 min | `release-0.56.0-prep-P3-report.md` |

## Not the team's

- pushing;
- **the external-review letter** (the architect drafts it against the candidate; the owner sends it);
- the tag; re-running any CI job on the tagged commit that was cancelled for runner capacity, until the run is fully
  green;
- verifying the artifact;
- **the release run's jobs, which the architect reads:**
  - the AUR jobs, then both AUR pages at 0.56.0;
  - `windows-install-from-release`, all four cells. The uninstaller guard changed this release;
- crates.io;
- **the consumer letters,** which the owner sends after the release:
  - **planeter:** what 0.56.0 changes for a forge, confirming the 2026-10-31 letter against the shipped binary;
  - **stikk:** the new refusal ids and `trust-add-v1`;
  - **inklkunn:** that the Unicode paths implementation and their two asks are scheduled in 0.57.0.

**P1 ACCEPTED 2026-11-07** (`ec042439` … `7df69576`, pushed; review `release-0.56.0-prep-P1-review-v1`). The architect
reran Security (b) on both binaries and measured a real 0.55.0 repository upgrading on the candidate ("no migration
is needed" holds). **Next: P2,** as above, plus:
- **the two costs, stated in the CHANGELOG** (ruling 9): one line each, in user words, measured on a release build;
- **smoke lines for Security (a), (b) and (d)** on both binaries. (c) needs a perturbed build, so the smoke says so.

**P3 carries these rulings first:**
- **R1: `### Security` gets four bullets, (a)–(d)**, each with:
  - what was wrong, in one sentence, pointing to its Fixed story;
  - **affected versions:** "0.55.0 (measured)". Add "and every earlier release that has `<command>`" only if
    `git log -S` on the refusal's code finds no earlier equivalent check;
  - **what a user should do,** with every sentence measured on the candidate: (a) `verify`'s
    `PRIKK-TRUST-PUBLICATION-INVALID-SIGNATURE` and the command that removes the forged history; (b) how to see that
    a received pointer was replaced, and how to re-import the right tip; (c) what to do once `verify` or `doctor`
    names it; (d) re-run `extract --claim` under 0.56.0 for claims from a source you don't control.
  - (d)'s narrower reach is stated in its bullet.
- **R2: B1–B7 `#[non_exhaustive]` confirmed;** help text unchanged; "no migration is needed" stays.
- **R3: remove the two older step labels** (`durability-recovery.md:350`, `integrity-recovery.md:239`).
- **R4: one sentence in `commands.md`'s `archive import` section.** With a local branch of that name already
  present, `--adopt` lands the delivery as `remotes/<name>`, classified against the received pointer only (often
  `no-local-history`). The local branch is neither compared nor moved. The class id is unchanged.
- **`log --ref` versus `tree`/`checkout` wording and exit code:** a ROADMAP candidate, not this release.

**P2 ACCEPTED 2026-11-07** (`45d42d89`, pushed; review `release-0.56.0-prep-P2-review-v1`). The architect reran the
smoke (1047 ok, 0 FAIL), the memory profile across four builds (+11 MiB at 64,000 files, W2b's guard) and an
independent `archive import` timing (+132 ms for 4,000 signatures, as predicted).

**Next: P2b, the way out that works** (one sitting, 45 min, stop at 90; report `release-0.56.0-prep-P2b-report.md`):
- **F1, a product fix.** Every command a stale-commit refusal names, run literally, is followed by the same `commit`
  succeeding, or by a refusal that names its own different reason. With `commit --ref <x>` where `<x>` is not the
  record's own ref, the way out is `prikk branch switch <x>`, substituted literally, and the two no-ops are not
  named. 44a and 44d keep their text.
  - **The test** enumerates the shapes by which of the three refs are equal: the ref committed onto, the record's ref
    and the current branch. Each shape that can refuse runs the named commands literally, then the same commit.
    Say which assertion fails without the fix.
  - **Docs:** `commands.md:162` ("always works") and the CHANGELOG at ~261 say the rule as measured. §44b asserts
    that the named command unblocks the commit.
- **One message string:** "this delivery is a `<class>` against …" becomes one grammatical form for all three
  classes. The `refusal-v1` ids and fields stay. Update the quotations: the CHANGELOG ~117, the `troubleshooting.md:993`
  heading, `backup-restore.md:253`, and the tests.
- `cargo test --workspace` and the 14 gates on the final commit.

**P3 then carries, besides R1–R4:**
- **R5:** `### Upgrading` gains the third cost, `commit` on a large tree needs about 10 % more memory (about 11 MiB
  more at 64,000 files, release build).
- **R6:** `commands.md:223` says "about 9 %", with the CHANGELOG's conditions.
- **R7:** "a refused or crashed import's leftover" becomes "a crashed or interrupted import's leftover" in
  `show.md:101–102`, `commands.md:451` and `:720`, `current-state.md:411`, and the CHANGELOG at ~387 and ~420.
- **R8:** one sentence on `bundle preview`, in `commands.md` ~383 and the CHANGELOG's preview bullet: the outcomes
  are for the tip's signatures, while `bundle import` checks every Block and RefState.
- **R9:** a refused `commit` leaves `.prikk` unchanged except for its cache, as `worktree-status` does.
  CHANGELOG ~39: "at its first `commit`, `seal`, or switch to another branch".
- **R10:** one line in `passes-on-0.55.0.md`: 46c's two lines pass on 0.55.0 only on some runs (171–173 ok over
  five runs), because 0.55.0's `sync pending` lists two or three patches.

**P2b ACCEPTED 2026-11-07** (`4fae9401` … `43787138`, pushed; review `release-0.56.0-prep-P2b-review-v1`). The
architect ran shapes 4 and 6 by hand and repeated the red control (`2 passed; 5 failed` with the new arm disabled);
the smoke gives 1046 ok. **Next: P3**, with these two added to its first commits:
- **R11:** `concurrency-locking.md:241` and `troubleshooting.md:991` stop calling `appears running` reliable. Say
  instead: "treat a lock that `appears running` as held; the check can be wrong after pid reuse, but only in the
  direction of not clearing it". The negative half is unchanged, and part L (0.57.0) changes the mechanism.
- **R12:** double-backtick spans for the two quotations of the class message, which now contains backticks:
  `troubleshooting.md:994`'s heading and `backup-restore.md:253`. Rewrap the CHANGELOG's line ~118.

**P3 reviewed 2026-11-07** (review `release-0.56.0-prep-P3-review-v1`). `40a63a8e` and `f2a586e2` are accepted and
pushed. **The release commit `25f28e10` is not the candidate, and is not pushed.** Finding 1 (a record naming a Block
this repository does not hold makes `commit` and `branch switch` fail with `integrity error`, while `verify` and
`doctor` stay clean) is fixed first.

**Next: P3b, the record that names a missing block** (one sitting, 60 min, stop at 120; report
`release-0.56.0-prep-P3b-report.md`):
0. **Drop the unpushed release commit.** `git log --oneline origin/main..main` must show exactly `25f28e10`; then
   `git reset --keep origin/main`. Paste both. `origin/main` now carries this note.
1. **Route (a).** `commit`, `seal`, `branch switch` and `worktree-status` read a record whose Block this repository
   does not hold as no record. That is the module's posture for a record that does not parse, and the next write
   replaces it.
   - **The test:** replace one hex digit of the block id with another valid hex digit. `commit` succeeds and authors
     only the new file; after `seal` the record names the new tip; on a second fixture the self-switch exits 0.
     Include a red control.
   - **`repository-layout.md`:** one sentence. Ignored records are absent, empty, unreadable, or naming a missing
     Block, and the next `commit`, `seal` or `branch switch` rewrites them. Until then `commit` cannot tell a moved
     branch from the user's own change, as before 0.56.0.
   - **CHANGELOG:** one clause in the commit-guard bullet, not a Fixed entry.
2. **Security (c)'s last sentence, measured.** Start with the fork's recipe, from a repository 0.55.0 admitted the
   archive into. Find the shortest route to a repository that passes `verify` with the user's own history (e.g.
   `bundle export --ref heads/<branch>` into a fresh `prikk setup`, plus whatever makes it a local branch again).
   Every step must be measured, ending with `verify` exit 0.
   - **If nothing works in 30 minutes,** the bullet ends: "0.56.0 has no command that makes such a repository pass
     `verify`; its other history stays usable (`commit` and `seal` work); a fix is planned" (the ROADMAP row exists).
   - The bullet must not say "restore a `.prikk`" or "read it with a newer version".
3. **The three-file release commit again.**
4. **On that final commit:**
   - `cargo test --workspace`;
   - the 14 gates;
   - the smoke on the release build;
   - `matrix.py` v6 at `--compactions 1` and `2` on the candidate, compared by row with P3's 0.55.0 runs;
   - the external `reproduce.sh`;
   - finding 1's probe (a hex-preserving flip of the block id now heals).

The 8 `SILENT` cells are accepted for 0.56.0 (ROADMAP: a `doctor` notice for an ignored record).

**P3b reviewed 2026-11-07** (review `release-0.56.0-prep-P3b-review-v1`). `baf66df2` and `c936c151` are accepted and
pushed. **The release commit `36544bf3` is held once more:** the Security (c) route text cannot be run as written,
because `sync have` lacks `--output` and a bare `sync accept` fails with "requires a file path". The architect ran
the route with the full flags (two branches, identical block ids, `verify` and `doctor` exit 0) and reran the memory
profile, which is unchanged.

**Next: P3c, the (c) wording** (one sitting, 20 min, stop at 40; report `release-0.56.0-prep-P3c-report.md`):
0. `git log --oneline origin/main..main` must show exactly `36544bf3`. Then `git reset --keep origin/main`. Paste
   both.
1. In `CHANGELOG.md` only, the (c) bullet's route sentences become the text in the review's ruling 1. Every command
   carries its file arguments: `sync have … --output have.bin`, `sync build … --have have.bin --output art.bin`,
   and `sync accept art.bin --claims-out claims.txt`, then `sync seal heads/<branch> --claims claims.txt`. Say
   "under the same user account, so that it holds the same keys", and keep the second-branch `branch create` step.
2. The three-file release commit again.
3. **On it:** `cargo test --workspace` and the 14 gates. Also show the release binary's sha256 is
   `286d5db50401…`, as at `36544bf3`. If it differs, rerun the smoke, the matrix and `reproduce.sh`.

**P3c is larger than the note above says (2026-11-07, after CI finished).** CI on `c936c151` is **red**: the Windows
mutation suite failed in `object_store::tests::two_racing_object_appends_serialise_and_never_share_an_offset`. One
racer got `Io(PermissionDenied, "Access is denied. (os error 5)")`, where only success or `LockConflict` is allowed.
The same source ran green at `9776930e`, so this is an intermittent race. It has existed since RFC 102 (2026-09-12)
and is not this round's. No release is cut on red CI, so it is fixed first. **P3c is now one sitting, 90 min, stop
at 180:**
0. As above (`git reset --keep origin/main`, after confirming `36544bf3` is the only commit ahead).
1. **The Windows race:**
   - give the I/O errors on the lock and object-append paths their path;
   - from the code, name the operation that returns `ERROR_ACCESS_DENIED` in this race (a delete-pending lock file
     under `CREATE_NEW`, or a replace-by-rename onto a file still open without `FILE_SHARE_DELETE`);
   - fix it there, Windows only, so a racer gets `LockConflict` or goes through. A persistent permission error with
     no competing writer stays itself;
   - say whether the store is consistent after such a failure today;
   - add a deterministic Windows-only test if one can be built, or say why not and how the fix is shown;
   - a CHANGELOG `### Fixed` entry (a released defect).
2. The (c) wording, as above.
3. The three-file release commit again.
4. **On it:** tests, the 14 gates, the smoke on the release build, `matrix.py` v6 at both depths by row, and
   `reproduce.sh`. The binary changes, so nothing carries over. The architect pushes the fix commits first and reads
   their Windows jobs before the release commit goes up.

**P3c reviewed 2026-11-07** (review `release-0.56.0-prep-P3c-review-v1`). `fe69c565` and `13a483a5` (the Windows lock
race) are accepted and pushed. **CI is green on `13a483a5`** (`42374829699`, 17/17); the Windows job runs the new
hammer test and the racing test, and both are `ok`. **The release commit `9aebe969` is held for one sentence:** the
Fixed entry's "about one run in forty" is not what CI shows. That test failed once in 503 CI runs on `main` since
2026-09-12, the architect having searched every failed run's log.

**Next: P3d** (one sitting, 20 min, stop at 40; report `release-0.56.0-prep-P3d-report.md`):
0. `git log --oneline origin/main..main` must show exactly `9aebe969`. Then `git reset --keep origin/main`. Paste
   both.
1. In the `### Fixed` entry and the hammer test's doc comment, write "seen once in CI (one run in about 500 since
   the test was added)", or drop the rate.
2. The three-file release commit again.
3. **On it:** `cargo test --workspace`, the 14 gates, and the release binary's sha256, which must be
   `1828804ee7ee…`. If it is, the smoke, the matrix and `reproduce.sh` carry over from P3c; if not, rerun them.

**P3d ACCEPTED 2026-11-07. Release prep is complete:** the 0.56.0 candidate is **`ffa581db`**, pushed, CI green
(`42378051038`, 17/17). The architect rebuilt it (sha256 `1828804ee7ee…`) and ran the external architect's `matrix.py`
v6 at both depths: 289/289 cells the same as their 0.55.0 output, plus 8 `SILENT` `worktree.baseline` cells. Their
`reproduce.sh` gives the same 68 exit codes. The external-review letter (046) is drafted, and the owner sends it.
**Nothing is owed by the dev team** until the external answer, or a re-date commit if the tag lands on another day.

**External review 046 (2026-11-07): one item blocks the tag** (reading `external-046-reading-v1`). On a repository
with one text file edited at every step, `verify` and `doctor` report one **false unreached Blob per checkpoint**
(every 64th block), with the forgery-leftover warning. The architect reproduced it: `ffa581db` gives 0 at 64 blocks
and 2 at 130; 0.55.0 gives 0, because it counted no Blobs. The cause is
`verify/reachability.rs::expand_frontier`: it follows a Block's `snapshot_blob_ref` to the manifest Blob, and its
`ObjectType::Blob` arm expands nothing. So the checkpoint's content Blobs, named only by the manifest, are never
reached. `archive/verify.rs` uses the same walk.

**Next: P4, the reach walk follows a checkpoint's manifest** (one sitting, 60 min, stop at 120; report
`release-0.56.0-prep-P4-report.md`):
1. **The fix, in `expand_frontier`:** for a Blob of `BlobKind::Snapshot`, decode its manifest (the existing
   `validate_snapshot_manifest`) and push every content Blob it names.
   - A manifest that does not decode is treated as the walk treats a Patch that does not decode today: say which,
     and keep it.
   - Every caller of `expand_frontier` gets the fix. Name them; `verify` and `archive verify` at least.
2. **Tests:**
   - 65 blocks with one edited text file: `verify` reads `unreached objects: 0`;
   - the same history exported: `archive verify` reports nothing unreached;
   - 130 blocks: still 0;
   - a red control: name the assertion that fails with the arm removed.
3. **Also:** check every other walk that decides "reached" (`received remove`'s plan, `doctor`'s unreached stage,
   `show`'s state). Each must either use this walk or reach manifests itself. Name each.
4. **The smoke:** a section with a 65-block edited-file history, asserting `unreached objects: 0`, on the release
   build. Name it on 0.55.0 too (it passes there vacuously: 0.55.0 counts no Blobs).
5. **CHANGELOG:**
   - **no Fixed entry**, because the unreached Blob count is unreleased; fold the fix into the "`verify` and `doctor`
     could not see unreached patches and blobs" story, if any wording there needs it;
   - **Security (a) and (b)'s "Affected"** become "0.53.0, 0.54.1 and 0.55.0 (measured), and every earlier release that
     has `bundle import` or `archive import` (from reading the code; not run)". The external review measured 0.53.0
     and 0.54.1. (c) and (d) are unchanged.
6. **No new release commit is needed:** the version is already 0.56.0 on `main`. If the tag day is not 2026-11-07,
   the last commit re-dates the CHANGELOG heading. On the final commit: tests, the 14 gates, the smoke on the release
   build, and `matrix.py` **v7** (from `receive/046…/reproduce/`) at both depths with and without `--hexflip`,
   compared by row with the external architect's `ffa581db` logs.

**P4 ACCEPTED 2026-11-08** (`7fe71628`, pushed, CI green `42383276299` 17/17; review `release-0.56.0-prep-P4-review-v1`).
**The 0.56.0 candidate is now `7fe71628`.** The architect rebuilt it: a 130-block edited-file history reads `unreached
objects: 0`, and the full smoke gives 1074 ok. Letter 047 to the external architect is drafted, and the owner sends
it. **For the dev team, only on the tag day:** the CHANGELOG heading says 2026-11-07, so a one-line re-date commit
comes last. The architect pushes it and reads its CI before the tag.

**External review 047 (2026-11-08): nothing blocks the tag** (reading `external-047-reading-v1`). Their build of
`7fe71628` is byte-identical to ours, and 046 §0 is gone in every shape. **Not live yet; it waits for the owner's
word on the tag:** the tag-day re-date commit. One line, `## 0.56.0 — 2026-11-07` → the tag day, `CHANGELOG.md`
only, one-line message. Then `cargo test --workspace` and the 14 gates on it, and the release binary's sha256 (it
must equal `e0a4208e02d0…`, since only the CHANGELOG changed). The architect pushes it, reads its CI, and tags it.

**2026-11-08: the owner authorized the tag ("Authorized.") and crates.io ("Authorized.").**

**Next, and live: P5, the tag-day date** (one sitting, 15 min, stop at 30; report `release-0.56.0-prep-P5-report.md`):
1. **In `CHANGELOG.md` only:** `## 0.56.0 — 2026-11-07` → `## 0.56.0 — <the day you do this>`. Keep the em-dash
   (U+2014), because `release-notes` extracts the section by that shape. One commit, one-line message.
2. **On it:**
   - `cargo test --workspace`;
   - the 14 gates;
   - the release binary's sha256, which must equal `e0a4208e02d0…` (only the CHANGELOG changed);
   - `git show --stat`: one file, one line.
3. **Nothing else.** The architect pushes it, reads its CI, tags it (signed), watches the release run, and publishes
   to crates.io. If the push slips past that day, the heading is re-dated again on the tag day.
