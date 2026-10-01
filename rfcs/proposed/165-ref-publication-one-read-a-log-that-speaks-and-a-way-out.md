# RFC 165 — Ref publication: one read per publication, a ref log that speaks, and a way out of an interrupted publication

**Status.** **PROPOSED 2026-10-01 by the architect** (0.49.0 step 2, in the owner-approved schedule: *"ref publication: F1
with N3 and M8 (design round)"*).
- **This RFC sets the questions; it does not yet choose the mechanisms.** A design round answers §4 from source and
  measurement, with prototypes and no product code: `rfcs/handoffs/163-a-write-never-buries-a-crash-state/
  ref-publication-design-round-handoff-v1.md`.
- **Then the architect rules on each option, this RFC is rewritten into a design, and the owner reads it** before any
  implementation handoff.

**Author-review independence.** The architect wrote RFC 162 rule 3 and RFC 164 §9, and both needed corrections that the
architect's own probes found late (a complete damaged record read as a tail; then a flipped header byte). This RFC
touches the same class of question for the ref log, so:
- every classification in §4 is answered from source by the dev team and checked by the architect's own crash probes;
- the 0.49.0 candidate goes to the external architect, whose matrix covers the ref log.

## 1. What is left, after RFC 164

1. **F1 — a publication reads the whole ref log three times.**
   - `replay_ref_subsequence` (`refs/container.rs:468`) replays the whole log, and is called by:
     - the append (`container.rs:443`);
     - the store's read (`refs.rs:411`);
     - the verify scan (`refs/verify/scan.rs:249`).
   - Measured by RFC 160's guard P1: 21.9 KB read at 4 generations, 274 KB at 64, so about 4.3 KB per generation for
     `seal` and `commit`.
   - The owner ruled F1 into 0.49.0 (RFC 160).
2. **M8 — a commit's cost grows with refs, squared.**
   - A commit touches no ref, yet every write path calls `ensure_no_incomplete_publication` (`refs.rs:178`). That runs
     a full `verify_refs`, whose scan replays the log once per ref (`scan.rs:249`): refs × log size.
   - External review 014 measured commit time against ref count: 1.4 ms at 1 ref, 7.2 ms at 100, 21.4 ms at 200,
     72.1 ms at 400; seconds at 4,000 by extrapolation. `status` stays flat.
3. **N3 — an interrupted publication has no way out.** External review 015, extended to `merge` in step 0:
   - **The crash:** inside `branch create` or `tag create`, it leaves the pointer durable and the ref log's last record
     torn.
   - **What the user sees:** `verify` exits 1 with `PRIKK-VERIFY-REF-DIVERGENCE`, `doctor` says "manual recovery", and
     a retry answers "already exists". DC-38's retry exists for `seal` only.
   - **It gets worse:** a `seal` of an unrelated ref exits 0 and appends behind the torn record. From then on `verify`
     reports a damaged ref-log record and `commit` is refused.
   - **Through `merge`:** re-running the merge is refused, and so is a `seal` retry. Measured as 10 of 300 kills on
     0.48.0, 16 of 300 on step 0's build (`merge_kill_probe.py`).
4. **M4 — the ref log is silent over bytes that are not records.** With 100 zero bytes appended, `verify` and `doctor`
   exit 0, and a `seal` appends behind them. RFC 164's Rules A–C left the ref log out, by ruling, for this round.
5. **A complete damaged pointer-index record has no way out except a copy** (RFC 164 §9.1, the accepted cost).
   **Rebuilding the pointer index from the ref log** was named as the way out for this round.
6. **Text:**
   - `troubleshooting.md`'s divergence entry quotes a detail line, hedged as "something like", that the binary does not
     print;
   - the "(block)" placement in the missing-reference forms;
   - both to be quoted from the real binary once the state is fixed.

## 2. Constraints every option must meet

- **C1 — meaning (I6).** A completion finishes only what a durable, signed RefState already says. A withdrawal removes
  only what never became authoritative. Neither moves a ref to a state no authorized key signed, and neither reads trust
  or authority from damaged bytes.
- **C2 — nothing silent, and §9.2 for the ref log.** Every byte of the ref log is a sound record, a reported tail, or
  reported damage (RFC 160 §7). Completeness is decided by the checksum, not the header.
- **C3 — Rule D.** Every publication checks the ref log and the pointer index before its first write.
  - **`seal` refuses while another ref's publication is incomplete**, as `commit` already does (RFC 163 §5).
  - A refusal writes nothing.
- **C4 — no format change in 0.49.0.** If a question cannot be answered well without one, say so. The answer then
  becomes format-8 input, beside the per-file witness (RFC 164 §9.1).
- **C5 — cost:**
  - a publication reads the ref log at most once;
  - a command that touches no ref (`commit`) does not read it in proportion to refs × log size;
  - each bound is shown by measurement, not argument.

## 3. Starting positions (to be tested, not decided)

- **F1 and M8 may share one fix.** A publication-state check that does not replay the whole log per ref: for example,
  compare each ref's newest pointer-index entry with the ref log's newest record for that ref, in one pass.
- **For N3, complete when the signed state is durable; withdraw only when nothing authoritative was written.** Which
  crash prefixes allow which is §4 Q1–Q2's answer.
- **M4: the ref log under Rules A–C and §9.2, but ordered against N3.** A torn last ref-log record while the pointer
  leads is an interrupted publication, not a tail. Truncating it would leave the pointer ahead of any record, so it must
  be completed. The two have to be classified together.
- **The pointer index may be derivable from the ref log.** If every field is, the pointer index becomes rebuildable,
  and a complete damaged record gains a way out.

## 4. Questions for the design round

1. **Write order.** From source, the exact write sequence of every publication: `seal`, `branch create`, `branch close`,
   `tag create`, `merge`, `sync seal`, `sync adopt-tag`. For each prefix of those writes, the state a crash leaves and
   how `classify_state` sees it. One table.
2. **Way out, per crash state:** complete, withdraw, or refuse, and why, against C1. Which data each relies on, and
   whether it is durable and signed at that point.
3. **The ref log's tail:** what a tail is under §9.2; how it combines with a leading pointer (Q2); and which repair is
   safe for each combination.
4. **F1:**
   - reads per publication today, instrumented, at generations 4, 64 and 1,024;
   - options that read the log once;
   - each measured.
5. **M8:**
   - what the precondition protects;
   - options that protect the same thing without refs × log reads;
   - commit time at 1, 100, 400 and 4,000 refs for each.
6. **The pointer index from the ref log:** is every field derivable? What does a rebuild need: signature checks, the
   trust in force at the time? Is it safe for a complete damaged record?
7. **Text:** the real divergence lines, and the real "(block)" placement, from the binary.

## 5. Out of scope

- A format change (C4).
- Network transport.
- A ref authorization model. Trust stays object trust (RFC 116).
- The object containers' labels, and existence checks for RefState, Tag and Attestation references (step 5).
