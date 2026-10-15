# Release 0.52.0 preparation, as a candidate — the dev team's part, in three one-sitting parts

**Live 2026-10-14, and it is next.** 0.52.0's planned work is complete (the owner's early cut, *"(a)"*, steps 0–2):
- **step 0:** the archive's closure follows the store;
- **step 1:** export refuses what import refuses, plus three texts; the Windows timeout and `gates.py`; `show`
  resolving each patch at that patch; `trust maintainer check` exiting 1 for "not trusted";
- **step 2:** one decode primitive for the store and the archive.

**The architect's reading, stated so it can be corrected:**
- **this prepares a candidate, not a cut;**
- **the external review is the owner's decision at the candidate.** I recommend offering it: step 0 rewrote the
  archive's acceptance of untrusted input, and step 2 rewrote the frame classifier both readers use;
- **no tag and no crates.io** without the owner's word.

Follow `release-prep-handoff-template.md`. This file fills in the blanks and splits the work into parts.

| | |
|---|---|
| version | **0.52.0**: a minor |
| theme | **"an archive restores what `verify` passes"**: the closure fix, export refusing what import refuses, and one frame classifier for the store and the archive; with `show` per patch and `trust maintainer check`'s exit status |
| CHANGELOG date | **the day of the tag** (the owner's rule), set in the last commit before it |
| last tag | `0.51.0` at `41c0bd1d`. The published Linux binary is kept at `/home/nabbisen/.pgtmp/rel-0.51.0-bin/prikk` (`prikk 0.51.0`) |
| memory ratio | against **0.51.0's 1.874×** (release-gate profile, the ratio of record in `release-0.51.0-prep-P3-review-v1`) |

## The working rule

- **One part per sitting,** in order, with one final report each. The report lists every item with its status.
- **No file in `.git-exclude/review-request/` until the part is complete.** If you must stop, end with *"Continuing — not
  ready for review"*.
- **`date` at each part's start and end. Every run inside an R1 scope.** Keep every output the report cites, gate logs
  included (`| tee`). Build before running anything that reports on a commit.
- **If a worktree is used,** paste `git worktree list` after removing it.

## Parts

| part | items | budget (stop at ×2) | report |
|---|---|---:|---|
| **P1: the sweep and the wording** | **0.** CI on `origin/main`, every job, and the Windows time. **1. W1, the one wording pass** (review `0.52.0-step-2-B-review-v2`): the two base messages ending *"…: an interrupted append, not damage"* (`interrupted_append_before_sound_frame_message`, and the overrun message) describe the frame only; each wrapper gives the verdict. Pin the four resulting lines (the store's two connectivity branches, and the archive's line for each shape), and update the CHANGELOG quotes. One commit, 14 gates. **2.** `--help` against 0.51.0's binary: every changed description (`archive export`'s refusals, `trust maintainer check`'s exit status, `show`). **3. The `### Security` candidates: a list to the architect, and no block written yet.** Start from 0.51.0's shipped `archive verify`. It never read the carried ref log, and it checked ref targets untyped, so a crafted archive could pass `verify` and import refs the destination's own `verify` then refused. Say whether anything was written that should not have been. **4.** The docs currency and the docs-debt grep: `backup-restore.md`, `archive-format.md`, `current-state.md` (the 0.51.0 closure entry reads *"Fixed in 0.52.0"*; the 256 MiB limit), `guide/show.md`, `trust-threat-model.md`, `development.md` (`gates.py --list`, and its refusal of other arguments, in one sentence). **5.** The public-API diff from 0.51.0, with `#[non_exhaustive]` on every new or changed public report type and enum (`UnreferencedRemnant` in `ArchiveVerifyReport`, anything new from step 2). **6.** `cargo package --list -p prikk`. **7.** The CHANGELOG process clause: the `### Changed` entry for `trust maintainer check` drops *"Reviewed against this project's own standing philosophy"* and keeps the user-facing reason | 90 min | `release-0.52.0-prep-P1-report.md` |
| **P2: measure and smoke** | **1.** The release-gate memory profile against 1.874×. **2. The archive at scale,** release build, idle machine, 3 samples: export, verify and import at about 1 GiB of blobs (8 × 128 MiB), against step 1's figures (export about 0.6 s and 12 MiB, verify about 388 MiB), plus verify on the patch-heavy archive (about 184 MiB). Import's promise in `current-state.md` (about 5× the largest object) must still hold. **3. RFC 133's release-gate `verify` timing,** which step 2A substituted with a 4,000-node repository: the real profile, on the release build and on 0.51.0's binary. **4.** `show <block-id>` on the 64-patch instrument, against step 1 Part C's figures. **5.** `smoke-0.52.0.sh`, extending 0.51.0's script. Add: `archive export` refusing an over-bound object (`#[cfg]` seam, or a real 257 MiB file) and a blocked ref publication, naming `ref complete` only when completable; the two closure shapes round-tripping (`sync accept` unsealed, edited-then-deleted); `show <block-id>` equal to the queued form for a rename and a delete; `trust maintainer check` exiting 0, 1 and 2; `gates.py --only` refused. Run it on a release build. **No `rm -rf` on a variable** | 90 min | `release-0.52.0-prep-P2-report.md` |
| **P3: the candidate** | After the architect's Security ruling: **1.** the CHANGELOG in RFC 161's shape (Security if ruled, Upgrading, **one** Output-changes block, Fixed, Changed, Added), with step 0's entries reading as one fix, not as part names. **2.** The three-file release commit (version, lock, CHANGELOG heading with today's date, re-dated at the tag if needed). **3.** The 14 gates; the RFC 162 matrix; the external `reproduce.sh`; and `matrix.py` v6 (`receive/025-…/reproduce/`) at `--compactions 1` and `2`, on the release build **and** on 0.51.0's binary, compared by row. **Name every row that differs, and why** (0.52.0 changes the archive and two `verify` lines). All outputs kept in `.git-exclude/review-request/release-0.52.0-prep-*` | 75 min | `release-0.52.0-prep-P3-report.md` |

