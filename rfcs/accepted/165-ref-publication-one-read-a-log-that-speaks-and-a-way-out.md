# RFC 165 — Ref publication: one read per publication, a ref log that speaks, and a way out of an interrupted publication

**Status.** **ACCEPTED by the owner 2026-10-01**, after reading. The owner said: *"Accepted. However, we had better be
careful around such design."* **§6 decision 2, who may complete:** *"Your recommendation is accepted."*: any adopted
maintainer key.
- *History:* proposed 2026-10-01 as questions (0.49.0 step 2, owner-approved schedule). It was answered by a design round
  (reviews `rfc165-design-round-review-v1` and `-v2`) and rewritten as this design the same day.

**The architect's reading, stated so it can be corrected:**
- R1 to R6 are accepted as written, and decision 2 is settled: any adopted maintainer key may complete. The completer's
  key is named in the record it signs.
- **"Be careful" is taken as binding on the implementation, not as a remark.** It means:
  - **K1, see before writing.** `prikk ref complete` and `doctor --rebuild-pointer-index` each get `--plan-only`, which
    prints exactly what would be written and changes nothing. A real run prints the same plan first: the ref, the
    leading RefState, its signer, its target and the chain for a completion; each ref's resulting state and every
    dropped lead for a rebuild.
  - **K2, fail closed on any doubt.** Any condition that cannot be evaluated refuses and writes nothing. That covers
    damaged trust state, an unreadable RefState, a missing target, and a log tail other than the one completion removes.
  - **K3, every condition can fail in a test.**
    - Each of R4's conditions (a)–(d) gets a negative test, and a control that removes the condition and turns the
      test red.
    - Adversarial constructions: a lead signed by a non-adopted key, one chained to the wrong previous state, one at the
      wrong sequence, one with a missing target or one of the wrong kind, and a WAL mismatch.
  - **K4, a recovery verb is a writer.** `ref complete` and the rebuild are each raced against ordinary publishers and
    commits under failpoints. A crash inside either one leaves a state the same rules classify.
  - **K5, nothing implicit.** Neither verb runs automatically, from another command or from `doctor`'s diagnosis.
    `doctor` only names them.
  - **K6, the ways out last, and reviewed from outside.** Implementation round 1 is R1–R3, which change cost and
    ordering, not meaning. Round 2 is R4–R6. The external review of the 0.49.0 candidate is asked to examine R4 and R5
    specifically.
  - **K7, a second addendum on round 2 means a design re-read,** per RFC 152 §7. A finding that would change R4 or R5
    comes back to the owner.
- **Live:** `rfcs/handoffs/165-ref-publication-one-read-a-log-that-speaks-and-a-way-out/round-1-handoff-v1.md` (R1–R3).

**Author-review independence.** The architect wrote RFC 162 rule 3 and RFC 164 §9, and both needed late corrections.
This RFC touches the same class of question for the ref log, so:
- every rule below rests on a reproduced crash state or a measured prototype, not on reasoning alone. One prototype test
  that did not reproduce was caught and re-checked by the architect (review v2 §1);
- the 0.49.0 candidate goes to the external architect.

## 1. What is wrong today (measured in the design round)

1. **F1 — every publication reads the whole ref log three times.**
   - The reads: `classify_state`, the append's own check, and `ensure_agreement`.
   - The count is 3 at generations 4, 64 and 1,024; the bytes read grow linearly.
2. **M8 — every write checks every ref's whole history.**
   - The write-path precondition (`ensure_no_incomplete_publication`, `refs.rs:178`) runs a full `verify_refs`, which
     replays the log once per ref.
   - Release timing: 62 µs at 1 ref, 6.3 ms at 100, 74 ms at 400, **6.8 s at 4,000**. A commit touches no ref.
3. **N3 — a publication that crashes between its pointer write and its log write is stuck.**
   - **Every publication** (`branch create`, `branch close`, `tag create`, `sync adopt-tag`, `merge`, `sync seal`) is
     then reported as `PRIKK-VERIFY-REF-DIVERGENCE`, "not proved by matching retained active state and trust".
   - **Only `seal` has a way out:** `require_retained_evidence` (`verify/ref_publication.rs:12-41`) accepts only the WAL
     as evidence, and only `seal` consumes the WAL.
   - **The retries:**
     - `branch create` and `tag create` answer "already exists";
     - `merge` answers "not confluent";
     - **`sync seal` refuses its own interrupted publication**, because its first line is the precondition that blocks
       it.
   - **And it buries itself:** a `seal` of another ref does not check, and appends behind the stuck one.
4. **M4 — the ref log is silent over bytes that are not records,** and a tail without a pointer lead is an internal
   error, not a reported tail.
5. **A complete damaged pointer-index record has no way out except a copy** (RFC 164 §9.1, accepted as a cost until
   this RFC).
6. **Text:** the divergence forms in `troubleshooting.md` do not match the binary, and "(block)" appears in no text the
   binary prints.

## 2. Constraints (unchanged from the questions round)

- **C1, meaning (I6):** a completion finishes only what a durable, signed RefState already says. Nothing moves a ref to
  a state no authorized key signed, or reads trust from damaged bytes.
- **C2, nothing silent:** §9.2 applies to the ref log.
- **C3, Rule D:** every publication checks before its first write.
- **C4:** no format change.
- **C5:** a publication reads the log at most once, and a commit does not scale with refs × log.

## 3. The rules

**R1 — one read per publication (F1).**
- `classify_state`'s replay is threaded through the append. The pointer-index and ref-log locks are held across the
  whole critical section, as they are today.
