# Release 0.55.0 preparation, as a candidate — the dev team's part, in three one-sitting parts

**Live 2026-10-28, and it is next.** The owner authorized release prep on 2026-10-28 (*"Authorize prep"*). 0.55.0's
planned work is complete (theme *"memory and reads that follow size"*, owner-approved 2026-10-24):
- **G2:** the CLI tests grouped by area; no user-facing change;
- **step 1:**
  - `extract` refuses a destination inside the repository (`destination-inside-repository`);
  - `claim-parent-not-sealed`;
  - the checkpoint-record docs;
  - `ArchiveUnreferencedRemnant` and the re-exports;
  - the Windows uninstaller deletes a `Path` value it created;
- **step 3:**
  - id lookups in constant time (2C);
  - two syncs per object written instead of seven, on every append;
  - `bundle import` streams a regular file: two passes under the write lock, a per-frame digest, early buffer release;
  - `bundle verify` and `bundle preview` stream too, with one shared closure validator;
  - the front-page install tabs fixed (live since `7d886ac7`).

**The architect's reading, stated so it can be corrected:**
- **This prepares a candidate, not a cut.**
- **The external review is planned, at the candidate** (owner-approved plan, 2026-10-24), because 2A is a write path
  that takes untrusted input. The architect drafts the letter after P3.
- **No tag and no crates.io** without the owner's word.

Follow `release-prep-handoff-template.md`. This file fills in the blanks and splits the work into parts.

