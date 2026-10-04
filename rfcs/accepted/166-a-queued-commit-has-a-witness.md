# RFC 166 — A queued commit has a witness: a damaged acknowledged commit is never read as a crash tail

**Status.** **ACCEPTED 2026-10-03 by the owner** (*"Accepted."*). Proposed and rewritten as a design the same day by
the architect (0.49.0 step 3, in the owner-approved schedule: *"N6's commit witness"*).
- **The architect's reading of "Accepted.", recorded here:**
  - the whole design is accepted: D1–D6 as amended by §13, and all seventeen of §13's items bind the implementation;
  - **decision 2 is P1**: the witness in its own file, and P3 is format-8 input;
  - **decision 3 is a disclosure** for 0.20.0 to 0.48.0, beginning with the troubleshooting entry, which lands first;
  - **K1–K7 bind it** as they bound RFC 165;
  - two implementation rounds (§11), the ways out last. The external review of the 0.49.0 candidate examines D5 (K6).
- **The decisions were taken in two steps:** decisions 2 and 3, and "almost accepted" for decision 1 pending the
  architect's review of security, performance and UI/UX (§13); then *"Accepted."* after §13 was written and folded into
  the body.
- **The design round** ran as `rfcs/handoffs/163-a-write-never-buries-a-crash-state/commit-witness-design-round-handoff-v1.md`
  and its Addendum 1:
  - report v1 was not accepted (review `rfc166-design-round-review-v1`), and that review found §1.6;
  - report v2 was accepted (review `rfc166-design-round-review-v2`), with the architect's measured corrections (§7).
- **Round 1 (D1–D4, D6) closed 2026-10-04** (`ee34ac63`, review `rfc166-round-1-review-v1`): 0 of 300 kills stuck,
  N6 refused end to end, 0.48.0 interop never a false loss. **Round 2 (D5) is live:**
  `rfcs/handoffs/166-a-queued-commit-has-a-witness/round-2-handoff-v1.md`.

**Author-review independence.** The architect wrote RFC 162 rule 3, the rule that produces N6. The external architect
found N6 (letter 015), and the architect's own §9 needed two late corrections (RFC 164 §9, §9.2). So:
- every verdict in §5 was built by the dev team in a prototype and checked by the architect's own probes (§7);
- the external architect has said letter 018 will look for "N6's witness" in the 0.49.0 candidate, and `matrix.py`
  gains rows for it. That review is the independent check.

**Carefulness.** RFC 165's K1–K7 were bound by the owner's *"we had better be careful around such design"*. This RFC
changes what `verify` says about queued commits and adds a refusal to a repair verb. **K1–K7 bind it in the same
way:**
- K1 plan-only and a printed plan;
- K2 fail closed;
- K3 every condition testable, with controls;
- K4 the recovery verbs raced;
- K5 nothing implicit;
- K6 the ways out last, with external review;
- K7 a second addendum goes back to the owner.

## 1. What is left: N6

1. **The defect** (external review 015, `matrix.py`, confirmed again in 016):
   - **One flipped byte in the body of the last queued commit.** `verify` exits 0, `status` shows trailing partial WAL
     bytes and one queued commit fewer than the user made, and `doctor --repair-wal-tail` removes the record.
   - **The bytes are kept** in `recovery/wal-<session>-at-<offset>-<hash16>.bytes`, so nothing is destroyed. But the
     user was told the commit succeeded, and it leaves the queue.
   - **Five cells are graded I1q on 0.48.0:** last record body flipped, length 2^62, length one less, file emptied, and
     file removed. The last two say "nothing is queued, the user was told 2 commits had succeeded".
2. **The cause is RFC 162 rule 3.** For the WAL, the tail is everything after the last sound record when nothing sound
   follows, "whatever its shape". A log that ends at its last sound record cannot tell an interrupted commit from a
   damaged one (015).
3. **Why the WAL kept rule 3.**
   - RFC 164 §9 and §9.2 made every other appended file call a complete record damage, decided by the checksum.
   - The WAL was the one exception. Damage there would block every `commit`, and the only way out would be a copy.
   - RFC 164 recorded that "N6's witness (0.49.0 step 3) replaces the trade-off with an exact answer".
4. **What 0.48.0 does instead:**
   - it discloses N6 (`current-state.md`, `durability-recovery.md`);
   - the repair counts *complete-looking* records among what it removes (`complete_records_removed`, `wal.rs`) and says
     so;
   - that count is a guess from the shape. A claimed length past the end of the file, such as 2^62, is not counted.
