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
