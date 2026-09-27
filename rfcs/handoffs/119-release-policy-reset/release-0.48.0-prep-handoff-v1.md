# Release 0.48.0 preparation, as a candidate — the dev team's part

**Live 2026-09-27, and it is next. The owner authorized the prep:** *"Yes, you may."* The question was: may 0.48.0
release prep start, producing a release candidate first under RFC 152 §7.

**The architect's reading, stated so it can be corrected:**
- this round prepares a **candidate**, not a cut;
- **the tag is cut only after** the candidate passes the gates, CI and the crash-and-corruption matrix, **and** the
  external architect has re-run their script and matrix against it;
- **0.48.0 is not called "stable"** (owner, 2026-09-27: *"We are out of 'early implementation' but not in 'stable
  version'"*). The notes say precisely which promises hold;
- **crates.io is not authorized.** Publishing 0.47.0 together with 0.48.0 needs the owner's own word at publication
  time.

Follow `release-prep-handoff-template.md`; this file fills in the blanks, and adds the external review's release-prep
items (letter 014: D2, D4, D5, D9, D10).

| | |
|---|---|
| version | **0.48.0**: a minor, because in v0 any runtime change is one |
| theme | **"recovery and cost at depth"**: seal without re-walking the lineage; writes and reads that no longer follow the store's size; repairs that keep what they remove and never report damage as clean; a recovery model where caches never refuse and logs end at their last sound record; runaway-loop guards; a release tied to green CI |
| CHANGELOG date | the day the release commit is prepared; if it slips past midnight JST, stop and ask |
| last tag | `0.47.0` at `21895f46` |
| memory ratio (§1.6) | against **0.47.0 under the trimmed profile, 1.916×** (template §1.6), never the old 1.910× |

## 0. Before the sweep

- **CI** on `origin/main`, job by job; the macOS and Windows mutation suites by name. The last pushed commit is
  `d89b5d7d`.
- **Every run in this round goes through R1's cgroup scope** (RFC 160 §9), perturbations and measurements included.

## 1. Readiness sweep: the template's eight steps, with these specifics

1. **`--help` (§1.1).**
   - New since 0.47.0: `doctor --repair-pointer-index-tail`.
   - Changed: the output of `doctor --repair-index` (lost ids, the recovery file, a non-zero exit), and of
     `--repair-wal-tail` (the recovery file, and its refusals).
   - **Owed since 0.47.0:** `--max-object-bytes N` measures the **encoded** size, so a file of N bytes needs N + 69.
     Say so in its help line, and render byte counts in KiB where the text gives one.
2. **CHANGELOG (§1.2), in RFC 161's shape, first in the section:**
   - **`### Security`:** list every fix reachable by untrusted input (a received bundle, an exchange artifact, a peer's
     objects) in `## Unreleased`, and bring the list to the architect for a ruling before you write the block.
   - **`### Upgrading`** must include:
     - **the first seal after upgrading walks the whole history once** (RFC 159: the replay-verified record is keyed to
       the crate version; at depth 1,024 that is 0.47.0's 1.8 GiB, once; `prikk verify` refills it too);
     - the refusal messages that changed;
     - the `#[non_exhaustive]` types.
   - **`### Output changes`:** consolidate the RFC 160 and RFC 162 bullets into one list.
   - **One spelling for a breaking change:** `### Changed — breaking once: …`.
   - **0.44.0's annotation (RFC 161 §6.1):** one dated line under its heading naming GHSA-px5q-233r-6hq5 for its first
     `### Fixed` entry. **Also list every fix in 0.40–0.47 reachable by untrusted input**, for the architect's ruling on
     which get the same line.
3. **Docs (§1.3), plus the external review's documentation blockers:**
   - **D2, known limitations disclosed.** `docs/src/reference/current-state.md` names each open limitation with its
     measured figure and the release planned for it:
     - **F1:** ref publication replays the ref log three times, about 4.3 KB per generation;
     - **F2:** listing by type reads the whole container;
     - **F4:** a one-file commit reads every stored blob;
     - **M4:** `verify` is silent over garbage in the ref log;
     - **M5:** resync is quadratic on hostile content; 2 MiB takes about 14 s;
     - **M8:** a commit's cost follows the number of refs, 72 ms at 400;
     - a commit holds all its new content in memory;
     - **AUD-01:** the index lookup is linear.
   - **D4:** where `platform-support.md` and `durability-recovery.md` say "the full suite" for Windows and macOS, write
     "the suite that compiles there", with **the test count per platform read from the CI logs** of the candidate's run.
     Put the counts in the report.
   - **D5:** `durability-recovery.md` says that **on Windows, the recovery file's save is not claimed durable**
     (`atomic_replace` there is not), so "a repair keeps every byte it removes" holds as written on Linux and macOS.
   - **D9, the documents agree with each other.** The ROADMAP row no longer gives 0.48.0 two scopes (it is this theme,
     not "large objects"); `CONTRIBUTING.md` says "independent design review" only as far as it is true (author review
     plus the external review of 014); "`cargo audit` on every push" matches the workflow that runs it; the Unreleased
     CHANGELOG no longer says both "breaking once" and "no change to the public Rust API". **`MILESTONES.md` is the
     owner's, so do not edit it.** List any disagreement you find there, for the owner.
   - **D10:** `durability-recovery.md` says that only the WAL and index repairs write `recovery/`, and the ref log's
     truncation does not.
4. **Public API (§1.4):** the root-export and struct-shape diff from `0.47.0`. Every addition is named in the
   CHANGELOG, and every new report type is `#[non_exhaustive]`.
5. **`cargo package --list -p prikk` (§1.5).**
6. **Memory ratio (§1.6)**, the trimmed profile, compared with 0.47.0's 1.916×. Also:
   - **the store-size row (RFC 160 P5):** peak RSS and bytes read of a one-small-file commit at an 8 MiB and a 256 MiB
     blob container, which should be flat;
   - **RFC 133's residual (RFC 160 P6):** how much of the N = 64,000 residual the RFC 102 fix removed. It is expected to
     move, and the RFC 102 fix is the explanation to **check**, not to assume;
   - **`ARMS_BUDGET` = 9,000 s**, the one-line change owed since the measurement-budget round.
7. **Smoke script (§1.7)**, covering the recovery commands too: every repair, on a real damaged fixture, reaching a
   repository that accepts a commit.
8. **Absence claims (§1.8).**

## 2. The candidate

- The release commit is exactly as the template says: the version and pins, the lockfile's member versions, and the
  CHANGELOG heading.
- **Before it is proposed as the candidate:** the 14 gates on it, in R1's scope; the RFC 162 matrix green; the external
  `reproduce.sh` run against its release build **and** against 0.47.0's, both outputs attached, M1–M3 closed and M5 at
  or below `bb81b0fb`.
- **The architect** re-runs the gates, pushes, reads CI job by job, and sends the candidate to the external architect.
  **No tag until their answer**, or until they say not to wait.

## 3. Not the team's

The tag (signed, with the Release-page link in its message, RFC 161 §4.3), the release workflow's CI gate, verifying the
artifact, crates.io (on the owner's word), and the consumer letters (from `### Output changes`; brygge waits, by the
owner's ruling).

## 4. Units and budgets

| unit | what | budget (stop at ×2) |
|---|---|---:|
| P1 | the release-gate memory profile (template §1.6) | 20 min |
| P2 | the store-size row (P5) | 5 min |
| P3 | the external `reproduce.sh`, candidate and 0.47.0 | 5 min |

Report: `.git-exclude/review-request/release-0.48.0-prep-report-v1.md`.
