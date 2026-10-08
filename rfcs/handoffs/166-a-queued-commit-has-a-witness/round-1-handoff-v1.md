# RFC 166 round 1 — `ref-name` once per session, the commit witness, one classification (D1–D4, D6)

**Live 2026-10-03, and it is next.** RFC 166 is ACCEPTED by the owner. **Read all of it first:**
`rfcs/done/166-a-queued-commit-has-a-witness.md`, and above all §4 (the design), §5 (the verdict table) and §13
(the seventeen items that amend it).
- **K1–K7 bind this RFC,** as they bound RFC 165.
- **Round 1 carries D1–D4, D6, and §13 items 1–9, 11–13, 16 and 17.** D5 (the two ways out) and §13 items 10, 14 and
  15 are round 2.

**The rules carried from earlier rounds:**
- **A cut is a question to the architect before delivery.** At ×2 of a unit's budget, file a question in
  `.git-exclude/review-request/` and wait.
- **Sweep the region:** every write ordinal, or the reached state asserted before anything is asserted about it.
- **Every site gets its own control, and a control must be able to fail.**
- **Measurements:**
  - build from the final commit in a clean worktree with `cargo build --release -p prikk --locked`, and state the
    sha256;
  - **every timing on `/home`, with `stat -f -c %T` printed;**
  - the calibration line (exit status, duration) before any sample.
- **No selector, environment variable or test-only switch in product code.** The design round's
  `PRIKK_166_WITNESS_OPTION` silently fell back on an unknown value, and cost a measurement.
- **The architect's probes** are in `/home/nabbisen/.pgtmp/arch-166/` and `/home/nabbisen/.pgtmp/arch-seal/`.

## 0. FIRST: the §1.6 troubleshooting entry, delivered alone

0.48.0 users can hit the stranded queue today, so the way out goes out before the fix.
1. **`docs/src/guide/troubleshooting.md`, one entry:**
   - **affected:** 0.20.0 to 0.48.0;
   - **the symptoms, quoted from the 0.48.0 binary** (`/home/nabbisen/.pgtmp/prikk-1c0d5b18`):
     - `status`: "queued patches: N targeting <missing metadata>";
     - `verify` and `doctor` exit 1;
     - `commit` and `seal`: "active WAL has records but active ref metadata is missing";
   - **the way out, run end to end on 0.48.0** against a real killed state (`commit_kill_recover.py`):
     - if the lock is stale: `prikk unlock --lock .prikk/active/default/active.lock --yes`;
     - write the branch name back: `printf '<branch>' > .prikk/active/default/ref-name`;
     - **how the user knows the branch:** name the command that shows it, verified on the binary. If none can, say
       so in the entry: the branch they were committing to;
     - `prikk verify` exits 0, then `seal`.
2. **`current-state.md`:** one known-limitations line for 0.48.0, linking the entry.
3. **Its own commit and its own short report:** `.git-exclude/review-request/rfc166-round-1-item0-report-v1.md`. The
   architect pushes it as soon as it is reviewed and gated, ahead of the rest of the round.

## 1. D1: `ref-name` is written only by a session's first commit

1. Both commit paths write `ref-name` only when the WAL is empty. `rollback-draft` already requires an empty WAL; keep
   that.
2. **§13 item 6:**
   - say from source why `c1df7ec2` (0.20.0) replaced the atomic replace with truncate-then-append, the "marker
     pattern";
   - restore the atomic replace for the first-commit write, unless that reason still stands. **If it does, that is a
     question to the architect,** not a silent keep.
3. **Tests:**
   - a failpoint at every write ordinal of a **second** commit: no state leaves a non-empty WAL without a readable
     owner;
   - **the kill probe** on the release build, `commit_kill_probe.py <binary> <workdir> 300`: **0 stuck**;
   - **the control:** the rewrite on every commit restored turns both red.

## 2. D2: the witness, through one function (§13 items 1, 2, 3, 9)

1. **One session-level function appends a record *and* writes the witness.**
   - All three appenders go through it: `commit` (`author_inner`), `ActiveSession::append_patch` and `rollback-draft`.
   - `Wal::append_patch` is reachable from nowhere else.
   - **A test lists its callers from source,** so a fourth appender fails the build's tests.
2. **The record:** magic and version; the owning ref name; the last acknowledged seq; its Patch id; its frame hash; the
   running hash; **the ref's tip RefState id at the time of writing (or none)**; and a SHA-256 over all of it.
3. **The write:** after the durable WAL append and before the report, atomically, through the anchored `MutationRoot`
   primitives only. **A test with a symlinked `witness`** gets the same refusal as for every other session file.
4. **The running hash (item 2):** a witness written over a queue it does not fully cover (absent, stale or behind)
   folds every sound record after the last covered one. A first witness over a legacy queue covers the whole queue.
