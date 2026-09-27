# A loop that never ends must not take the machine with it — RFC 160 §9 (R2–R4), handoff v1

**Live 2026-09-27, and it is next.** F3 closed at `bb81b0fb`: CI run `36280930300` went 16/16, with both recovery-file
tests green by name on Windows. **In 0.48.0** (owner, 2026-09-27: *"All approved and authorized."*). The external review
of `bb81b0fb` (letter 014, answered the same day) graded this round **blocks 0.48.0**, and asked for the three
amendments marked **[014]** below. **Next after this:** the recovery-model design (RFC 160 §10, being written by the
architect for the owner's reading), then its implementation, then 0.48.0 prep.

**Read first:**
- `rfcs/accepted/160-costs-that-follow-the-store-and-lengths-read-from-disk.md` §9;
- `.git-exclude/reviewed/torn-tail-is-one-frame-review-v3.md` §0 (R1, the cgroup scope, which applies to every run in
  this round);
- the external review: `.git-exclude/upstream/external-architect/receive/014-review-before-0-48-0-findings-and-answers/014-review.md`,
  its M5, M6, D3 and D11 above all.

## 1. What lands

1. **R2 — no decode loop can stop advancing.**
   - One helper, in `foundation/frame_resync.rs` or beside it. It takes the current offset and the next one, and returns
     the next only if it is **strictly greater**; otherwise an `Integrity` error naming the reader and the offset.
   - **Every framed reader's loop advances through it:** the nine readers of F3's table, plus any other loop you find
     that walks a byte buffer by an offset it computes. List them.
   - The WAL, index and container loops are the priority.
   - The result: a loop that would spin becomes an error on the spot, never a hang and never an unbounded `Vec`.
2. **R3 — every framed reader terminates on arbitrary bytes.**
   - A property test per reader, reusing P4's harness: random bytes, and valid records mutated by flips, truncations
     and duplicated spans, bounded in size (say at most 64 KiB).
   - Each case runs in a **child process under an address-space cap and a timeout**.
   - Each must finish, and **must report no more outcomes than there are input bytes**, since each outcome consumes at
     least one byte.
   - Seeds are fixed, so a failure reproduces; a `proptest-regressions/` file is committed only for a real product
     failure.
   - **[014] R3 bounds work, not only outcomes** (the external review's M5).
     - Measure bytes read **and bytes hashed** per input byte, using the read tally P2 already has plus a hashing tally
       you add.
     - Assert a fixed bound per reader, for example at most 8 × the input.
     - **The review's hostile WAL tail is a named case:** a torn tail packed with frame headers, each claiming a body
       to the end of the file. Today `verify` over 2 MiB of it takes **14 s** (four times per doubling; the architect
       reproduced 0.25 / 0.88 / 3.49 / 14.02 s at 256 KiB → 2 MiB), because `sound_frame_after_partial` fully parses,
       and so hashes, every candidate.
     - **Fix the quadratic in this round if it is mechanical.** For example, a candidate's claimed length must fit
       before its body is hashed, and resync never re-hashes a region it has already rejected. Otherwise report it:
       the structural fix (a header that vouches for itself) is format-8 design input, and R3's bound then documents
       the worst case.
   - **[014] P2's open rows get a ceiling as well as their floor** (the review's D11). Today a `FollowsContent` or
     `FollowsHistory` row fails only if the finding disappears, so F1 or F4 growing tenfold would pass. Each open row
     asserts `large ≤ measured today × 1.5`, and the report gives the numbers.
3. **R4 — every CI job has `timeout-minutes`.**
   - All 15 jobs of `ci.yml`, plus the jobs of `docs.yml`, `docs-pr.yml`, `release.yml` and `security-audit.yml`.
   - Each is set from its **measured** duration: the last ten green runs, via `gh run view --json jobs`. The limit is
     2× the slowest, rounded up to 5 minutes.
   - Give the table (job, slowest measured, limit) in the report.
4. **[014] D3 — a release is mechanically tied to a green CI run.**
   - `release.yml` gains a first job that fails, and so stops the release, unless **every** CI job for the tagged
     commit concluded `success`, the Windows and macOS mutation suites included.
   - It uses `gh api`/`gh run list --commit <sha>` with the workflow's own token, read-only.
   - Today the release workflow fires on any matching tag, runs no tests and checks no CI result.
   - **Show it red:** run its check against a commit whose CI had a red job (for example `85d699af`, where CI run
     `36204467050` failed) and against a green one (`bb81b0fb`, run `36280930300`).

## 2. Controls — each shown red, **every run under R1's scope**

1. **R2:** make one reader's resume return the same offset. The test gets an `Integrity` error, **not a hang**. Run it
   under R1 **and** a timeout, and show that the timeout was not what ended it.
2. **R2, per reader:** remove the helper from each reader in turn. The shared termination test goes red for that reader
   only.
3. **R3:** a deliberately non-terminating decode in a test-only reader is caught as a timeout or an outcome-count
   violation. The suite stays alive.
4. **R4:** the `ci.yml` check that every job has `timeout-minutes` goes red when you drop one. Do it as a
   release-policy or test gate that reads the workflow files.
5. **[014] R3's work bound:** the hostile WAL tail at two sizes is within the bound after your fix (or documented at
   its worst case). **Perturb:** re-hash every candidate. The bound goes red.
