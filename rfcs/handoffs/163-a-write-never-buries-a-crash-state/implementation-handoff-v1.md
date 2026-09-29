# RFC 163 implementation — 0.48.0 candidate 2, part 2: a write never buries a crash state (scope B)

**Live 2026-09-29, and it is next.** Part 1 (`119-release-policy-reset/release-0.48.0-candidate-2-handoff-v1.md`) is
accepted and closed at `903c8f6f` (review `release-0.48.0-candidate-2-part-1-review-v1`).

**RFC 163 is ACCEPTED by the owner, 2026-09-29, with scope B** (*"Accepted."*, *"B"*). Read it all first:
`rfcs/accepted/163-a-write-never-buries-a-crash-state.md`, then 015's §2 (N1, N2, N3, N6) and §3.

## 0. Part 1's two follow-ups, first (review `release-0.48.0-candidate-2-part-1-review-v1`, §3)

1. **The call-job timeout exemption.** In `ci_timeouts.rs`, exempt only a job-level `uses:` whose value starts with
   `./.github/workflows/` and names a file present in the scanned directory. Tests:
   - a remote `uses: owner/repo/.github/workflows/x.yml@ref` job stays reported as missing;
   - a local `uses:` naming a file that is not there stays reported too.
2. **`durability-recovery.md`, "WAL Replay and Tail Handling":** "When the evidence is ambiguous the answer is damage,
   never tail" contradicts rule 3 as the same paragraph now states it. Scope it to whether a sound record follows, or
   remove it. Write it together with §5's N6 disclosure, which goes in the same place.

## 1. The rule at the five files of scope B

**Before an append, the writer confirms under its lock that the file ends at its last sound record. If it does not, it
refuses before writing anything.**

- **Files:** the pointer index, trust keys, trust policy, author keys, and the received index.
- **One shared check**, called at every append site of these files. The architect's grep finds these sites:
  - `refs/pointer_index.rs:502`;
  - `trust_index.rs:318` and `:605`;
  - `author/author_key_index.rs:450` and `:489`;
  - `received/received_index.rs:350`.

  **Confirm the list from source**, and add any site it misses.
- **The refusal** names the file, the offset where its last sound record ends, how many bytes follow, and the way out
  (§2). Use the integrity refusal class.
- **Where it reads from:** say for each site what it already reads before appending, and whether the check adds a read.
  A new whole read of a file that grows with the store needs a P1 scope row with its reason, or a header walk instead.

## 2. The way out

- **Pointer index:** the refusal names `prikk doctor --repair-pointer-index-tail`. After it, the same write succeeds.
- **The other four:** no repair verb in 0.48.0. The refusal names the offset, and `troubleshooting.md` gets one entry
  per refusal message: back up the file, truncate it to the named offset, run `prikk verify`, retry. **The matrix runs
  this manual way out**, so the entry is tested, not only written.

## 3. Every other appender: classify, do not fix

`append_file_required` has more callers than scope B's files. Among them are `foundation/generation.rs:130`,
`commit_boundary/active.rs:168`, `rename_declaration.rs:127`, `worktree_marker.rs:50` and `:137`, `compact.rs`, and
`refs/container.rs` (the ref log, out of scope by RFC 163 §2).
- For each, report the file it writes, and whether a crash tail followed by an ordinary write buries anything.
- **Show it with one write-first probe each**, not by argument.
- Anything that buries is **reported for the architect's ruling**, not fixed in this round.

## 4. N6: the WAL repair says when it removed a whole record

`doctor --repair-wal-tail` says when what it removed includes one or more **complete** records, naming how many. A removed
commit the user was told had succeeded is then never silent. This is an output change: put it in `### Output changes`.

## 5. The notes and the documents

- **`### Fixed`**: the burying at the five files. **Affected versions from history**, as part 1 did for M2 and M3; N1,
  N2 and N3 are in 0.47.0 (the architect's run of the reviewer's script on the released binary).
- **`### Output changes`**: the new refusal at each of the five files; the WAL repair's new line.
- **`current-state.md`, "Known limitations, measured"** (N5 item 5, and the disclosures):
  - remove "each is a cost or a silence, not a correctness defect";
  - **N2's remainder:** the four files have no repair verb, and `verify` says nothing about their tails (N7, 0.49.0);
  - **N3:** a crash inside `branch create` or `tag create` has no command that completes it, and a `seal` of another ref
    buries it;
  - **N6:** a damaged last WAL record is treated as a tail. `verify` exits 0, and the repair removes the record, keeping
    its bytes. Say what the user sees, and that 0.49.0 adds the witness.
- **`durability-recovery.md`:** the rule, the five files, the manual way out, the ref log's exclusion.

## 6. The matrix gains I5

In `rfc162_recovery_matrix.rs`:
- **Rows:** for each of the five files, a torn prefix and 100 zero bytes, then the ordinary write that appends to it:
  - `seal`, `branch create` and `tag create` for the pointer index;
  - `trust maintainer add` for trust keys and policy;
  - a commit by a new author for author keys;
  - `bundle import` for the received index.

  Then the way out (§2) and a `commit`.
- **Asserted:** the write exits non-zero, and the file is byte-identical to before it. The way out then reaches a
  repository where the same write succeeds, `verify` exits 0, and a `commit` is accepted.
- **Controls:** remove the check at each site, one at a time, and show which rows go red. **A site whose removal
  reddens nothing is a finding.**

## 7. The candidate

- The version is already 0.48.0 (`0f0ea817`). New entries go into the `## 0.48.0` section.
- **The architect gives the date when the candidate is accepted.** If that is a later day than `2026-09-29`, the heading's
  date changes in a commit of its own, and that commit is the candidate.
- **Before proposing it:**
  - the 14 gates on the exact final commit, in R1's scope;
  - the RFC 162 matrix green, the I5 rows included;
  - **the reviewer's `reproduce.sh` v2 and `matrix.py`** (`.git-exclude/upstream/external-architect/receive/015-…/
    reproduce/`), on a release build of the final commit and on 0.47.0's. Attach both outputs. N1 and N2 must be
    closed.

## 8. Units and budgets

| unit | what | budget (stop at ×2) |
|---|---|---:|
| P1 | the §3 probes, one per other appender | 30 min |
| P2 | `reproduce.sh` v2 + `matrix.py`, the candidate and 0.47.0 | 45 min |

**Report:** `.git-exclude/review-request/rfc163-write-never-buries-report-v1.md`, with:
- the site list and its read analysis;
- §3's classification table;
- each control with the rows it reddened;
- both outputs of the reviewer's scripts;
- the gates.
