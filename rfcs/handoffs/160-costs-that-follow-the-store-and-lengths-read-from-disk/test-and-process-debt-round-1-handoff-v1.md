# 0.49.0 step 5, round 1 — the gates as one script, the guards' reach, honest labels

**Live 2026-10-05, and it is next.** 0.49.0 step 4 (RFC 167) is closed: review
`.git-exclude/reviewed/rfc167-implementation-review-v1.md`.
- **What step 5 is:** the owner-approved schedule's "test and process debt", mostly from external review 014's
  D-series (`.git-exclude/upstream/external-architect/receive/014-review-before-0-48-0-findings-and-answers/014-review.md:222-229`,
  plan `.git-exclude/scratch/0.49.0-plan-proposal-v1.md:37-47`). **Those D-labels are not RFC 167's D1–D6.**
- **Why it is filed here:** most items are RFC 160's guards (P1–P5). The rest are how those guards and gates are run.
- **Round 1 (this file):** D8, D12, D11 with the command rows, the `require_progress` scan, and the object containers'
  label.
- **Round 2 (later):** a first fuzzing target, existence checks for RefState, Tag and Attestation references, and
  D4's Linux-only tests.

**The rules carried from RFC 166's and 167's rounds:**
- **A cut is a question to the architect before delivery,** at ×2 of a unit's budget.
- **Every guard has a control that can fail,** and the control is stated with its result.
- **Read the code before writing a claim about it,** and cite the line.
- Measurements: a release build with its sha256, on `/home`, with the filesystem printed; R1 scopes with timeouts.
- **Tests that need `--all-features`** (the `test-support` feature) say so where they are named.

## 1. D8: the gates as one script (U1)

1. **`scripts/gates.sh`** (or the name you justify) runs all 14 gates, exactly as `rfcs/EXECUTION-ORDER.md` §6 rule 9 lists
   them, and reports each one's exit status. **No gate is conditional** in the script. Where a rule is conditional
   (the cross-target clippy runs), run it anyway: running it costs little, and skipping it already let a Windows
   failure through twice.
2. **R1 inside it:** when `systemd-run --user` is available, every gate runs in its own scope, with a memory ceiling and
   a timeout. Otherwise the script says plainly that it ran without them.
3. **One summary, machine-readable:** one `name exit` line per gate, and an exit status that is 0 only when all 14
   are 0.
4. **The documents** (`rfcs/EXECUTION-ORDER.md` and the contributor copies) name the script as the way to run the gates.
   The contributor copies' missing `size-check` is fixed by this.
5. **Control:** break one gate (for example a formatting change), and show the script reports that gate and exits
   non-zero.

## 2. D12: one shared release-binary check (U2)

1. Every instrument under `crates/prikk-cli/tests/` that measures time, memory or bytes read refuses an unoptimised
   binary, through **one shared helper**, as `tools/corpus/tests/support/mod.rs:45-95` already does for the corpus.
2. **List every instrument from source** (grep for `CARGO_BIN_EXE_prikk` in measuring tests), and say which ones you
   changed and why the rest are not measurements.
3. **Control:** a debug build run of one instrument refuses, saying why.

## 3. D11: the guards' reach, and the command rows (U3)

1. **P1** (`whole_read_guard.rs:79-95`): add the pointer index, the received index, the trust policy and the WAL to the
   families it sees. Each new family gets a control: a whole-file read added to that family's reader turns it red.
2. **P2** (`store_size_independence.rs`): the refs axis and the command rows asked for at 014-review.md:290.
3. **The command-level hashing rows RFC 167 D4 asked for and did not get.** The architect accepted the WAL row alone,
   and that was the architect's miss.
   - Add a hostile **blob container** row and a hostile **ref log** row beside the WAL's
     (`runaway_guards.rs::verify_hashes_a_hostile_wal_within_k_times_its_size`), each a whole `verify`, at most k× the
     input;
   - **keep a control as code this time:** a test-only switch that adds a second decode and asserts the row goes red.
     If that cannot be done without a production hook, say so, and run it by hand per row.
