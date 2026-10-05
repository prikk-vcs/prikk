# RFC 167 — Resynchronisation is linear: a hostile tail cannot make a reader quadratic (M5)

**Status.** **ACCEPTED 2026-10-05 by the owner** (*"1. Accepted. 2. Your recommendation is accepted. 3. Your
recommendation is accepted."*). Proposed and rewritten as a design the same day by the architect (0.49.0 step 4).
- **The architect's reading:**
  - **D1–D6 are accepted as written,** with §6's guards (the 4× honest margin, one coherent message per case) binding;
  - **decision 2:** object containers and the ref container are inside the budget (option (c)). Their way out stays
    RFC 164's;
  - **decision 3:** D5, the doubled `verify` cost, is fixed in this RFC's implementation;
  - K1–K7 apply where a verdict changes. One implementation round (§8).
- **The design round:** handoff
  `rfcs/handoffs/160-costs-that-follow-the-store-and-lengths-read-from-disk/resync-linear-design-round-handoff-v1.md`,
  review `rfc167-design-round-review-v1`.
- **Delivered 2026-10-05** (`becaadbe`, review `rfc167-implementation-review-v1`):
  - M5 is linear: 0.27 s at 64 MiB, against hours;
  - the doubled `verify` is gone (`03d3be22`'s second decode);
  - a command-level guard whose control was run.

  One text note is carried into release prep.
- **Correction (2026-10-05):** D4 asked for command-level rows on three inputs (the WAL, a container, the ref log).
  Only the WAL row was delivered, and the architect accepted it without noticing. The other two are carried into 0.49.0
  step 5, round 1, §3.

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
   - **Content from outside the repository reaches them indirectly.** A received blob's bytes are stored inside sound
     frames. If any earlier frame in the same container is torn or damaged, the forward scan meets crafted headers
     inside later bodies.
   - **Bundle files themselves do not reach the scan:** `decode_bundle` parses sequentially, with bounded frames.
   - **So M5 is a denial-of-service shape** that needs received content plus local damage.
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
  partial frame was never acknowledged by this binary. **The design round found it cannot settle a WAL that an older
  binary appended to** (§7), so the scan stays.
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

## 4. The design

**D1 — one work budget per decode call.**
- **The unit is bytes hashed:** at most 8× the length of the input being decoded. **A constant, not a config key:** a
  key would add a surface and a way to misconfigure a safety check.
- **Two placements, both required** (each was measured insufficient alone):
  - **narrow:** inside `sound_frame_after_partial`, between candidates;
  - **broad:** in each reader's own decode loop, so that the reader's ordinary per-frame checksum is counted too.
- **The budget is an explicit value passed through each reader's decode,** not a thread-local counter in every build.
  It is part of the call, so it is visible and testable. The thread-local hash tally stays test-only.

**D2 — exhaustion is a third outcome, "undetermined", and every reader treats it as damage** (C3). It is never a
tail.
- **Six readers are affected** (measured quadratic on `main`): the WAL, object containers, trust policy, received
  index, ref container and pointer index.
- **Four are immune already,** by exact-length or bounded-length checks: the object index, the generation logs, the
  trust keys and the author keys. They are unchanged.
- **The message,** one coherent statement per reader: *"the bytes after offset N look like many frame headers; prikk
  stopped checking after 8× the file's size and treats this as damage."*
- **Where RFC 164's Rule E applies** (an object frame nothing references), the frame is a harmless remnant, and the
  message says that, and only that.

**D3 — the ways out stay the existing ones.**
- WAL: `--repair-wal-tail`. Pointer index: `--repair-pointer-index-tail`. Trust policy and received index:
  `--repair-tails`.
- **Object containers and the ref container: no automated repair, as today** (RFC 164 ruled them report-only). An
  unreferenced frame is a remnant (Rule E). A referenced one was damage before the budget too, and the way out is a
  copy.

**D4 — guards that can fail:**
- **The reader level:** a linear bound (bytes hashed at most 8× the input, plus slack), shapes A and B, for all six.
  It replaces the 1.5× ceilings, and the ignored ceiling test goes.
- **The command level:** whole-`verify` bytes hashed on hostile input (the WAL, a container, the ref log) at most k× the
  input, **with a control: a second decode added turns it red.**

**D5 — the doubled `verify`.**
- `verify` on a 2 MiB hostile WAL took 14.73 s on 0.48.0 and 29–33 s on `main`. **On honest input it means a second
  full decode.**
- It entered between `24ca5991` and `a222d2c2` (RFC 164 round 1's last fix, or round 2), and RFC 165 and 166 added
  about 4 s more.
- **The implementation finds the second decode and removes it.** Honest `verify` on a 1,000-commit WAL is measured
  against 0.48.0: at most 1.2× its cost.

**D6 — the records.** `current-state.md` and the 0.48.0 CHANGELOG line are corrected. The self-vouching header is
format-8 input, not 0.49.0.

## 5. Evidence

- **The reviewer's generator, on `/home` (btrfs), `main` against the prototype** (architect's runs):

  | size | `main` | prototype |
  |---:|---|---|
  | 256 KiB | `verify` exit 0, 0.54 s | exit 1, 0.00 s |
  | 2 MiB | exit 0, 33.14 s | exit 1, 0.02 s |
  | 16 MiB | not run (quadratic) | exit 1, 0.14 s |

- **All six affected readers are linear on the prototype** up to 64 MiB, at 0.25–0.29 s (team, both shapes); on
  `main` they take 9.9–16.3 s at 2 MiB, which extrapolates to 2.8–4.6 hours at 64 MiB.
- **Honest data:**
  - the existing suite, and the external reviewer's M1–M4, M8 and N1–N3 corpus: no change;
  - **an honest crash tail dense with real frame magics** (256 MiB of content, torn by a real `SIGXFSZ`): no verdict
    change. Random bytes after a magic almost never claim a length that fits, so they are rejected without hashing.
- **A hostile tail in an object container,** end to end on the prototype: `verify` exits 0 (a remnant) and `commit`
  works. Not a dead end.

## 6. Self-review: security, performance, and users (2026-10-05)

**Security:**
1. **The budget turns hours into a bounded refusal** on the files that can hold received content. A false *tail* is
   impossible by construction (D2).
2. **A false *damage* on honest data** is the risk. **Guard:** the implementation measures the largest
   bytes-hashed-to-input ratio over the suite, the RFC 133 corpus, `matrix.py` and a repository whose committed files
   hold thousands of frame magics. **8× must leave at least a 4× margin over it,** or the round stops and asks.
3. **No new trust:** the budget only ever adds a refusal or a report.

**Performance:**

4. **Honest cost is unchanged:** counting bytes is one addition per hash call. The implementation measures it against
   `main`.
5. **D5 restores the cost `verify` lost in 0.49.0.** That makes it the larger gain for ordinary users.

**Users:**

6. **One coherent message per case.** The prototype printed "treated as damage … a harmless remnant, not damage" in
   one line, which contradicts itself. D2 requires one statement, in plain words, with the way out where one exists.
7. **No new verb and no new flag:** the existing repairs are named where they apply, and "a copy" where none does.
8. **`current-state.md` and the CHANGELOG stop promising a format change** for 0.49.0 (D6).

**Residual:** a crafted container frame that something *does* reference has no automated repair. That was true before
this RFC, and it is RFC 164's accepted ruling.

## 7. Alternatives considered

- **(a) a container repair verb first:** a feature, not a cost fix. Deferred.
- **(b) leave object containers and the ref container unbudgeted:** leaves the denial-of-service shape open on the very
  files that hold received content. Rejected.
- **The WAL's witness instead of the scan:** it cannot settle a WAL that an older binary appended to. One mechanism
  for all six readers is simpler anyway.
- **A self-vouching header** (a header-only checksum): exact and cheap, but a format change. Format-8 input.
- **A budget counted in candidates:** cheap honest candidates would use it up as fast as hostile ones.
- **Runtime wrapping of `verify`'s stages** for the command guard: a test-level row catches the same regression,
  without runtime code.

## 8. Implementation, after acceptance (one round)

- **U1:** D1 and D2 in `frame_resync` and the six readers, every call site's "undetermined" arm listed.
- **U2:** D4: the reader-level bound and the command-level row, each with its control.
- **U3:** D5: find the second decode, remove it, measure honest `verify` against 0.48.0.
- **U4:** the honest margin (§6 item 2): `matrix.py` against `/home/nabbisen/.pgtmp/ext-matrix-83a42498.txt`, the RFC
  133 corpus, and the magic-dense repository.
- **U5:** D6 and the messages, quoted from the binary.

## 9. Out of scope

- A self-vouching header, and any other format change: format-8 input.
- A container repair verb.
- Network transport.

## 10. For the owner

1. **Accept the design** (D1–D6), or not.
2. **Object containers and the ref container inside the budget** (option (c), recommended; §7). Their way out stays
   RFC 164's: an unreferenced frame is a remnant, and a referenced one needs a copy.
3. **D5:** the doubled `verify` cost is fixed in this RFC's implementation (recommended). The alternative is a
   separate round.
