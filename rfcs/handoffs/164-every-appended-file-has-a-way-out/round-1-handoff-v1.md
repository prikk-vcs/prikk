# RFC 164 round 1 — tails by position, nothing silent, one repair (Rules A, B, C)

**Live 2026-09-30, and it is next.** RFC 164 is ACCEPTED by the owner (*"Yes. RFC 164 is accepted."*), with both §6
decisions ruled: one `--repair-tails` verb, and Rule E classification only. **Read the whole RFC first**,
`rfcs/done/164-every-appended-file-has-a-way-out.md`, including the architect's reading in its Status. This round is
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

## Addendum 1 — 2026-10-01: a complete record is never a tail, I6, the object containers, the matrix and its controls (review `rfc164-round-1-review-v1`)

Report `rfc164-round-1-report-v1.md`, commits `bfa567db` … `09f416af`. **Not accepted yet.** The round implemented Rule A
as written. The rule was wrong in one place: the owner **ACCEPTED RFC 164 §9** on 2026-10-01. **Read §9 and §9.1 first.**

1. **§9, for the seven Rule-A files and the pointer index.**
   - A tail is an incomplete record (header or body short), or bytes that are not a record header, when nothing sound
     follows.
   - **A complete record whose checksum or envelope fails is damage, even when last.** Your `never_a_tail` flag is the
     natural place: it now also holds for a complete record whose checksum or envelope fails.
   - Readers fail closed on it, as on interior damage. `--repair-tails` and `--repair-pointer-index-tail` refuse, and
     change nothing.
   - **The WAL keeps RFC 162 rule 3 unchanged.**
2. **Invariant I6, in every repair row:** a repair never changes the meaning of committed state. Assert, before and after:
   - `trust maintainer list`;
   - every ref tip (`branch list`, tags, received pointers);
   - each compacting container's live slot.
3. **The rollback rows, which must now pass** (the architect's probes, `arch-seal/rfc164_rollback_probe.sh` and
   `arch-seal/pointer_index_flip_probe.sh`):
   - a flipped byte in the last complete record of the trust policy, a generation log and the pointer index. `verify`
     fails, the repair refuses, and **nothing reverts**: the removed maintainer stays untrusted, the branch stays, `main`'s
     tip stays;
   - also trust keys, author keys and the received index.
   - **Control:** restore "whatever its shape" at one file at a time. Its rollback rows go red.
4. **Rule B for the five object containers** (N7's short tails): a warning line, as for the other files. **The ref log's
   line moves to the F1 round** (my ruling): leave it out, and say so in the text.
5. **§4, as the handoff asked:**
   - the matrix rows for every file of §1: a torn prefix, 100 zero bytes, 100 random bytes, **and a flipped byte in the
     last complete record**, in both orders, asserting I1 to I6;
   - **the four controls, per site:** a reader's refusal restored; one file's line dropped; one file removed from
     `--repair-tails`; `--repair-tails` touching files before its check;
   - report each control with the rows it reddened. A site whose removal reddens nothing is a finding.
6. **Text:**
   - the §9 rule, in `durability-recovery.md`;
   - `troubleshooting.md`: a damaged complete record means restore from a copy; no truncation advice;
   - **the 0.48.0 disclosure** in the known limitations: `--repair-pointer-index-tail` can remove a damaged, not torn,
     last record, and the ref then reverts. Fixed in 0.49.0 by §9;
   - CHANGELOG to match.

**Before proposing:**
- the 14 gates on the final commit, in R1's scope;
- `matrix.py` v4 compared with `matrix-5e50a661.txt`, **every changed cell explained, each with its own replay**;
- the architect's probes, including the two above;
- the generation-log tree probe.

**Report:** `.git-exclude/review-request/rfc164-round-1-report-v2.md`.

## Addendum 2 — 2026-10-01: the checksum decides whether a record is complete (review `rfc164-round-1-review-v2`)

Report `rfc164-round-1-report-v2.md`, commits `b24a3283` … `284a5308`. **Not accepted yet.** Addendum 1 is implemented as
written. The design was wrong in one more place: a flipped magic, version or length byte in the last record still reads
as a tail, **and the readers roll back before any repair runs.**
- The owner **ACCEPTED RFC 164 §9.2** on 2026-10-01, with disclosure, not an advisory, for the released versions.
- **Read §9.2 and review v2 §2 first.**
- **Fixes only.**

1. **§9.2's rule, in the six decoders** (trust keys, trust policy, author keys, received index, generation logs, pointer
   index):
   - at a tail candidate, **if the stored checksum verifies** over the claimed length, or over the length to the end of the
     file, the bytes are a complete record: `never_a_tail`, whatever the stored magic, version or length say;
   - **one shared helper**, beside `sound_frame_after_partial`, called by each decoder;
   - the write-side scans too (`scan_received_index_tail` and any like it);
   - **the WAL is untouched.**
2. **Every reader, before any repair:** for any single flipped byte in the last record, no reader returns the state from
   before that record. List each file's readers (round 1 listed them), and assert it for each.
3. **The matrix:**
   - the "flipped byte" shape becomes **one row per field: magic, version, length, checksum, body**. For the seven files
     **and the pointer index**, in both orders;
   - **I6 for the §9 shapes:** no reader returns the older state, before or after the repair;
   - **the whole-record sweep:** a store-level test per decoder that flips every offset of the last record and asserts no
     offset decodes to a tail. It only decodes, so it is cheap.