6. **[014] P2's ceiling:** inflate an open row's operation (read the store twice). The ceiling goes red.

## 3. Units and budgets

| unit | what | budget (stop at ×2) |
|---|---|---:|
| T1 | R3's property tests, the whole set, one run | 10 min |
| T2 | the CI duration survey (`gh`, read-only) | 5 min |

## 4. CHANGELOG (RFC 161 shape)

`### Fixed`, only if R3 finds a real non-terminating input. Otherwise nothing user-visible changes, and no entry.

Report: `.git-exclude/review-request/runaway-guards-report-v1.md`. It covers:
- the gates on the exact final commit, **run in R1's scope**;
- the loop list;
- the controls with their perturbations;
- T1;
- the R4 table.

## Addendum 1 — 2026-09-27: two tests, then the push

Report `runaway-guards-report-v1.md`; review `.git-exclude/reviewed/runaway-guards-review-v1.md`. **The behavior is
accepted.** The architect re-ran the gates (14/14 on `59c085a8`, in R1's scope) and perturbed R2 under a cap: a
non-advancing resume is an `Integrity` error in 0.00 s. **M5's fix goes to 0.49.0**, as its own design round.

1. **M5 gets a standing guard.** A non-ignored case at a small size (32 and 64 KiB) asserts bytes hashed at or below
   **1.5 × today's measurement**, with a comment that the 0.49.0 fix replaces it with the 8× bound. **Perturb:** re-hash
   each candidate twice. It goes red.
2. **`isolated_with_timeout` is exercised by a committed test:** a child that never exits is reported as a timeout
   within its limit, and the parent survives. **Perturb:** remove the kill, bounded by R1, and report what happened.

The handoff's control 2 was ill-specified by the architect. It is replaced by a `require_progress` source scan in
0.49.0, and is not owed here.

Gates on the exact final commit, **in R1's scope**. Report: `.git-exclude/review-request/runaway-guards-report-v2.md`.
Then the architect pushes, and the round closes on a green Windows mutation suite.

**ACCEPTED 2026-09-27** (`023ea46c` … `3e56d604`; reviews `runaway-guards-review-v1`, `-v2`; 14/14 gates re-run by the
architect on `3e56d604` in R1's scope, 2435 / 0 / 56). **M5 has two quadratic paths, measured by the architect:**
- RFC 102's `Invalid`→resync, long-standing, is guarded by this round's standing ceiling;
- F3's partial-frame scan, new in this cycle and the external review's own case, gets its ceiling in the RFC 162
  implementation round, which rewrites that path.

Both fixes go to 0.49.0. **Closes when the Windows mutation suite is green on the pushed commit.**