5. **Stale text:** `durability-recovery.md`'s claims table still says the WAL-tail repair "refuses complete-record
   integrity failures". It has not done so since RFC 162.
6. **A torn `ref-name` strands the queue** (found by the architect in the design-round review, 2026-10-03; shipped in
   0.48.0):
   - **Every** commit rewrites `ref-name` by a durable truncate, then a durable append, not only the first commit of a
     session.
   - A crash between the two, on any commit after the first, leaves acknowledged commits in the WAL and `ref-name`
     empty.
   - **Then nothing moves it:** `verify` exits 1; `commit`, `seal`, `--repair-wal-tail` and `--repair-tails` all
     refuse. The only way out is writing the ref name back by hand.
   - **Measured by SIGKILL of a second commit:** 12 of 150 kills on shipped 0.48.0, and 12 of 300 on `main`
     (`/home/nabbisen/.pgtmp/arch-166/commit_kill_probe.py`).
   - The external matrix grades "`ref-name` emptied" `ok`, because it is reported as damage. **It is reachable by a
     crash, though, and it has no way out.**
   - **It belongs here:** it is in the commit's own write sequence, and the design round's P3 placed the witness in
     this same file.

## 2. Facts from source

- **The frame** (`wal.rs`):
  - a 58-byte header: magic `PWALR001`, version, seq, body length, and a SHA-256 over all of them and the body;
  - the body is the signed Patch envelope;
  - seq counts from 1 and restarts after a drain.
- **The session directory** is `.prikk/active/<name>/`, holding `queue.wal`, `active.lock`, `ref-name` and
  `declarations`. `init` creates them, and they stay.
- **A commit, in order** (`node_authoring.rs`, under the active lock):
  - content blobs through the object store;
  - author key material;
  - `ref-name` (truncate, then a durable append, **on every commit**: §1.6);
  - the WAL append, durable (file and directory synced);
  - `declarations` cleared;
  - the report, "committed". **A commit is acknowledged only after its record is durable.**
  - A retry whose envelope equals the last record's appends nothing and returns the last seq.
  - **A retry from the CLI never reaches that branch:** with the content already queued, `commit` refuses "worktree has
    no node-addressed changes to commit" (design round, report v2).
- **A seal drains the queue last:** after the pointer and the log agree, `finish_active_publication_cleanup` truncates
  the WAL to empty, then clears `ref-name`.
- **Nothing durable today identifies an acknowledged commit outside the WAL itself:**
  - `cache/commit-index.v1` is a stat cache;
  - the object index is a pure cache (RFC 162);
  - blobs are shared and content-addressed;
  - the Patch envelope reaches the store only at seal.
- **A Patch names no ref:** `PatchPayload` holds operations, intent, preconditions, purpose and message. Ownership of a
  queue lives in `ref-name` alone.
- **The queue limit is 1,000.** The 1,001st commit is refused.
- **The format is 7,** shared with every release since format 7. `require_current_format` accepts any format-7
  repository, so **an older binary and a newer one can both write the same repository.**

## 3. Constraints (as proposed, and met by §4)

- **C1 — meaning (I6).** A record the user was told had committed is never removed silently, and never read as a crash
  tail. A record that was never acknowledged may still be removed as a tail. The witness decides which is which; it
  never decides what a record *means*.
- **C2 — the witness grants nothing.** It is unsigned local metadata, so it only ever *adds* a refusal or a report:
  - it never makes a record, a patch, a ref or a signature accepted;
  - **it never supplies data to a write** (§4 D5 takes the ref name from the user, not from the witness).
- **C3 — fail closed, nothing silent.** No repair removes a record the witness covers without an explicit, named
  acknowledgement of the loss (K5).
- **C4 — no format change.** An additive file in format 7, safe against an older binary writing the same repository
  (§6).
- **C5 — cost.** A commit costs what it costs today, measured (§7).
- **C6 — Rule D.** Every writer checks the witness against the WAL before its first write. A refusal writes nothing.

## 4. The design