4. **Controls, one at a time; report the rows each reddens:**
   - §9.2's helper bypassed, per decoder: its header-field rows go red;
   - §9's `never_a_tail` restored to `false`, per decoder, **including the pointer index**. Today the whole suite stays
     green without it (review v2 §6).
5. **`--repair-tails` on a damaged generation log:** the refusal names the file and says nothing was touched, as the
   other refusals do. Today it prints the reader's "run doctor before reading".
6. **Text and disclosure** (no advisory; the owner's ruling):
   - **rewrite the CHANGELOG `### Security` entry, and the known limitations in `current-state.md`, for the released
     versions:**
     - the trust policy: a flipped length byte in its last snapshot silently brings back the previous policy (a removed
       maintainer trusted again), with `verify` 0. **Find the first affected release from history** (`2827fab7`; the
       architect measured 0.46.0 and 0.48.0);
     - the pointer index, 0.48.0: its readers show the previous tip, and its repair removes the record;
     - fixed in 0.49.0 by §9 and §9.2;
   - what remains, in the known limitations: corruption spanning more than one field of the last record (for example a
     zeroed sector) still reads as a tail, until a per-file witness (format 8);
   - `durability-recovery.md`: "complete" is decided by the checksum.

**Before proposing:**
- the 14 gates on the final commit, in R1's scope;
- the architect's `arch-seal/rfc164_every_offset_probe.sh` and `rfc164_header_flip_reader_probe.sh` on a release build of
  the final commit: **0 rollbacks at every offset, before and after the repair**;
- `rfc164_rollback_probe.sh`, `pointer_index_flip_probe.sh` and the seven RFC 163 probes;
- `matrix.py` v4 compared with `matrix-5e50a661.txt`, every changed cell explained with its own replay.

**Report:** `.git-exclude/review-request/rfc164-round-1-report-v3.md`.

## Addendum 3 — 2026-10-01: the disclosure corrected, the sweeps strengthened, the readers in the suite (review `rfc164-round-1-review-v3`)

Report `rfc164-round-1-report-v3.md`, commits `0bc2e739` … `24ca5991`. **Not accepted yet. The code is right:** the
architect's every-offset probe over all eight files and five readers found 0 rollbacks. **Text and tests only; no
change under `crates/*/src`.** Read review v3, §2 to §4.

1. **Correct the disclosure** in CHANGELOG `### Security` and `current-state.md`, to review v3 §2's table:
   - **The trust policy, released versions: the length bytes only.** The removed maintainer is trusted again, and
     `verify` exits 0. Measured on 0.46.0, 0.47.0 and 0.48.0.
   - **The pointer index:** readers show an older tip on a flipped length byte, from at least 0.46.0. In 0.48.0 they do
     so on any byte, and the repair also removes a body-flipped record. `verify` 1.
   - **The pointer-index generation log:** readers resolve the older slot on a flipped length byte in 0.46.0 and 0.47.0.
     `verify` 1.
   - Magic and version flips are refused on every released version.
   - **The code first shipped in 0.20.0** (`2827fab7`, `0550e340`, `b33d1942`). Keep that history apart from the
     measured versions.
2. **The six sweeps** also assert a `Failed` outcome at every flipped offset, not only no tail.
   - **Control, per decoder:** stop recording the failure for a complete damaged record. That sweep goes red.
3. **The readers, in the suite:** one CLI test in the shape of `arch-seal/rfc164_all_files_reader_probe.sh`.
   - **Flips:** all eight files, each with at least two records. Five flips per file in the last record: magic, version,
     length, checksum field, body.
   - **Assert:** `verify` fails; `--repair-tails` refuses; each of `trust maintainer list`, `branch list`, `sync tags`,
     `status` and `log` either refuses or prints exactly its baseline, before and after the repair. Normalise the
     repository path.
   - **Control:** `complete_by_checksum` bypassed. The header rows go red.
4. **The helper bypassed at one decoder's call sites at a time.** Report the rows each run reddens.

**Ruled:** the CLI matrix needs no restructuring into one row per field. Items 2 and 3 replace it.

**Process:** a cut is a question to the architect before delivery, not a disclosure after it.

**Before proposing:**
- the 14 gates on the final commit;
- the architect's `rfc164_all_files_reader_probe.sh` on a release build: 0 rollbacks, 0 silent, 0 repairs;
- `rfc164_rollback_probe.sh`, `pointer_index_flip_probe.sh` and the seven RFC 163 probes.

**Report:** `.git-exclude/review-request/rfc164-round-1-report-v4.md`.

**ACCEPTED and CLOSED 2026-10-01** (`bfa567db` … `2a56be48`; reviews `rfc164-round-1-review-v1` … `-v4`; pushed `2a56be48`).
- **Verified:** the 14 gates on a clean build, and the architect's every-offset probe over all eight files and five
  readers on the release build (sha256 `10ab1b00…`): 0 rollbacks, 0 silent `verify`. The control on the pre-§9.2 build
  found 84 and 68.
- **Next is round 2** (Rules D and E): `round-2-handoff-v1.md`.
