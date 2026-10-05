# RFC 167 — Resynchronisation is linear: a hostile tail cannot make a reader quadratic (M5)

**Status.** **PROPOSED 2026-10-05 by the architect** (0.49.0 step 4, in the owner-approved schedule: *"M5's
structural fix without a format change"*).
- **This RFC sets the questions; it does not choose the mechanism.** A design round answers §5 from source and
  measurement, with prototypes and no product code:
  `rfcs/handoffs/160-costs-that-follow-the-store-and-lengths-read-from-disk/resync-linear-design-round-handoff-v1.md`.
- Then the architect rules, this RFC is rewritten into a design, and the owner reads it before any implementation.

**Author-review independence.**
- **M5 came in through a design the architect accepted.** RFC 160 F3 has a reader fully parse every candidate frame,
  which hashes each candidate's body. The architect "read the loop for termination and not for cost" (assessment of
  external review 014).
- So:
  - every cost claim here is measured by the dev team and re-measured by the architect;
  - **every design condition names the code it relies on, read before it is written** (the RFC 166 D5 lesson);
  - the external architect, who found M5, examines the fix at the 0.49.0 candidate.

**Carefulness.** K1–K7 (RFC 165) apply where this RFC changes a verdict. Any change in what a reader calls a tail and
what it calls damage is a change in meaning (I6).

## 1. What M5 is

1. **The finding** (external review 014; generator `reproduce/mkhostile.py`):
   - `verify` over a WAL whose torn tail is packed with frame headers, each claiming a body that runs to the end of
     the file;
   - **each doubling of the tail costs four times as long:** 0.24, 0.88, 3.49 and 13.96 s at 256 KiB, 512 KiB, 1 MiB
     and 2 MiB on `bb81b0fb`. 64 MiB extrapolates to hours;
   - **the shipped 0.48.0 asset:** 0.27, 1.00, 3.76 and 14.73 s (`receive/018…/results-shipped-0.48.0.txt`);
   - **no record names the filesystem** of any M5 measurement.
2. **The mechanism, read from source (2026-10-05):**
   - `sound_frame_after_partial` (`foundation/frame_resync.rs:50-64`) visits every occurrence of the magic after a
     partial frame and asks `sound_frame_at` whether a sound frame starts there;
   - for the WAL, that is `parse_frame_at` (`wal.rs:535-589`), which hashes the whole claimed body (`record_checksum`)
     whenever it fits;
   - **N candidates, each claiming a body to the end of the file, cost N × the file's length:** quadratic. The "length
     must fit" guard does not help, because the hostile bodies fit exactly.
3. **Two paths reach it** (RFC 160, `runaway-guards-review-v2`):
   - **shape A:** every header fits, then fails its checksum. This is RFC 102's invalid-frame resync loop, already
     quadratic on 0.47.0;
   - **shape B:** a partial frame followed by hostile headers. This is F3's partial-frame scan, new in 0.48.0.
4. **Ten readers share the scan** (`grep sound_frame_after_partial`): the WAL, the object containers, the object
   index, the generation logs, the pointer index, the ref log, the received index, the trust index, the author-key
   index, and `verify`'s object scan.
   - **Some read content from outside the repository:** object containers from bundle import and `sync accept`, and
     the received index.
   - **So M5 is also a denial-of-service shape** on input a user receives.
5. **What shipped instead:**
   - standing ceilings on bytes hashed (`test_gates/runaway_guards.rs:391-431`, ≤ 1.5× the measured figure at 32 and
     64 KiB), which stop a regression but leave the quadratic;
   - **no runtime cap on work.**
6. **The records disagree about the fix:**
   - `current-state.md` names a self-vouching header for 0.49.0;
   - the 0.48.0 CHANGELOG says "Fixed in 0.49.0";
   - **the owner's schedule says without a format change,** and a self-vouching header *is* a format change (RFC 162
     §3: format-8 input).

## 2. Facts that bound the design

- **A torn tail is a prefix of one frame** (RFC 160 F3). An honest crash leaves one partial frame and nothing sound
  after it.