## Not the team's

- pushing;
- the external-review question;
- the tag;
- verifying the artifact;
- crates.io;
- the consumer letters. The owner sends them after the release: stikk (`show`, `trust maintainer check`, the archive)
  and planeter (the archive is now restorable).

**P1 ACCEPTED 2026-10-14** (`a395ba96`, `9cad5d1f`; review `release-0.52.0-prep-P1-review-v1`). **`### Security`: one
entry, "Upgrade. No advisory."** 0.51.0's `archive verify` and `import` accepted an archive whose branch ref targets a
non-Block, or whose ref log disagrees with its refs, and import wrote it.
**P3 gains item 0:**
- one crafted archive (a branch RefState targeting a carried Blob). On 0.51.0's binary, verify exits 0 and import
  lands it. On the candidate, verify names the damage and import refuses, with the destination untouched. Outputs kept;
- then the entry, in RFC 161's shape, saying only what was shown.

**Next: P2.**

**P2: the measurements are ACCEPTED 2026-10-14** (review `release-0.52.0-prep-P2-review-v1`):
- the ratio is 1.891× against 1.874×;
- import is 5.03× the largest object, and flat at 2 GiB;
- RFC 133's `verify` at 64,000 nodes is not slower than 0.51.0.

**P2b (about 30 min, `release-0.52.0-prep-P2b-report.md`), next:**
- 29b uses stikk's two-patch shape: patch 1 edits two files, patch 2 renames one and deletes the other. Each queued
  patch must equal its entry in the block;
- 29d imports and verifies the destination;
- section 29 runs on 0.51.0's binary. Every line that passes there is named and explained.

**Then P3.**

**P2b ACCEPTED, and P2 CLOSED, 2026-10-14** (review `release-0.52.0-prep-P2b-review-v1`). The architect re-ran the
smoke on a current release build: exit 0, **330 ok**. Section 29 fails on 0.51.0 where it should, and every line that
passes there is explained. **Next: P3, starting with item 0** (the Security demonstration).

**P3 ACCEPTED 2026-10-14** (`8e7a5a8c`, `99bba962`; review `release-0.52.0-prep-P3-review-v1`). The Security
demonstration is on the binary both ways. The CHANGELOG is in RFC 161's shape, with "breaking once" agreed. 14 gates;
`matrix.py` v6 is byte-identical to 0.51.0, and the reason is traced.
**P3b (about 30 min, `release-0.52.0-prep-P3b-report.md`), next:** `prikk verify` fails on a received ref without ever
naming it. `received_ref_item_outcomes` is counted in the verdict, and the prose never prints it. Print each failed
received ref by name with its message, correct `main.rs`'s stale comment, add a CLI test shown red on `99bba962` (with
no double report for a not-adopted ref), a CHANGELOG Fixed and Output line, then 14 gates and the smoke.
**Then the candidate goes to the owner.**

