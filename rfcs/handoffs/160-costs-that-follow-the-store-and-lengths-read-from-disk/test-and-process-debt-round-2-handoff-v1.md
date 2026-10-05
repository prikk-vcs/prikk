# 0.49.0 step 5, round 2 — a first fuzz target, the borrowed-tail label, existence checks, Linux-only tests

**Live 2026-10-06, and it is next.** Round 1 is closed: reviews `.git-exclude/reviewed/step5-round-1-review-v1.md`
and `-v2.md`.

**The rules carried, and enforced this time:**
- **At ×2 of a unit's budget, stop, file a question in `.git-exclude/review-request/`, and wait.** Round 1 ran past
  ×2 in every unit and never asked. A finding that changes scope is a question, not more work.
- **Every guard has a control that can fail,** with its result stated.
- **Read the code before writing a claim about it,** and cite the line. Numbers in this handoff are claims too: if
  one does not match the source, say so (round 1 found my 41/11 was wrong).
- Measurements: a release build with its sha256, on `/home`, in R1 scopes. **Run the gates with `scripts/gates.py`,**
  and paste its summary.

## 1. A first fuzzing target (U1)

1. **Proptest, not cargo-fuzz** (DC-41: libFuzzer needs nightly, and the gate is stable 1.85.0). The corpus policy is
   DC-41's: minimized `proptest-regressions` files only.
2. **The target:** every framed decoder in round 1's `FRAMED_FORMAT_CASES` registry, fed mutations of a valid file:
   byte flips, truncations, insertions, length-field rewrites, and wholly random bytes.
3. **The properties, each asserted:**
   - no panic, and termination;
   - bytes hashed at most 9× the input plus a stated slack (RFC 167's budget);
   - **a frame whose checksum verifies over bytes that are all present is never reported as a tail** (RFC 164 §9);
   - decoding the same bytes twice gives the same result.
4. **Cost:** the default suite gains at most 30 seconds. A longer run is an `#[ignore]`d measurement, with its case
   count stated.
5. **Control:** make one decoder call a complete record a tail, and the fuzz target finds it, with the shrunk case
   shown.

## 2. The borrowed-tail label, U5's remaining case (U2)

1. **The case:** a full-length frame failing its checksum, with later bytes after it. Today it stays an "interrupted
   append" (`foundation/container.rs`, the comment at round 1's fix site).
2. **The rule:**
   - if a sound frame starts inside the claimed range, the frame was torn and overrun by a later write, so it is an
     interrupted append;
   - otherwise it is a damaged record;
   - **use RFC 167's budgeted scan** (`sound_frame_after_partial_budgeted` and `ScanBudget`) inside the claimed range,
     never a new unbudgeted one. Exhaustion is damage (C3).
3. **Tests:**
   - a torn frame overrun by a later commit is still an interrupted append (`interrupted_append.rs` unchanged);
   - an earlier object with bit rot, followed by later commits, is damage;
   - a hostile claimed range is linear. Extend the round-1 command row if it reaches this path.

## 3. Existence checks for RefState, Tag and Attestation references (U3): measure first

1. **The fields** (`verify/reachability.rs:15-26`):
   - `RefState.target_object_id`, `previous_ref_state_id` and `required_attestation_ids`;
   - `Tag.target_block_id`;
   - `Attestation.target_block_id`. That needs an `AttestationPayload` decoder, which `prikk-object` does not have.
2. **These add new `verify` failures, so measure before anything fails:**
   - implement the checks in report-only mode first;
   - run them over the test suite's fixtures, the corpus in `tools/corpus`, `matrix.py`, repositories built by `sync`
     and `bundle import`, and a repository upgraded from format 6;
   - **from source, answer whether sync or import can legitimately leave a referenced object absent** (for example a
     received ref whose previous state was never transferred).
3. **If any honest repository would newly fail `verify`, stop and file a question.** That is a change in meaning, and
   it goes to the owner before it lands.
4. **If none would:** the checks become failures, with RFC 164 Rule E's classification (an object nothing references
   with a missing reference is a remnant). Each field gets a test with a control.

## 4. D4: Linux-only tests that can run on Windows and macOS (U4)

1. **The inventory:** every `cfg(all(test, target_os = "linux"))` gate (about 42; count them yourself). For each,
   from source:
   - **Linux by necessity:** failpoints tied to Linux syscalls, signals, `/proc`, `RLIMIT`, cgroups;
   - **portable:** say why.
2. **Port the portable ones,** in the order that covers the most at risk first: the WAL, refs recovery, locks, bundle,
   trust, `format_upgrade`.
3. **CI time:** the Windows mutation job's timeout is 65 minutes. **Measure its duration before and after** from the CI
   logs of a pushed run. If it would pass 50 minutes, stop and ask.
4. **The documents:** `platform-support.md` states what is still Linux-only, by name.

## 5. Units, report

| unit | what | budget (stop at ×2) |
|---|---|---:|
| U1 | §1: the fuzz target, its properties, its control | 90 min |
| U2 | §2: the borrowed-tail rule, budgeted | 60 min |
| U3 | §3: existence checks, measured first; stop if an honest repository fails | 90 min |
| U4 | §4: the inventory, the ports, CI time | 120 min |

**Report:** `.git-exclude/review-request/step5-round-2-report-v1.md`, with each unit's real start and end, and the
gates script's summary on the final commit.

## Addendum 1 — 2026-10-06: CI is red, and three corrections (review `step5-round-2-review-v1`)

**Outcome: Corrections Required.** U1, U2 and U3's attestation check are accepted as a checkpoint. Read review v1 in
full; its "Corrections" section is the work list.

0. **URGENT, first and alone:** fix the path assertions in `complete_damaged_record_is_not_interrupted_append.rs`,
   which fail on macOS and Windows (CI run `37382076762`). Compare paths by identity, sweep the step 5 test files for the
   same pattern, and commit it on its own. The architect gates it and pushes it with U1–U3.
1. **The received-tip gap:** import refuses a chain-incomplete bundle (a missing previous state, or a missing required
   attestation), and `verify` checks the received tip's previous state. Tests with controls, and CHANGELOG entries.
2. **The budget charge audit:** every `ScanBudget` reader charges each candidate's header plus its body. A
   zero-length-candidate test per reader, with a control.
3. **U4 continues:** classification, then the ports. The architect measures CI time from the push.

**Non-change scope:** no product behaviour other than item 1's import refusal and `verify` check. **Prohibited:**
weakening an assertion, or `cfg`-ing a test away from a platform without naming the Linux-only mechanism it needs.

| unit | what | budget (stop at ×2) |
|---|---|---:|
| A0 | item 0 | 20 min |
| A1 | item 1 | 60 min |
| A2 | item 2 | 60 min |
| U4 | item 3 (as in §4) | 120 min |

**Report:** `.git-exclude/review-request/step5-round-2-report-v2.md`, with `scripts/gates.py`'s summary on the final
commit.

**Round 2 closed 2026-10-06 for U1–U3 and Addendum 1** (reviews `step5-round-2-review-v1`, `-v2`). Commits `86a40b4f` … `c2e43920`, then `917792f6`, `d3b72787`, `10155238`. **U4 moved to round 3:** `test-and-process-debt-round-3-handoff-v1.md`.