4. **P4** (`hostile_lengths.rs:289` `formats()`): derive the format list from one registry the readers themselves use,
   so a new framed file cannot be missed. Give every child process a timeout.
5. **Control:** add a dummy fourteenth format to the registry without a hostile-length case, and P4 fails, naming it.

## 4. The `require_progress` scan (U4)

1. **A source scan, in the P3 scanner's shape** (`test_gates/allocation_bound_scan.rs`): every framed reader's
   advance point goes through `require_progress` (`foundation/frame_resync.rs:282-289`).
   - It has an allowlist with witnesses, self-tests, and a minimum site count. There are 41 sites across 11 readers
     today.
2. **What counts as an advance point,** stated precisely: for example, every assignment to a decode loop's offset
   after a resync. **The definition is what the review checks first.**
3. **Control:** remove `require_progress` from one site, and the scan names that site.

## 5. The object containers' label (U5)

1. **A full-length frame whose checksum fails is not an "interrupted append".** It is a damaged record (RFC 164 §9's
   principle, applied to text; `rfc164-round-1-review-v2.md:70-74`).
2. **Keep the meaning, change only the words:**
   - `verify/objects.rs:369-380` tells a torn tail (fewer bytes than claimed) from a complete damaged record (all bytes
     present, checksum fails);
   - `output/verification.rs:479,523,535` prints them differently;
   - connectivity still decides what matters, exactly as today.
3. **Test both shapes,** with the real lines quoted. CHANGELOG `### Output changes`.

## 6. Units, gates, report

| unit | what | budget (stop at ×2) |
|---|---|---:|
| U1 | §1: the gates script, R1 inside it, the documents | 60 min |
| U2 | §2: the shared release-binary check | 30 min |
| U3 | §3: P1, P2, the two command rows, P4 | 120 min |
| U4 | §4: the `require_progress` scan | 45 min |
| U5 | §5: the label | 30 min |

**Before proposing:** run `scripts/gates.sh` itself on the final commit, and paste its summary. That is also the first
real use of the script.

**Report:** `.git-exclude/review-request/step5-round-1-report-v1.md`, with each unit's real start and end.

## Addendum 1 — 2026-10-05: two fixes (review `step5-round-1-review-v1`)

Report v1: **not accepted yet.** U2, P1, P2, P4, U4 and U5 are accepted. **This is a fix round:** fixes only.

1. **F1, the gates script on this checkout:**
   - `scripts/gates.py` keeps an already-set, writable `TMPDIR`. Otherwise it uses the system temp directory if
     writable, and the repository-local directory only as the last resort. It prints which one it chose;
   - amend rule 9's wording to match;
   - `rfc147_declaration_resolution::every_destination_kind_resolves_as_commit_then_acts` creates its socket in a
     short directory of its own, independent of the checkout path, refusing clearly if even that is too long;
   - **evidence:** the script's own summary at 14 of 14, run from this checkout.
2. **F2, the ref log decoded at least three times inside `verify`:**
   - decode it once per `verify` and share the result, as RFC 167 D5 did for the WAL. Say what each former caller
     still checks;
   - **account for the measured 27×:** three decodes explain about 3×, so find and name the rest;
   - restore the row's real name and the 10× bound (`SCAN_BUDGET_MULTIPLE + 2`), and keep its control;
   - measure honest `verify` on a repository with many refs, before and after.
3. **The ×2 rule applies:** at ×2 of a unit's budget, file a question and wait.

| unit | what | budget (stop at ×2) |
|---|---|---:|
| A1 | F1 | 30 min |
| A2 | F2 | 90 min |

**Report:** `.git-exclude/review-request/step5-round-1-report-v2.md`.

**ACCEPTED and CLOSED 2026-10-06** (reviews `step5-round-1-review-v1`, `-v2`). Commits `7bca7432` … `16d47def`, then Addendum 1 `96c8b347`, `6ef46283`, `8ccd8c7f`. Round 2: `test-and-process-debt-round-2-handoff-v1.md`.