| | |
|---|---|
| version | **0.55.0**: a minor. Runtime changes, and the store's public API changes (`import_bundle`, `verify_bundle` and `preview_bundle` take a `BundleSource`) |
| theme | **"memory and reads that follow size"** |
| CHANGELOG date | **the day of the tag** (the owner's rule), set in the last commit before it |
| last tag | `0.54.1` on commit `2d8ec0d5` (runtime identical to `0.54.0`). The published 0.54.0 Linux binary is at `/home/nabbisen/.pgtmp/rel-0.54.0-prikk` |
| memory ratio | against **0.54.0's 1.8804×** (release-gate profile, `release-0.54.0-prep-P2-review-v1`) |
| smoke base | `.git-exclude/review-request/smoke-0.54.0.sh` (438 ok) |

## The working rule

- **One part per sitting,** in order, with one final report each. The report lists every item with its status.
- **No file in `.git-exclude/review-request/` until the part is complete.** If you must stop, end with *"Continuing — not
  ready for review"*.
- **Paste the `date` output when the work starts and when it ends.** Every run inside an R1 scope. Keep every output the
  report cites, gate logs included (`| tee`). Build before running anything that reports on a commit.
- **Every timing is alternated** (before and after in turn, order reversed each round). State the filesystem.
- **If a worktree is used,** paste `git worktree list` after removing it. **No `rm -rf` on a variable.**

## Parts

| part | items | budget (stop at ×2) | report |
|---|---|---:|---|
| **P1: the sweep and the wording** | **0.** CI on `origin/main`, every job by name, the Windows store job included. **1.** `--help` against 0.54.0's binary: every changed line, named. **2. The `### Security` question: a list to the architect, and no block written yet.** The architect's expectation is **no Security entry**: the streaming import closes no hole that 0.54.x had (0.54.x read the whole file once, so there was no window between passes), and every refusal still comes before the first write. Say whether you agree, and whether anything else is a security fix. **3. Docs currency and the docs-debt grep:** every `docs/src` statement about `bundle import`/`verify`/`preview` memory (whole file read, multipliers), `verify`'s cost at depth, sync and durability (directory syncs), and the object-store lock scope must match the binary. Include `current-state.md` and `commands.md` ("What is still true until RFC 158 Stage B"). No round names in user text. **4. The public-API diff from 0.54.1:** every change named. `import_bundle`/`verify_bundle`/`preview_bundle` and `BundleSource` are breaking for a library user, so they go in the CHANGELOG with what to call instead. Put `#[non_exhaustive]` on every new or changed public enum and report type, or one doc line saying why not. **5.** `cargo package --list -p prikk`. **6. The CHANGELOG read through as a user.** `## Unreleased` has grown entry by entry across step 3, so fold it into one coherent story per command:<br>• the same fixture under the same label everywhere, with each bundle's size stated;<br>• no unreleased intermediate figures;<br>• no "pass 1/pass 2", "this part", "Part 3b-…", "target";<br>• the import, verify and preview memory, the index lookup, and the sync reduction each said once | 75 min | `release-0.55.0-prep-P1-report.md` |
| **P2: measure and smoke** | **1.** The release-gate memory profile against 1.8804×. **2. Re-confirm every figure the CHANGELOG states on the candidate,** by the method that produced it, alternated against 0.54.0's binary:<br>• `verify` and `bundle import` at 10,000, 30,000 and 100,000 objects (tmpfs);<br>• peak RSS for `bundle import`/`verify`/`preview` on the three shared fixtures;<br>• `commit` and `bundle import` of 1,000 and 10,000 files on the `/home` disk, on an idle machine (say how you checked it was idle).<br>Correct any figure that does not hold. **3. `smoke-0.55.0.sh`,** extending `smoke-0.54.0.sh`:<br>• **section 31:** `extract` refuses a destination inside the repository (absolute, relative, under `.prikk/`, reached through `..`), and accepts a sibling directory whose name starts with the repository's;<br>• **section 35:** the claim with an unsealed parent must now assert `claim-parent-not-sealed` (it pinned `damaged-repository`);<br>• **bundles:** `bundle import`, `verify` and `preview` from a regular file and from a FIFO give identical results; a re-import gives `new objects: 0`;<br>• **every new `refusal-v1` id reachable from the CLI.** Name any that is not (the digest mismatch is reachable only through a test seam, so say so).<br>Run it on a release build. Then run the new lines on 0.54.0's binary and name each that passes there, and why. The section 31 and 35 lines must fail there | 90 min | `release-0.55.0-prep-P2-report.md` |
| **P3: the candidate** | After the architect's rulings on P1: **1.** the CHANGELOG in RFC 161's shape: Security only as ruled; **Upgrading** for the library API change; **one** Output-changes block (`destination-inside-repository`, `claim-parent-not-sealed`); Fixed; Changed; Added. **2.** The three-file release commit: version, lock, and the CHANGELOG heading dated today (re-dated at the tag if needed). **3.** The 14 gates; the RFC 162 matrix; the external `reproduce.sh`; and `matrix.py` v6 at `--compactions 1` and `2`, on the release build **and** on 0.54.0's binary, compared by row. **Name every row that differs, and why.** The sync reduction removed every append's directory sync, so a crash row may now land differently. Each difference must be explained as safe: every name is created with a directory sync, and each append is still synced before the next | 75 min | `release-0.55.0-prep-P3-report.md` |

## Not the team's

- pushing;
- **the external-review letter** (the architect drafts it against the candidate; the owner sends it);
- the tag; re-running any CI job on the tagged commit that was cancelled for runner capacity, until the run is fully
  green;
- verifying the artifact;
- **the release run's jobs read by the architect:**
  - the AUR jobs, then both AUR pages at 0.55.0;
  - `windows-install-from-release`, all four cells;
- crates.io;
- **the consumer letters,** which the owner sends after the release:
  - **planeter:** the reply covering both of their letters (claim-chain `extract` and the forge rulings for 0.56.0),
    with what 0.55.0 changes for a forge (streaming imports, `claim-parent-not-sealed`, two syncs per object instead
    of seven, which bears on their `sync accept` throughput);
  - **stikk:** the new refusal ids;
  - **inklkunn:** the reply drafted 2026-10-28, if not sent before.

**P1 ACCEPTED 2026-10-28** (`dc10e079`, `b5b69720`; review `release-0.55.0-prep-P1-review-v1`). **Next: P2,** unchanged.

**P3 carries four rulings first:**
- **R1:** no `### Security` entry.
- **R2:**
  - `ExtractRefusalKind` gained `ClaimParentNotSealed` on what was an exhaustive public enum. Add `#[non_exhaustive]`,
    and correct its "new in this very release" doc;
  - `### Upgrading` names both breaks (the `BundleSource` signatures, and the non-exhaustive enum);
  - paste `cargo public-api diff 0.54.1..HEAD` on the candidate.
- **R3:** `concurrency-locking.md:267` names `bundle verify` and `bundle preview` among the commands that take no lock.
- **R4:** `current-state.md` gains this release's known limits: one large object at about 4.3× in `import` and about
  2.4× in `preview` (the rest is 0.56.0 work), and `preview` into a new repository holding every created file at once.
