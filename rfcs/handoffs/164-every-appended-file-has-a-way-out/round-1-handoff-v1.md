# RFC 164 round 1 — tails by position, nothing silent, one repair (Rules A, B, C)

**Live 2026-09-30, and it is next.** RFC 164 is ACCEPTED by the owner (*"Yes. RFC 164 is accepted."*), with both §6
decisions ruled: one `--repair-tails` verb, and Rule E classification only. **Read the whole RFC first**,
`rfcs/accepted/164-every-appended-file-has-a-way-out.md`, including the architect's reading in its Status. This round is
Rules A, B and C. **Rules D and E are round 2. Do not start them here.**

## 1. Rule A — a tail by position, for seven more files

**The files:**
- trust keys (`trust/keys.container`);
- trust policy (`trust/policy-{a,b}.container`);
- author keys (`trust/author-keys.container`);
- the received index (`refs/containers/received-index-{a,b}.container`);
- the three generation logs (`refs/containers/pointer-index-generation.log`, `refs/containers/received-index-generation.log`,
  `trust/policy-generation.log`).

`containers/generations.log` is created empty and nothing appends to it. Say so in the report, and leave it alone.

1. **Each file's decode applies RFC 162 rule 3, as the WAL's and the pointer index's already do.**
   - **Tail:** bytes after the last sound record, when no sound record follows, whatever their shape. The decode
     returns it as trailing bytes, not a failed item.
   - **Interior damage:** a sound record follows. It stays a failed item, naming its offset.
2. **Every reader tolerates a tail.** List every reader of these seven files from source, with its call site, and show
   each one no longer refuses over a tail. Interior damage still refuses where it did.
   - **The known refusals that must end for a tail:** "has a damaged entry; run doctor before reading" on the four files,
     and "generation log has a damaged record" on the three logs.
   - **The measured worst case:** 100 zero bytes after `pointer-index-generation.log` made `status`, `log`, `commit`,
     `seal`, `verify` and `doctor` all refuse. After this round every one of them runs.
3. **Every writer still refuses before appending behind a tail.** RFC 163's guards read the trailing count. Confirm they
   now catch zeros and garbage too, which rule 3 makes a tail, and name the refusal message each gives.

## 2. Rule B — nothing silent

**`verify` and `doctor` report, for every framed file:**
- a tail: a warning naming the file, the offset of the last sound record, the byte count, and the repair;
- interior damage: a failure, naming the offset.

**The files:** the WAL, the pointer index, the seven files above, the ref log (report only, M4), and the five object
containers (their short tails, N7).
- **Exit codes:** tails alone leave `verify` at 0. Interior damage exits 1.
- **JSON:** name the new fields, and keep the report types `#[non_exhaustive]`. This is an output change:
  `### Output changes`.
- **List each file's line** as `verify` prints it, in the report.

## 3. Rule C — `prikk doctor --repair-tails`

- **Covers:** the WAL, the pointer index, and the seven files of Rule A. **Not** the object containers, and **not** the ref
  log, which only report (RFC 164's Status, the architect's reading).
- **For each covered file with a tail:** save exactly the removed bytes to `.prikk/recovery/` (named per file, like the WAL
  repair's), durably, then truncate. Report per file what was removed and where it was saved. A file without a tail is
  reported as clean.
- **All or nothing on interior damage:** if any covered file has interior damage, refuse before touching any file, naming
  the file and the offset.
- **Idempotent:** a second run changes nothing and says so.
- **Locks:** each file under the same lock as its writers. Say which lock each takes, and in what order.
- **Exit codes:** follow `--repair-wal-tail`'s convention. State it.
- `--repair-wal-tail` and `--repair-pointer-index-tail` stay, unchanged, as the single-file forms.
- **`--help`, `commands.md` and `troubleshooting.md`:** the manual-truncation entries for the four files and the
  generation logs are replaced by `--repair-tails`.

## 4. Tests, controls, the matrix

- **`rfc162_recovery_matrix.rs` and `rfc163_*`**, for every file of §1: a torn prefix, 100 zero bytes and 100 random
  bytes, in both orders (the repair first, and the write first). Assert:
  - I1 to I5;
  - Rule B's line;
  - after `--repair-tails`, `verify` 0, and a commit accepted;
  - the recovery file holds exactly the removed bytes.
- **Controls, one at a time, at each site:**
  - restore a reader's refusal over a tail;
  - drop Rule B's line for one file;
  - remove one file from `--repair-tails`;
  - make `--repair-tails` touch files before checking for interior damage.

  **Each removal must redden its rows.** A site whose removal reddens nothing is a finding.
- **On a release build of the final commit:**
  - **the external `matrix.py` v4** (`receive/017-…/reproduce/`), compared with `matrix-5e50a661.txt`. Expected: the 21
    damaged-shaped-tail I5 cells gone, the N10 cells gone, the silent count down, **no cell worse**. Explain every
    remaining finding by name, **each with its own replay**;
  - **the architect's `rfc163_generation_log_tree_probe.sh`:** its `pointer-index/zeros100` row must now show `commit
    after: 0`;
  - **the architect's seven probes**, unchanged otherwise.

## 5. Text

- **`current-state.md`:** N2's remainder, N10, and N7 (except what is left) leave the known limitations, or shrink to what
  remains. The ref log's repair stays listed, for the F1 round.
- **`durability-recovery.md`:** Rule A's files, Rule B, `--repair-tails`.
- **CHANGELOG `## Unreleased`:** `### Added` (the verb), `### Changed`, and `### Output changes` (the new lines, the
  refusals that end).

## 6. Gates, units, report

- **The 14 gates on the final commit**, in R1's scope.

| unit | what | budget (stop at ×2) |
|---|---|---:|
| U1 | the matrix rows and controls | 60 min |
| U2 | `matrix.py` v4, the seven probes and the tree probe, on the release build | 45 min |

**Report:** `.git-exclude/review-request/rfc164-round-1-report-v1.md`.
