# Release 0.51.0 preparation, as a candidate — the dev team's part, in three one-sitting parts

**Live 2026-10-11, and it is next.** 0.51.0's planned work is complete: step 1 (the 020/021 corrections and
`--keep-slot`), and RFC 155 (`prikk archive`), designed, accepted by the owner and implemented.

**The architect's reading, stated so it can be corrected:**
- **this prepares a candidate, not a cut;**
- **the external review is the owner's decision at the candidate.** I recommend it: an archive is a new kind of
  untrusted input;
- **no tag and no crates.io** without the owner's word.

Follow `release-prep-handoff-template.md`. This file fills in the blanks and splits the work into parts.

| | |
|---|---|
| version | **0.51.0**: a minor |
| theme | **"a whole repository in one file"**: `prikk archive export\|verify\|import [--adopt]`, `doctor --cancel-import`, `compact --keep-slot`, and the corrections reviews 020 and 021 graded |
| CHANGELOG date | **the day of the tag** (the owner's rule since 0.50.0), set in the last commit before it |
| last tag | `0.50.0` at `7551c3aa`. The published binary is kept at `/home/nabbisen/.pgtmp/prikk-0.50.0-7551c3aa` (checksum verified, `prikk 0.50.0`) |
| memory ratio | against **0.50.0's 1.854×** (release-gate profile) |

## The working rule

- **One part per sitting,** in order, with one final report each. The report lists every item with its status.
- **No file in `.git-exclude/review-request/` until the part is complete.** If you must stop, end with *"Continuing — not
  ready for review"*.
- **`date` at each part's start and end. Every run inside an R1 scope.**

## Parts

| part | items | budget (stop at ×2) | report |
|---|---|---:|---|
| **P1: the sweep** | **0.** CI on `origin/main`, every job, and the Windows time. **1.** `--help` against 0.50.0's binary: `archive export\|verify\|import`, `doctor --cancel-import`, `compact --keep-slot`, every changed description. **2.** **The `### Security` candidates: a list to the architect, and no block written yet.** **3.** The docs currency and the docs-debt grep: `backup-restore.md`, `archive-format.md`, `commands.md`, `troubleshooting.md`, `current-state.md`'s limitations (the archive's stated limits, the `bundle import` memory finding). **4.** The public-API diff from 0.50.0, with `#[non_exhaustive]` on every new report type and enum (`Archive*Report`, `KeepSlotReport`, `RecoveryEntryKind`, `ContainerGenerationAmbiguity`, `RetiredSlotTailNote`). **5.** `cargo package --list -p prikk`. **6.** The absence claims: no page says the archive is missing or unbuilt | 90 min | `release-0.51.0-prep-P1-report.md` |
| **P2: measure and smoke** | **1.** The release-gate memory profile against 1.854×. **2.** **The archive at scale:** release build, idle machine, at least 3 samples, about 1 GiB of blobs. Wall time and peak RSS for `export`, `verify` and `import`. **Import has not been measured for memory yet:** it must stay bounded (the prohibited shortcut was holding the archive in memory). **3.** `smoke-0.51.0.sh`, extending 0.50.0's: `--keep-slot` on the trust-policy twin (plan, then a real run, then `--recovery-list`); the carry-forward story (export; `init`; verify lists the maintainer keys; `trust maintainer add`; `import --adopt`; `branch list`; `prikk verify` exits 0); re-import reports *"already imported"*; export refuses with a queued commit; `doctor --cancel-import` on a repository with no journal refuses by name. Run it on a release build. **No `rm -rf` on a variable** | 75 min | `release-0.51.0-prep-P2-report.md` |
| **P3: the candidate** | After the architect's Security ruling: **1.** the CHANGELOG in RFC 161's shape (Security, Upgrading, one Output-changes block, Fixed, Added); RFC 155's entries read as one feature, not as part names. **2.** The three-file release commit (version, lock, CHANGELOG heading with today's date, re-dated at the tag if needed). **3.** The 14 gates; the RFC 162 matrix; the external `reproduce.sh` and **`matrix.py` v6** (from `receive/021`, at `--compactions 1` and `2`) on the release build **and** on 0.50.0's binary, all outputs kept in `.git-exclude/review-request/` | 60 min | `release-0.51.0-prep-P3-report.md` |

## Not the team's

- pushing;
- the external-review question;
- the tag;
- verifying the artifact;
- RFC 155's move to `done/` at the cut (the architect's records);
- crates.io;
- the consumer letters (the owner's, possibly after 0.51.0).

**P1 ACCEPTED 2026-10-11** (`65bc464b`; review `release-0.51.0-prep-P1-review-v1`). **No `### Security` block.** **P2 gains item 0:** `#[non_exhaustive]` on `ArchiveImportOptions` (in-tree literals move to `default()`). The archive's memory limits go into `current-state.md` after P2's import measurement. **Next: P2.**

**P2 approved in part 2026-10-11** (review `release-0.51.0-prep-P2-review-v1`). **P2b (90 min, `release-0.51.0-prep-P2b-report.md`), blocking:**
- (1) stage 1 of `archive import` streams within a type in container order, with per-object or byte-bounded admission; RSS bounded by the largest object, measured at 1 GiB in one type, 3 samples, plus a control; the doc comment and `current-state.md` corrected;
- (2) A/B the +1.3 to 1.9% ratio move (the journal check stubbed);
- (3) per-binary Windows test times against `50737d0f`.

**Next: P2b, then P3.**

**P2b ACCEPTED, and P2 CLOSED, 2026-10-11** (`38177fe7`; review `release-0.51.0-prep-P2b-review-v1`). Import RSS is bounded at about 5x the largest object, and flat when the archive doubles. The ratio move is measurement conditions: the architect's same-day A/B gives 0.50.0's code 1.911x mean and HEAD 1.874x. Windows CI time is runner variance (the same binary swung 1,574 to 1,101 s before any journal code). **Next: P3.**

**P3 approved in part 2026-10-11** (`328dbc3d`, `e6ec2560`; review `release-0.51.0-prep-P3-review-v1`). The release commit, the gates and the matrix v6 are accepted.

| part | items | budget | report |
|---|---|---:|---|
| **P3b: the notes include step 1** | Step 1's Parts A and B are unreleased (`git tag --contains 01e30337` is empty) but missing from 0.51.0's notes. Add one `### Fixed` entry, and six Output-changes lines with before and after: `ref complete` named in one step; the WAL trailing count for acknowledged damage; `VERIFY-OK` absent beside errors; `precondition not met:` in place of `integrity error:`; the retired-slot torn-tail warning; no interior-damage line beside the ambiguity issue. One commit on top of `e6ec2560`; 14 gates | 30 min | `release-0.51.0-prep-P3b-report.md` |

**Next: P3b.**

**P3b ACCEPTED 2026-10-11** (`e2292eb5`; review `release-0.51.0-prep-P3b-review-v1`). **The candidate is complete.** Three wording fixes ride the date commit at the tag. The architect pushes; the owner decides on the external review.

**External review 023 blocked the tag (2026-10-12).** RFC 155 parts I-C to I-F follow in
`rfcs/handoffs/155-repository-complete-artifact/implementation-handoff-v1.md`. P4 re-runs the candidate on I-F's
final commit. **It is live only once I-F is accepted.**

| part | items | budget (stop at ×2) | report |
|---|---|---:|---|
| **P4: the candidate again** | On I-F's accepted commit, untouched tree, release build. **1.** The 14 gates. **2.** `matrix.py` v6 at `--compactions 1` and `2`, on the release build and on 0.50.0's binary, compared by row as in P3. **3.** The external `reproduce.sh`, both binaries. **4.** `smoke-0.51.0.sh`. **5. The archive at scale, again,** because I-D and I-D2 rewrote the walk stage 1 uses: P2b's method (about 1 GiB of blobs in one type, idle machine, 3 samples). Wall time and peak RSS for export, verify and import, against P2b's figures. The promise in `current-state.md` (about 5× the largest object, flat when the archive doubles) must still hold, or the text changes. **6.** The CHANGELOG's 0.51.0 section read once as a whole against RFC 161's shape. All outputs kept in `.git-exclude/review-request/release-0.51.0-prep-P4-*` | 75 min | `release-0.51.0-prep-P4-report.md` |

**I-F ACCEPTED 2026-10-12 (`2bb03455`). P4 is LIVE**, with one item before the re-run:
- **Item 0:** the trust note ends *"adopts the signing keys"* (and *"once they are"*) when more than one distinct key
  is named, and *"key"* / *"once it is"* when one is. Extend `rfc155_part_if_trust_note`'s r1 to assert the plural, and
  r2 the singular. One commit.
- **Items 1–6** then run on that commit, untouched tree, as the P4 row above says.

**Next: P4.**

**P4 ACCEPTED 2026-10-13** (`f2cb3cde`; review `release-0.51.0-prep-P4-review-v1`), on the architect's own re-run:
- the matrix v6 is identical to 0.50.0 at both depths;
- `reproduce.sh` differs only in `info=0`;
- the smoke gives 317 ok;
- the archive's import is flat at about 5× the largest object.

**The candidate is `f2cb3cde`.** For the next prep:
- keep every output the handoff names, gate logs included (`| tee`);
- build before running anything that reports on a commit.

**Nothing is open for the team.** Next, the architect's and the owner's: push, CI, letter 024, the answer, the date
commit, and the tag.

**External review 024: nothing blocks the tag.** Part I-G (`3551fc92`, accepted) fixed one warning's wording, added both
signature counts, and set the date to **2026-10-13**. **The candidate is this record commit**, pushed through the gate
service. The tag goes on the owner's word, on the commit CI ran on. Nothing is open for the team.

**0.51.0: RELEASED 2026-10-13.** The tag `0.51.0` is signed and sits on `41c0bd1d`, the commit CI ran on (the owner:
*"Yes. Authorized."*).
- **CI run `38321958923`: 16/16.** In the first attempt, the Windows mutation suite was cancelled at its 65-minute limit.
  The runner was about twice as slow at every stage, and it was not a hang. The cancelled job was re-run on the same
  commit and passed in 37 minutes.
- **Release run `38330880100`:** all six jobs green.
- **The architect verified the assets:**
  - checksums, 6/6;
  - build info, 4/4, naming `41c0bd1d`;
  - the smoke on the published Linux binary, 317/317;
  - the Release page carries the CHANGELOG section byte for byte, followed by the standard footer.

**crates.io: PUBLISHED 2026-10-13** (the owner: *"You may also publish."*). All eight crates are at 0.51.0, published in
dependency order from the tag's worktree, and each is confirmed on the crates.io index. **RFC 155 moved to `done/`.**
**Letter 025** carries the release run's id; the owner sends it.