**P3b: the fix is ACCEPTED 2026-10-14** (`91b919bf`, `5b8f7c81`; review `release-0.52.0-prep-P3b-review-v1`). `verify`
now names a failed received ref.
**P3c (about 20 min, `release-0.52.0-prep-P3c-report.md`), next:** the new test lands its damaged shape through
`/home/nabbisen/.pgtmp/rel-0.51.0-bin/prikk` and passes silently without it. So it checks nothing in CI, and it ships a
local path in the crate. Build the destination's state in-process with the store's test support (a Blob, a branch
RefState targeting it, the received pointer). No outside path, no early return. Red on `bde2e5bf`, then 14 gates.

**P3c ACCEPTED 2026-10-15** (`81f815fc`, `77ddc2dd`; review `release-0.52.0-prep-P3c-review-v1`). **The candidate is
complete.** The architect pushes it through the gate service. The owner decides the external review, the advisory
ruling and the tag. The CHANGELOG date is re-dated at the tag. **Nothing is open for the team.**

**External review 026 (on `39b5938c`, since the letter went out before the candidate was pushed): nothing blocks.**
Reading: `.git-exclude/reviewed/external-review-026-reading-v1.md`.
**P4 (about 30 min, `release-0.52.0-prep-P4-report.md`), next:**
1. **The Security entry gains one sentence,** shown first on a real repository:
   - a receiver that imported such an archive with 0.51.0 keeps the received pointer, and its `verify` fails on it;
   - a later `bundle import` (or `archive import`) of that ref replaces the pointer, and `verify` then passes;
   - prikk has no command that removes a received ref.

   Build that receiver in-process as P3c did, import a sound bundle of the same ref, and run `verify`. Keep the output.
   **If the replacement does not clear it, stop and report.**
2. **The date commit:** `## 0.52.0 — <today>`, with `date` run first. One line.
3. **The 14 gates** on that commit, alone.

**Not the team's:** the push, letter 027, and the tag.

**P4 stopped at item 1, correctly** (review `release-0.52.0-prep-P4-review-v1`). A sound import replaces the dangling
received pointer, but the old untrusted `RefState` stays in the store, and `verify`'s trust check (over every stored
`RefState` and `Block`) keeps failing. No command removes stored objects. The sentence "verify then passes" was false.
**P4b (about 30 min, `release-0.52.0-prep-P4b-report.md`), next:**
1. the Security entry's added sentence, in the review's exact wording (what remains, the pointer replaced, the objects
   kept and still failing trust, no removal command, the `.prikk/` copy as the way back);
2. one `current-state.md` limitation entry for content imported under an unadopted key, naming only the import paths
   source confirms;
3. the date commit (`date` first);
4. the 14 gates on the final commit.

**P4b ACCEPTED 2026-10-15** (`59a3bcf9`; review `release-0.52.0-prep-P4b-review-v1`). The Security sentence and the
limitation entry (`bundle import` and `archive import`; `sync accept` writes no `Block` or `RefState`) are in, and the
release is dated 2026-10-15. **The final candidate is the architect's record commit on `59a3bcf9`.** It is pushed
through the gate service, and the external architect re-runs on it after letter 027. **Nothing is open for the
team.**

**External review 027 (on `6e5337d0`): nothing blocks the tag.** Reading: `.git-exclude/reviewed/external-review-027-reading-v1.md`.

**0.52.0: RELEASED 2026-10-15.** The tag `0.52.0` is signed and sits on `6e5337d0`, the commit CI ran on (the owner:
*"Yes. Authorized to cut 0.52.0."*).
- **CI run `38645581515`:** 16/16.
- **Release run `38663325609`:** all six jobs green.
- **The architect verified the assets:**
  - checksums, 6/6;
  - build info, 4/4, naming `6e5337d0` and tag `0.52.0`;
  - the smoke on the published Linux binary, 330/330;
  - the Release page carries the CHANGELOG section byte for byte, followed by the standard footer.

**crates.io: PUBLISHED 2026-10-15** (the owner: *"Yes. Authorized, too."*). All eight crates are at 0.52.0, published
in dependency order from the tag's worktree, and each is confirmed on the crates.io index. **Letter 028** carries the
release run's id; the owner sends it.