- The post-write check **reads back only the record just appended** and compares it with what was written. That keeps
  the check's purpose (the bytes landed as intended) without a whole-log read.
- Measured on the prototype: 1 read at every depth, about 1.8× faster at depth 1,024.
- **Permanent tests:**
  - the read count is 1 at depths 4, 64 and 1,024;
  - the stale-replay control: a second writer between the read and the write must be caught, and the test can fail.

**R2 — the write-path precondition answers its own question in one pass (M8).**
- One pass over the pointer index and one over the ref log, comparing each ref's newest pointer with its newest log
  record.
- It refuses on any damaged record, and **keeps `has_incomplete_active_cleanup`**: a settled publication whose queue has
  not drained.
- `verify_refs`' fuller checks stay where they belong, in `verify` and `doctor`.
- Measured: **4.4 ms at 4,000 refs, against 6.8 s.** It agrees with the original at every crash ordinal of a branch
  create (the architect's 32-row control).

**R3 — every publication refuses while another's is incomplete (C3).** `seal`, `branch create`, `branch close`,
`tag create`, `sync adopt-tag`, `merge` and `sync seal` call R2's check before their first write. R2 makes that cheap.
A `seal` no longer appends behind another ref's interrupted publication.

**R4 — completing an interrupted publication (N3).**
- **The completion rule.** A pointer lead is completable when all of these hold:
  - (a) the leading RefState verifies under the current trust policy, signed by an adopted maintainer key;
  - (b) it chains: its ref name is the ref, its previous state is the log's tip, and its sequence is the next one;
  - (c) its target exists, with the kind the ref requires;
  - (d) for a publication that consumed the WAL (`seal`, `sync seal`), today's WAL evidence still matches.
  - In the design round, every crash-produced lead passed (a)–(c). A lead that fails them needs corruption or a writer
    outside prikk.
- **Reporting.**
  - A completable lead is reported as `PRIKK-VERIFY-REF-POINTER-LEADS-LOG`, and `doctor` names the verb below.
  - A lead that fails the rule stays `PRIKK-VERIFY-REF-DIVERGENCE` (damage), and its way out is R5's rebuild.
  - `require_retained_evidence` becomes condition (d), for WAL-consuming publications only.
- **The verb: `prikk ref complete <ref>`.**
  - It is signer-backed: it signs the ref-log record that endorses the leading RefState, removing a partial log tail
    first, as `publish_locked` already does.
  - It applies the rule and refuses, writing nothing, if any condition fails.
  - Every refusal over an incomplete publication names it: the "already exists" retries, `merge`'s, and `sync seal`'s,
    whose retry then succeeds through its existing no-op path.
- **Withdrawing a completable lead is never offered.** It would roll back a transition an authorized key signed (I6).

**R5 — the ref log speaks, and the pointer index can be rebuilt (M4, RFC 164 §9.1).**
- **A torn last log record while the pointer leads** is an interrupted publication. It is completed by R4, not
  truncated.
- **A tail with no pointer lead** (M4's zeros, or garbage) is a tail.
  - Rule B reports it. It is an internal error today.
  - `--repair-tails` covers the ref log and saves what it removes.
- **A complete damaged log record** is damage (§9.2), and fails closed. The way out stays a copy (§4).
- **`prikk doctor --rebuild-pointer-index`:** structural, never trust-filtered (a revoked key does not move a ref), and
  no signing.
  - It writes the newest state of each ref into the pointer index's other slot, and switches the generation log, as
    compaction does. The switch is atomic, and the old slot stays until the next compaction.
  - **It refuses** if the ref log has damage or a tail (repair first), or **if any lead is completable (complete
    first)**, because a rebuild would drop an authorized transition.
  - **It drops a lead that fails R4's rule, and names it.** That is its purpose.
  - It serves a complete damaged pointer-index record, and a lead that is not completable.
  - Proved byte-identical, in its uncompacted form, against live pointer indexes at two depths.

**R6 — text.**
- Quote the binary's real lines: the three divergence texts, and the pointer-leads texts with and without a tail.
- Correct the missing-reference forms to `object <owner> references missing <role> <id>`.

## 4. What it costs, and what remains

- **Two new recovery verbs,** `ref complete` and `doctor --rebuild-pointer-index`.
  - They are ways out, not features: within the 0.49.0 theme.
  - The new refusals and report lines are output changes.
- **Remains:**
  - **a complete damaged *last* ref-log record** still needs a copy. A signer could re-endorse the pointer's state, but
    that is a design of its own, not taken here;
  - **corruption across several fields of one record**: the per-file witness, format 8.
- **Cost:** R2's check runs on every write. It was measured at 12 µs at 1 ref and 4.4 ms at 4,000.

## 5. Implementation, in two rounds

1. **Cost and refusal first:** R1, R2, R3.
2. **The ways out:** R4, R5, R6.

- **Tests:** every ordinal swept, or the reached state asserted first; never a bare failpoint ordinal (review v2 §2).
- **Units:** smaller than the design round's, each with a budget.
- **External review:** of the 0.49.0 candidate (§ Status).

## 6. Decisions for the owner (both decided 2026-10-01: accepted; any adopted maintainer key)

1. **This design, as written.**
2. **Who may complete another's interrupted publication:**
   - **Recommended: any adopted maintainer key.**
     - The state being endorsed is already signed by an adopted maintainer; the completer adds only the log record,
       which names its own key.
     - It matches prikk's object-trust model, and it does not strand a repository whose original signer is away.
   - **The alternative: only the key that signed the leading RefState.** It is narrower, but a repository whose original
     signer's key is unavailable stays stuck until a rebuild, and the rebuild drops the authorized transition.