**D1 — `ref-name` is written once per session.**
- A commit writes `ref-name` only when the WAL is empty (the session's first commit). Later commits never rewrite it:
  ownership cannot have changed, since a non-empty WAL already proves it.
- **That closes §1.6.** No commit tears `ref-name`. Measured with 300 kills each: 10–13 stuck states for the rewrite on
  every commit, and **0** with D1 (§7).
- `ActiveSession::append_patch` already has this shape.

**D2 — the witness: one record in its own file, replaced atomically after every acknowledged commit.**
- **The path is `.prikk/active/<name>/witness`.** It is written with `write_file_atomically` (file sync, rename,
  directory sync), so a crash leaves the old record or the new one, never a mix. Swept at every ordinal.
- **The record:**
  - magic and version;
  - the owning ref name;
  - the last acknowledged seq;
  - its Patch id;
  - its frame hash (W2);
  - a running hash over every acknowledged frame, `next = SHA-256(previous ‖ frame hash)` (W3);
  - a SHA-256 over all of it.
- **The write comes after the durable WAL append and before the report.**
  - A crash between the two leaves a sound record the witness does not cover. The user was not told, so it is not
    acknowledged, and it is kept as today.
  - **Every appender writes it, through one function** (§13 item 1): `commit` (`author_inner`), `ActiveSession::append_patch`
    and `rollback-draft` (`rollback/draft.rs`).
- **Seal's drain clears it,** in today's order: the WAL is truncated, then the witness is cleared, then `ref-name` is
  cleared. That order is the safe one: clearing ownership before the WAL leaves an owned queue without an owner, shown
  for every placement.
- **Why the ref name is in it:** connectivity (D3) needs a ref after a drain, and every drain clears `ref-name`.

**D3 — one classification, used by `verify`, `status`, `doctor`, the repair, and the checks `commit` and `seal` make
before their first write.**
1. **No witness file** (a legacy session, or one removed): rule 3 for this session, exactly as 0.48.0, with a line
   saying so. Removing the witness can therefore never make anything worse than 0.48.0.
2. **A damaged witness:** reported. It matters only if the WAL has a tail (row 7 of §5).
3. **The witness disagrees with the WAL** (it is ahead, or the same seq has a different identity), **so connectivity is
   tried first:**
   - if the witnessed Patch is sealed and reachable from the witness's own ref, the witness is **stale**. A drain it
     missed (an older binary's seal) left it behind;
   - **a stale witness counts as no witness** (item 1), with a note. The next commit replaces it.
4. **Otherwise, the verdict table in §5.**

**D4 — `verify` checks W3:** the running hash, recomputed over the WAL's acknowledged records. It catches a substituted
earlier record, which W2 cannot. Cost: 0.73 ms at 1,000 records (design round, library call).

**D5 — the ways out (K6: delivered last, in their own round, and examined by the external review).**
- **`prikk doctor --discard-damaged-commits [--plan-only]`** removes acknowledged damage (§13 item 14).
  - **The plan names each acknowledged record it would remove:** its seq, and the Patch id the witness holds for the
    last one. It also names the recovery file the bytes go to.
  - After the removal, the witness is rewritten to cover the sound prefix.
  - **The text says the content may still be in the working tree** only where `status` shows it as uncommitted.
- **`prikk doctor --restore-queue-target --ref <ref> [--plan-only]`** (§13 item 15) gives an owned queue its owner back, for a
  `ref-name` that is missing or disagrees with the witness. The 0.48.0 strandings of §1.6 are the main case.
  - **The ref comes from the user, never from the witness** (C2). `doctor` may show the witness's ref name as a hint.
  - **It refuses:**
    - when a witness exists and names a different ref;
    - when the queue does not validate against `<ref>`'s current tip by the same check `seal` makes, run without
      writing.
  - **The implementation round shows from source that such a check exists.** If it does not, the round stops and asks.
- **No other command calls either of them** (K5).

**D6 — `ref-name` is checked against the witness.** When both exist and their ref names differ, that is damage:
`verify` exits 1, the writers refuse, and D5's restore is the way out. **This closes the pre-existing
`ref-name` "final byte removed" gap** (`heads/main` read as `heads/mai`, `I1` in `matrix.py` today) for every session
that has a witness.

## 5. The verdict table (D3, item 4)

| # | the WAL | the witness | verdict | `verify` | `commit`, `seal` | the repair |
|---:|---|---|---|---|---|---|
| 1 | sound | agrees | healthy | 0 | proceed | nothing to do |
| 2 | sound | behind (a crash before acknowledgement, or an older binary's commit) | pending | 0, with a note | proceed; the next commit advances it | nothing to do |
| 3 | sound, plus a tail past the witnessed record | agrees | crash tail (never acknowledged) | 0, with the tail line | refuse, as today | removes it, as today |
| 4 | the record after the witnessed prefix is damaged or missing, and the witness names it | ahead by its seq | **acknowledged damage (N6)** | **1** | refuse | **refuses; `--discard-damaged-commits` (D5)** |
| 5 | shorter than the witness, or empty, and connectivity fails | ahead | **acknowledged loss** | **1** | refuse | **refuses; `--discard-damaged-commits`** |
| 6 | same seq, different Patch or frame hash, and connectivity fails | mismatch | **substituted record** | **1** | refuse | refuses; a copy is the way out |
| 7 | a tail | damaged | unknown | 1 | refuse | refuses; `--discard-damaged-commits` |
| 8 | sound, no tail | damaged | witness damaged | 0, with a warning | proceed; the next commit replaces it | `--repair-tails` rebuilds it (§13 item 5) |
| 9 | non-empty | any | ownership missing | 1 | refuse, as today | D5's restore |
| 10 | any | W3 disagrees | substituted earlier record | 1 | refuse | a copy is the way out |

- **Rows 4, 5 and 6 close the five I1q cells:**
  - a flipped body byte, length 2^62 and length one less are row 4;
  - an emptied or removed WAL is row 5.
- **Row 8 does not refuse,** because nothing is at risk: the WAL is wholly sound, and the witness only decides tails.
  Refusing there would add a dead end without protecting anything.
- **Rows 6 and 10 have no repair verb.** A sound record that is not the acknowledged one is not a crash shape. Removing
  it would be a guess, so the way out is a copy, and the text says so.

## 6. Format 7 and older binaries (C4)

Measured against the shipped 0.48.0 binary:
- **0.48.0 ignores the witness file:** it commits, seals and repairs a witnessed repository with no error, and leaves
  the file untouched.
- **A 0.48.0 commit after a witnessed one** leaves the witness behind: row 2, harmless.
- **A 0.48.0 seal** drains the WAL and leaves the witness behind. D3 item 3 finds the witnessed Patch sealed and
  reachable from the witness's ref, so the witness is stale, never a loss. **That is why the ref name is in the record.**
- **A 0.48.0 seal, then a 0.48.0 commit** (the same seq, a different Patch): connectivity is tried before row 6, so it
  is stale, never a substitution.
- **This binary on a 0.48.0 repository:** the first commit creates the witness. No migration is needed.

**The design round's prototype did not yet try connectivity in those two cases:** `readers.rs:152` and `:181-184`
report a loss after an ordinary 0.48.0 seal (review v2 §5). D3 item 3 is the correction, and the implementation tests
both sequences against the real 0.48.0 binary.

## 7. Evidence

From the architect's runs on the prototype (`e821775f…`) and `main` (`12b117d0…`), on `/home` (btrfs on LUKS),
release builds:

- **The kill probe, 300 kills of a second commit each:**

  | | stuck (`ref-name` empty) |
  |---|---:|
  | rewrite on every commit (today) | 13, 10, 12 |
  | D1 (P1Fixed) | **0** |
  | P3 | **0** |

  On shipped 0.48.0: 12 of 150.
- **Commit cost, interleaved,** the mean of 5 commits, 3 samples:

  | depth | `main` | witness alone (no D1) | **D1 + D2** | P3 |
  |---:|---:|---:|---:|---:|
  | 1 | 20.6–21.9 ms | 23.2–24.6 ms | **21.3–21.6 ms** | 20.9–21.3 ms |
  | 64 | 21.3–21.5 ms (one sample 25.2) | 24.0–24.3 ms | **21.6–22.1 ms** | 21.4–22.0 ms |

  **The witness write alone costs about 2.5 ms (12%). D1's saved rewrite pays for it.**
- **From the design round (report v2), checked where marked:**
  - **the crash windows:** the atomic replace leaves old or new, never a mix (failpoints at every ordinal);
  - **the drain order:** today's is the safe one;
  - **W1 is fooled by a substitution of the same length; W2 catches it.** W3 catches a substituted earlier record;
  - **`matrix.py`:** under P1, the prototype changes no existing cell. The witness's own rows are `SILENT` only because the
    prototype's readers are not wired into the CLI.
- **Corrections the architect made to report v2** (review v2):
  - its cost "baseline" was the witness itself;
  - its Q5 ran only P1, and P3 breaks both directions (§8);
  - the readers' connectivity order (§6);
  - its measured binary predates the kept source.

## 8. Alternatives considered

- **P3, the witness folded into `ref-name`** (one checksummed record, replaced atomically):
  - it has the same kill result (0 of 300) and the same cost as D1 + D2;
  - it closes the `ref-name` checksum gap for every session, not only witnessed ones;
  - **but it changes the format of an existing file.** Measured against 0.48.0:
    - **a 0.48.0 queue opened by the P3 prototype is stranded** ("malformed metadata"; `commit` and `seal` refuse).
      Reading the legacy plain name would fix that;
    - **0.48.0 cannot use a queue the P3 binary wrote** ("not UTF-8"). Nothing fixes that inside format 7.
  - **Rejected under C4,** because P1 carries the witness safely. If the owner prefers P3, it is a format change and
    belongs to format 8.
- **W1, a count or end offset:** fooled by a substitution of the same length.
- **W2 alone:** cannot see a substituted earlier record. W3 costs one 64-byte hash per commit.
- **P2, an appended witness log:** answers nothing P1 does not, and adds a file needing its own tail rules.
- **Refusing on a damaged witness over a sound WAL (row 8):** a dead end that protects nothing.

## 9. What remains

- **An absent witness is 0.48.0's behaviour:** a legacy session, or a witness removed by hand. They cannot be told
  apart, and both fall back to rule 3, reported.
- **The `ref-name` checksum gap remains for a session with no witness.**
- **Corruption that rewrites the WAL and the witness consistently** is beyond any unsigned local witness. A signed or
  chained frame is format-8 input, with RFC 164 §9.1's per-file witness.

## 10. For the owner

1. **Accept the design** (D1–D6, as amended by §13), or not. *Owner 2026-10-03: "almost accepted", pending §13.*
2. **P1 or P3** (§8). *Owner 2026-10-03: P1.*
3. **§1.6:** a disclosure or an advisory. *Owner 2026-10-03: a disclosure*, covering 0.20.0 to 0.48.0 (§13).

## 11. Implementation, after acceptance

- **Round 1 — D1 to D4, D6, with §13 items 1–9, 11–13, 16 and 17:**
  - **item 0:** the §1.6 troubleshooting entry, delivered and pushed ahead of everything else;
  - D1 next, as its own commit;
  - the witness written and cleared;
  - the classification wired into every reader and writer;
  - W3 in `verify`;
  - text, including the stale claims-table row and the repair's line.
  - **Tests:** a failpoint at every write ordinal of a second commit, the drain and the repair; every row of §5 built,
    each with a control that turns it red; the 0.48.0 sequences of §6 against the real binary; the kill probe; and
    `matrix.py`.
- **Round 2 — D5, the ways out (with §13 items 10, 14 and 15):** both verbs under K1–K5, raced against `commit` and `seal`, with every refusal
  condition paired with its control.
- **At the 0.49.0 candidate:** the external review examines D5 with RFC 165's R4 and R5 (K6).

## 12. Out of scope

- **The per-file witness for the other appended files** (RFC 164 §9.1), and multi-field corruption elsewhere: format-8
  input.
- **M5's structural fix** (0.49.0 step 4).
- **Non-default active sessions:** the witness sits in each session's own directory, so a later generalisation carries
  it unchanged.
- Network transport, and any change to what a signature or a ref means.


## 13. Self-review before acceptance (2026-10-03, at the owner's request)

Each item is a defect or risk the architect found in §4 as written, and the change that binds the implementation.

**Correctness and robustness:**
1. **A third appender.** `rollback/draft.rs:208` (`prikk rollback-draft`) also appends to the WAL and reports the patch
   queued; the prototype and D2 named only two paths.
   - **Change:** one session-level function appends *and* writes the witness, and every appender goes through it
     (`ActiveSession::append_patch` is the natural one).
   - `Wal::append_patch` is reachable from nowhere else, and a test lists its callers from source.
2. **W3 restarted from zero wrongly.**
   - The prototype folds only the new frame onto the previous witness's hash, while `verify` recomputes from seq 1.
   - **Any queue begun or extended by 0.48.0 would read as "a substituted earlier record"** (row 10): a false
     alarm that refuses commits.
   - **Change:** a witness written over a queue it does not fully cover (absent, stale or behind) folds every sound
     record after its last covered one, O(those records). A first witness over a legacy queue covers the whole
     queue: those commits were acknowledged by the older binary.
3. **The connectivity walk was unbounded** (the whole ref history), and it would run on every `status` while a loss is
   reported.
   - **Change:** the witness also records the ref's tip when it was written. The walk stops there, so it costs the
     seals since then (normally one). Reaching that tip without finding the Patch means a loss.
4. **Both WAL repairs must classify.** `--repair-tails` (RFC 164) also repairs the WAL. D3 said only "the repair".
   - **Change:** `--repair-wal-tail` and `--repair-tails` both refuse acknowledged damage and name the way out.
5. **A damaged or stale witness over a wholly sound WAL** (row 8) had no repair verb.
   - **Change:** `--repair-tails` rebuilds it from the WAL, covering every sound record. That only adds protection,
     so C2 holds.
6. **`ref-name`'s first-commit write still truncates, then appends.** A tear there is harmless, because the WAL is
   still empty. But the tear was introduced in 0.20.0 (`c1df7ec2`, the "marker pattern" migration); before it, the
   file was replaced atomically.
   - **Change:** the implementation round says from source why the marker pattern was adopted, and restores the
     atomic replace unless that reason stands.

**Security:**
7. **C2 holds call site by call site,** with one rule made explicit: the witness decides *whether* records were
   acknowledged, never *which bytes* a write removes or *which ref* a write targets. Those come from the WAL and from
   the user.
8. **A forged witness can only force refusals** (rows 4–7, 9, 10). That is denial of service by someone who can
   already write anything under `.prikk`. Deleting the witness restores 0.48.0's behaviour, no worse.
9. **Path safety:** the witness is read and written only through the anchored `MutationRoot` primitives, with a test
   for a symlinked `witness`, as for every other session file.
10. **Restoring the owner** (D5): when the queue validates against more than one ref's tip, the plan lists every one,
    and says so before writing. A user should never attach a queue to the wrong branch without being told.

**Performance:**
11. **Mean commit cost is unchanged** (§7): D1 removes two durable operations and D2 adds one atomic replace.
    - The atomic replace varied between 2.7 and 12.8 ms on LUKS, so **tail latency is not yet measured.**
    - **Change:** the implementation reports p50 and p95 over 100 commits against `main`.
    - A session's first commit pays about 2.5 ms more; seal's witness clear is negligible.

**UI/UX (users must not be confused):**
12. **"Witness" is never a user-facing word.** The text says what happened, for example "a queued commit you were told
    had succeeded is damaged". The codes carry the internal names.
13. **The interrupted commit** (row 2). Today a commit killed after its durable append looks failed, and retrying it
    answers "no node-addressed changes to commit", which is confusing.
    - **Change:** `status`, and that refusal, say that a queued commit was written but not confirmed, either because
      the command was interrupted or because an older prikk wrote it. That wording is honest: the two cannot be told
      apart.
14. **A removal of acknowledged commits is not a "tail repair."**
    - **Change:** a verb of its own, `prikk doctor --discard-damaged-commits [--plan-only]`, instead of a flag on
      `--repair-wal-tail`. The tail verbs keep their single meaning: never-acknowledged bytes.
15. **"Active ref" is internal vocabulary.** `status` already says "queued patches … targeting <ref>".
    - **Change:** `prikk doctor --restore-queue-target --ref <ref> [--plan-only]`.
16. **A stale witness after an older binary's seal is silent in `status`.** It is a note in `doctor` only, and the next
    commit or `--repair-tails` replaces it. It is not a problem the user must act on.
17. **Consumers:** the new `verify` codes and the `status` lines go under `### Output changes`, and stikk's letter at
    the cut names them (stikk consumes CLI output).

**What stays as designed:** P1; D1; connectivity first; the verdict table; K1–K7; two implementation rounds.

**Toward "finally clean":** `ref-name` and the witness now carry the owner twice, which D6 keeps consistent. **At format
8, P3 folds them into one checksummed record,** removing the duplication and the legacy fallback. Recorded as format-8
input, beside RFC 164 §9.1.

**The disclosure (decision 3):** a stranded queue on 0.20.0 to 0.48.0 (measured on 0.46.0, 0.48.0 and `main`).
- **Round 1, item 0:** a `troubleshooting.md` entry with the manual way out (write the ref name back), pushed ahead of
  the fix, so 0.48.0 users have it now.
- **Then** the known-limitations line until 0.49.0, and a CHANGELOG `### Fixed` entry at the cut.
