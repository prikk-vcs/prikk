# RFC 165 round 1 — one read per publication, a one-pass precondition, every publication refusing first (R1, R2, R3)

**Live 2026-10-01, and it is next.** RFC 165 is ACCEPTED by the owner: *"Accepted. However, we had better be careful
around such design."* **Read the whole RFC first,** `rfcs/accepted/165-ref-publication-one-read-a-log-that-speaks-and-a-
way-out.md`, **including K1–K7 in its Status.** They are how "be careful" binds this work.
- This round is **R1, R2 and R3**, which change cost and ordering, not meaning. **R4–R6 are round 2. Do not start them.**
- The prototypes in `scratch-165/proto` are the starting point. Review v2's three corrections apply: keep
  `has_incomplete_active_cleanup`; the post-write check is ranged, not trusted; no bare failpoint ordinals.

**The rules carried from RFC 164's rounds:**
- **A cut is a question to the architect before delivery.** A report that marks a required item not done without a prior
  question is returned unread past that point.
- **Sweep the region; every site gets its own control; every timing names its binary.**
- **At ×2 of a unit's budget, stop and ask.**

## 1. R1 — one read per publication

1. **Thread `classify_state`'s replay through the append** (`append_ref_container_record`). The pointer-index and
   ref-log locks stay held across the whole critical section.
2. **Replace `ensure_agreement`'s whole read with a ranged one.** Read back only the record just appended, at the offset
   the append wrote it, and compare it byte for byte with what was written. The damage and tail checks rest on the
   pre-write replay, which is valid because the locks were never released. Say so in the module doc.
3. **Tests:**
   - **read count:** one whole read of the ref log per publication at generations 4, 64 and 1,024, through
     `whole_read_guard`, for `seal` and `branch create` at least;
   - **correctness:** identical durable state to today's path over several generations, and a `Complete`-state retry
     stays idempotent;
   - **the stale-replay control**, made permanent: a second writer between the read and the write is caught;
   - **the ranged check's own control:** a test in which the bytes that land differ from the bytes intended, so the
     check refuses. With the comparison removed, the test goes red. Design it so it can fail; say how.

## 2. R2 — the write-path precondition in one pass

1. **`ensure_no_incomplete_publication` becomes:**
   - one pass over the pointer index and one over the ref log;
   - each ref's newest pointer compared with its newest log record;
   - a refusal on any `Failed` record outcome;
   - **and `has_incomplete_active_cleanup`, unchanged.**
   - `verify_refs` is no longer called from it. `verify` and `doctor` keep the full check.
2. **Tests:**
   - **equivalence with the old function, kept as a test oracle:**
     - at **every AppendWrite and RequiredFileSync ordinal of every publication** (`seal`, `branch create`,
       `branch close`, `tag create`, `merge`, `sync seal`);
     - with 0 and with 3 settled refs beside it;
     - asserting that at least one row refused, so the sweep cannot pass by exercising nothing;
     - (the architect's 32-row `arch_q5_equivalence_at_every_ordinal` is the shape);
   - a damaged record (refuses), and a pending active cleanup (refuses);
   - **control, per check:** remove the damaged-record check, then the cleanup check, one at a time. Each one's rows go
     red.
3. **Release timing of `commit`** at 1, 100, 400 and 4,000 refs: three samples each with the spread, before and after,
   on the shipped binary (sha256).

## 3. R3 — every publication refuses while another ref's publication is incomplete

1. `seal`, `branch create`, `branch close`, `tag create`, `sync adopt-tag`, `merge` and `sync seal` call R2's check
   **before their first write**.
   - **Scope:** an incomplete publication **on any ref other than the one this command publishes.** The command's own
     ref keeps its existing handling, so `seal`'s DC-38 retry of its own interrupted publication must still complete.
     Test that explicitly.
   - **A refusal writes nothing.** Compare the `.prikk/` tree before and after.
2. **Tests:**
   - for each publication: another ref's interrupted publication, produced by a sweep with its state asserted first,
     then the publication. It refuses, and the tree is identical;
   - `seal`'s own retry still completes;
   - **control, per publication:** remove its call. Its row goes red, or shows a write.
3. **Known interim state:** `sync seal`'s self-lockout and the stuck `branch`/`tag`/`merge` states keep no way out until
   round 2 (R4). Say so in the report; do not work around it.

## 4. Text

- `current-state.md`: F1 and M8 leave the known limitations, measured. N3's bullet adds that every publication now
  refuses behind another ref's interrupted one.
- CHANGELOG `## Unreleased`:
  - `### Changed`: R1, R2;
  - `### Output changes`: the new refusals of R3, with their exact text.

