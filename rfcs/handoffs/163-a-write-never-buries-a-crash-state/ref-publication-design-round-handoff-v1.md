# Ref publication — RFC 165 design round, handoff v1

**Live 2026-10-01, and it is next.** 0.49.0 step 1 (RFC 164) is closed: review
`.git-exclude/reviewed/rfc164-round-2-review-v2.md`.
- **Why it is filed here:** it settles what RFC 163 §5 deferred ("a way to complete or withdraw an interrupted
  `branch create` or `tag create`, and `seal` refusing while another ref's publication is incomplete, are settled with
  F1 in 0.49.0"). RFC 165 is still proposed, and a proposed RFC carries no handoffs.
- **This is a design round: measure, prototype, report, then stop. No product code lands.**

**Read first:**
- `rfcs/proposed/165-ref-publication-one-read-a-log-that-speaks-and-a-way-out.md`, all of it;
- RFC 163 §5;
- RFC 164 §9 to §9.2 (what a tail is now);
- RFC 160's F1 entry.

**The rules carried from RFC 164's rounds:**
- **A cut is a question to the architect before delivery.**
- **Sweep the region:** every write ordinal of every publication, not one sample.
- **Every measurement names its binary**: release, with its sha256, unless the question is about debug.

## 1. What to answer

**RFC 165 §4, all seven questions.** Each is answered from source, and backed by a measurement where it can be measured.
1. **Write order (Q1):**
   - **one table:** publication × write ordinal → file written, durable state after it, and `classify_state`'s
     verdict;
   - **the crash states come from failpoints, not reading:** the existing
     `refs/tests/publication_recovery/failpoints` infrastructure, a failpoint at every write ordinal of every
     publication (`seal`, `branch create`, `branch close`, `tag create`, `merge`, `sync seal`, `sync adopt-tag`);
   - for each, record what `verify`, `doctor`, a retry, an unrelated `seal`, and a `commit` do.
2. **Way out (Q2):** per crash state, complete / withdraw / refuse, against RFC 165 C1.
   - Name the durable, signed data each completion relies on.
   - **List first any state where neither completing nor withdrawing meets C1.** The architect rules on those before
     anything else.
3. **The ref log's tail (Q3):** the decision table of (ref-log last record: sound / tail / complete-damaged) ×
   (pointer: agrees / leads / lags), with the safe repair for each cell. **Every cell is reproduced, not reasoned.**
4. **F1 (Q4):**
   - instrument the reads (bytes, and replays of `replay_ref_subsequence`) for each publication at generations 4, 64
     and 1,024;
   - prototype one option that reads the log once, and measure it.
5. **M8 (Q5):**
   - state from source what `ensure_no_incomplete_publication` protects;
   - prototype one option that protects the same thing without refs × log reads;
   - commit time at 1, 100, 400 and 4,000 refs, before and with. Three samples per point, and the spread.
6. **The pointer index from the ref log (Q6):**
   - field by field, derivable or not;
   - what a rebuild would check (signatures, the trust in force), and whether it is safe for a complete damaged record;
   - **a prototype rebuild, compared byte for byte with the real pointer index** on the RFC 133 corpus at two depths.
7. **Text (Q7):** the real divergence lines for every N3 state, and the real "(block)" placement, quoted from a release
   build.

## 2. Prototypes and measurement

- **Prototypes live in a scratch worktree with their own `CARGO_TARGET_DIR`.** Nothing lands on `main`.
- **Release builds for every timing.** Name the sha256.

| unit | what | budget (stop at ×2) |
|---|---|---:|
| U1 | Q1–Q3: failpoint sweep, the crash-state and tail tables | 90 min |
| U2 | Q4–Q5: instrumentation, the two prototypes, the timings | 90 min |
| U3 | Q6: the rebuild prototype and its byte comparison | 60 min |
| U4 | Q7: text, from the binary | 15 min |

**Nothing else runs while U2's timings do.**

## 3. Report

- Answers to Q1–Q7, with the tables, each option laid out, and **no option chosen**: the architect rules.
- **Before proposing:** the 14 gates on `main`'s tip are not needed (no product code). State that the primary tree is
  clean.
- **Report:** `.git-exclude/review-request/rfc165-design-round-report-v1.md`.

## Addendum 1 — 2026-10-01: finish the round (review `rfc165-design-round-review-v1`)

Report `rfc165-design-round-report-v1.md`. **Not accepted: the round is incomplete.** Read review v1, §1 to §3.
- **Still a design round:** measure, prototype, report, stop. No product code lands.
- **Ruled already** (review v1 §3):
  - a pointer-index rebuild is structural, never trust-filtered;
  - Q4's sketch and Q5's per-ref comparison are the directions to prototype;
  - Q7's text findings go into the RFC.

1. **Q1, by failpoints:**
   - every write ordinal of `seal`, `branch create`, `branch close`, `tag create`, `merge`, `sync seal` and
     `sync adopt-tag`, upstream writes included;
   - for each state: `verify`, `doctor`, a same-command retry, an unrelated ref's `seal`, and a `commit`.
2. **Q2:** test review v1 §2's proposed completion rule against every pointer-lead state from item 1:
   - (a) the RefState verifies under current trust as signed by an adopted maintainer key;
   - (b) it chains (ref name, previous, sequence);
   - (c) its target exists, with the right kind;
   - (d) the WAL evidence, for `seal` and `sync seal`.

   Report which states pass, **any that pass yet should not be completed**, and whether a failing lead can come from a
   crash or only from corruption. `require_retained_evidence` (`verify/ref_publication.rs:12-41`) is today's rule;
   start there.
3. **Q4:** implement the prototype in the scratch worktree.
   - Reads and bytes at generations 4, 64 and 1,024; release timing of one `seal` at 1,024, three samples.
   - A second-writer test, by failpoint, of the lock-held invariant, with a control that makes it fail.
   - Reconcile 417 B against RFC 160's 4.3 KB per generation.
4. **Q5:**
   - release `commit` timings at 1, 100, 400 and 4,000 refs, three samples each with the spread, before and with the
     per-ref prototype;
   - what a narrower precondition still protects.
5. **Q6:**
   - the structural rebuild prototype;
   - a byte-for-byte comparison with the live pointer index on the RFC 133 corpus at two depths;
   - a row with a revoked key, where the ref must not move.
6. **Q7:** the tail-variant text, live.
7. **Time spent against each unit's budget.** A cut is a question to the architect before delivery, answered within the
   round.

**Report:** `.git-exclude/review-request/rfc165-design-round-report-v2.md`.