5. **Seal's drain** clears it in today's order: WAL, then witness, then `ref-name`.
6. **Tests:**
   - a failpoint at every write ordinal of a commit, `rollback-draft`, and the drain, each state classified (§3);
   - **0.48.0 commits, then this binary commits:** W3 verifies (the false alarm item 2 removes), with a control.

## 3. D3 and D6: one classification (§13 items 3, 4, 5, 7, 8)

1. **One function, used by:** `verify`, `status`, `doctor`, `--repair-wal-tail`, `--repair-tails`, and the checks
   `commit`, `rollback-draft` and `seal` make before their first write.
2. **Connectivity first** for every witness that disagrees with the WAL. The walk stops at the recorded tip (item 3).
   Show the walk's length in a test: one seal after the witness means one step.
3. **The verdict table, §5 rows 1–10:**
   - **each row built by its own test, with a control that turns it red;**
   - rows 4, 5 and 7: **both** WAL repairs refuse (item 4).
   - **The refusal names no verb yet.** `--discard-damaged-commits` arrives in round 2, and a refusal never names a
     command that does not exist. It says the commit was acknowledged, and it cannot be removed as a crash leftover.
4. **Row 8 (item 5):** `--repair-tails` rebuilds a damaged or stale witness over a wholly sound WAL, covering every
   sound record.
5. **D6:** `ref-name` checked against the witness's ref name; a mismatch is row 9's refusal.
6. **C2, call site by call site (item 7):** list every reader of the witness and show that each one only refuses or
   reports. Nothing takes a byte range or a ref from it.
7. **K4:** race `--repair-tails`'s rebuild against `commit` under the existing lock harness.
8. **K5:** only the append function, the drain and `--repair-tails` write the witness. A test shows that every
   `doctor` mode without `--repair-tails`, and `verify` and `status`, leave it byte-identical.

## 4. D4, text and output (§13 items 12, 13, 16, 17)

1. **D4:** `verify` checks the running hash, which is row 10.
2. **User words (item 12):** "witness" appears in no user-facing line. The codes carry the internal names.
3. **The interrupted commit (item 13):** `status`, and `commit`'s "no node-addressed changes" refusal, say that a queued
   commit was written but not confirmed, either because the command was interrupted or because an older prikk wrote
   it.
4. **A stale witness (item 16):** silent in `status`, a note in `doctor` only.
5. **Text:**
   - `troubleshooting.md` quotes each new line from the binary;
   - `durability-recovery.md`'s stale claims-table row is fixed;
   - CHANGELOG `## Unreleased`: `### Fixed` (§1.6, 0.20.0–0.48.0), `### Changed`, and `### Output changes`: every new
     code and line, and the `status` lines stikk will read (item 17).
   - `current-state.md`'s N6 entry is rewritten in round 2, when the way out exists.

## 5. Gates, probes, units, report

**Before proposing:**
- the 14 gates on the final commit;
- on a release build of it (sha256 stated), on `/home`:
  - the kill probe, 300 kills: 0 stuck;
  - **§6's 0.48.0 sequences** against `/home/nabbisen/.pgtmp/prikk-1c0d5b18`, including a 0.48.0 seal followed by a
    0.48.0 commit: never a false loss, never a false substitution;
  - `matrix.py` v4
    (`.git-exclude/upstream/external-architect/receive/017-review-of-the-candidate-5e50a661/reproduce/`) against
    `matrix-5e50a661.txt`, every changed cell explained with its own replay. **Expected:** the five I1q cells
    reported; "`ref-name` final byte removed" caught when a witness exists;
  - the architect's `rfc164_*`, `rfc163_*` and `rfc165_*` probes in `arch-seal/`;
  - **cost (item 11):** p50 and p95 over 100 commits against `main` (`12b117d0…`) at depths 1 and 64; `seal` at 64 and
    1,000; `verify` at 1,000.

| unit | what | budget (stop at ×2) |
|---|---|---:|
| U0 | item 0: the troubleshooting entry, alone | 30 min |
| U1 | §1: D1, its sweep, the kill probe | 60 min |
| U2 | §2: the append function, the record, folding, the drain | 90 min |
| U3 | §3: the classification wired, rows 1–10 with controls, C2, K4, K5 | 120 min |
| U4 | §4: D4 and the text | 60 min |
| U5 | §5: the probes, `matrix.py`, the cost | 90 min |

**Nothing else runs while U5's timings do.** Each unit's real start and end time.

**Reports:**
- item 0: `.git-exclude/review-request/rfc166-round-1-item0-report-v1.md`;
- the round: `.git-exclude/review-request/rfc166-round-1-report-v1.md`.

**K7:** a second addendum on this round, or any finding that would change D1–D6, goes back to the owner before the
work continues.

**ACCEPTED and CLOSED 2026-10-04** (review `rfc166-round-1-review-v1`; earlier directions `-item0-review-v1`, `-u3-`, `-u4-`, `-u5-direction-v1`). Commits `6854bbdb` … `ee34ac63`. One required fix (the refusal text of `commit` and `seal` over acknowledged damage) opens round 2 as its item 0.