## 5. Gates, units, report

**Before proposing:**
- the 14 gates on the final commit, in R1's scope;
- on a release build of it:
  - the architect's `rfc164_all_files_reader_probe.sh`, `rfc164_rule_d_writers_probe.sh` and
    `rfc164_rule_d_sync_rollback_probe.sh`;
  - the seven RFC 163 probes;
  - `matrix.py` v4 compared with `matrix-5e50a661.txt`, every changed cell explained with its own replay.

| unit | what | budget (stop at ×2) |
|---|---|---:|
| U1 | R1 and its tests and controls | 60 min |
| U2 | R2, the ordinal-sweep equivalence, the controls | 60 min |
| U3 | R3, per-publication tests and controls | 60 min |
| U4 | release timings, probes, `matrix.py`, text | 45 min |

**Report:** `.git-exclude/review-request/rfc165-round-1-report-v1.md`, with time spent against each unit.

## Addendum 1 — 2026-10-01: a wedge, the cross-target gates, three unheld call sites (review `rfc165-round-1-review-v1`)

Report `rfc165-round-1-report-v1.md`, commit `1785a849`. **Not accepted. Fixes only.** Read review v1, §1 to §5.

1. **The wedge (review §1):**
   - **The defect:** a ref-log tail with no pointer lead (zeros, garbage, a lead-free torn prefix) now blocks every
     `commit`, while `verify` exits 0, and the refusal names a `seal` retry that cannot apply.
   - **R2 separates the two states:** a pointer lead is an incomplete publication; a lead-free tail is not one.
     `commit`, and every writer that does not append to the ref log, proceeds over it.
   - **Publications refuse over it, writing nothing** (Rule D). Their message names the tail, its offset and its byte
     count, and says the repair arrives with R5. Not "incomplete ref publication", and not "seal retry".
   - **Tests:**
     - 30, 100 and 4,096 zero bytes, 100 random bytes, and a lead-free torn prefix: `commit` succeeds, and each
       publication refuses with an identical tree;
     - **control:** today's behaviour turns these rows red.
   - **`matrix.py`:** the four `log-a.container` cells return to `way-out: yes`.
2. **The cross-target gates (review §2):**
   - fix the dead code on Windows and macOS: `ensure_no_incomplete_publication_via_verify_refs_for_test` and
     `RefVerification::has_item_failure`;
   - **run all 14 gates every round.** The cross-target rule counts against the last release tag, not the round's diff.
3. **The unheld call sites (review §3):** `seal`, `branch create` and `branch close` each get an entry-point test with
   another ref's interrupted publication present. **Control, per site:** remove its call; its row goes red.
4. **Timings (review §4):** the raw samples and the binary's sha256, measured on an idle machine.
5. **Process (review §5):**
   - each unit's start and end time goes in the report;
   - a unit reaching ×2 files a question in `.git-exclude/review-request/` at that moment.

**Before proposing:**
- the 14 gates;
- the architect's probes, in `/home/nabbisen/.pgtmp/arch-seal/`: `rfc164_all_files_reader_probe.sh`,
  `rfc164_rule_d_writers_probe.sh`, `rfc164_rule_d_sync_rollback_probe.sh`, and the seven `rfc163_*.sh`;
- `matrix.py`.

**Report:** `.git-exclude/review-request/rfc165-round-1-report-v2.md`.

**ACCEPTED and CLOSED 2026-10-02** (`1785a849`, `522f8c0f`; reviews `rfc165-round-1-review-v1`, `-v2`; pushed
`522f8c0f`). Next is round 2: `round-2-handoff-v1.md`.