- **Ambiguity resolves to damage** (`frame_resync.rs`). A false damage costs a refused repair; a false tail can lose
  data (RFC 164 §9, I6).
- **There is no header-only checksum:** each format's checksum covers header and body together. Deciding that a
  candidate is sound needs its body hashed. That is the structural cause, and only a format change removes it.
- **The WAL now has a witness** (RFC 166). With a valid witness that agrees with the sound prefix, any record after a
  partial frame was never acknowledged, so the WAL's verdict may not need the scan at all. **To be confirmed from
  source** (§5 Q3).
- **The WAL keeps RFC 162 rule 3:** its tail is everything after the last sound record when nothing sound follows.

## 3. Constraints

- **C1 — no format change** (the owner's schedule).
- **C2 — linear:** for every reader, the bytes hashed and the time are bounded by a constant times the input,
  measured on `/home` from 32 KiB to at least 64 MiB, shapes A and B. Replace the 1.5× ceilings with that bound.
- **C3 — never a false tail** (I6, RFC 164 §9). Wherever the work is cut short, the verdict leans to damage, never to
  tail.
- **C4 — honest data keeps its verdict.** No existing test changes its verdict, and neither does the RFC 133 corpus,
  `matrix.py`, or a repository whose committed content itself contains many frame magics (prikk's own test fixtures
  are such content).
- **C5 — one shared mechanism** in `frame_resync`, used by all ten readers. No per-reader copies.
- **C6 — a way out:** if a cut-short scan reports damage, the user can still get out, and the text says how.

## 4. Starting positions (to be tested, not decided)

- **A shared work budget per decode call** (the RFC 160 round's sketch):
  - counted in bytes hashed, relative to the input (for example 8× its length), not in candidates;
  - exhaustion returns a third outcome, "undetermined", which every reader handles as damage (C3).
- **Cheap rejections before hashing** cut the honest cost but not the adversarial one: version, plausible lengths and,
  for the WAL, a seq that follows the last sound record. An attacker can satisfy each of them, so the budget stays as
  the backstop.
- **The WAL's witness may settle the WAL** without the scan (§2). That would make the WAL, the file users meet most,
  exact, and leave the budget to the other nine.
- **The risk to test hardest is C4 against C3:** an honest crash tail whose torn record carries content full of
  magics. Under a budget, could that read as damage and block a repair the user needs? Measure it; do not argue it.

## 5. Questions for the design round

1. **Where the cost is, per reader:** shapes A and B against each of the ten readers, bytes hashed and time at 32 KiB,
   256 KiB, 2 MiB and 64 MiB, on `/home`, release build. Which readers can receive hostile input from outside the
   repository, and by which command?
2. **The budget:** prototype it in `frame_resync`.
   - The unit (bytes hashed or candidates), the factor, and why.
   - Shapes A and B are linear afterwards, for every reader.
   - **The "undetermined" outcome through every reader's arms:** list each call site, and show from source what each
     does with it.
3. **The WAL's witness:** from source, can a valid, agreeing witness decide the WAL's tail without the scan? Which
   cells of RFC 166 §5 change, if any? Prototype it if yes.
4. **Honest data (C4):**
   - the RFC 133 corpus, the existing suite, `matrix.py`, and a repository whose committed files contain thousands of
     frame magics: no verdict changes;
   - **an honest crash tail whose torn record is full of magics:** its verdict with and without the budget, and the
     way out if it reads as damage.
5. **The way out (C6):** what the user sees and can do when the budget makes a reader report damage, for each reader.
6. **The command-level guard:** a `verify` row, bytes hashed on hostile WAL input, that also catches a second decode
   (the RFC 162 regression the reader-level ceiling missed).
7. **The records:** what `current-state.md` and the CHANGELOG should say, once the mechanism is chosen.

## 6. Out of scope

- **A self-vouching header** (a header-only checksum), and any other format change: format-8 input.
- RFC 166's verdicts, except where Q3 shows that the witness settles the WAL's tail.
- Network transport.
