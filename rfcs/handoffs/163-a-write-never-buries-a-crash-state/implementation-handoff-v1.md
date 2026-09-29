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

## Addendum 1 — 2026-09-29: four fixes (review `rfc163-write-never-buries-review-v1`)

Report `rfc163-write-never-buries-report-v1.md`, commit `ddf1e82a`. **Not accepted yet.** N1, and the burying at the four
N2 files, are closed and reproduced by the architect. **Fixes only in this addendum.** The generation log is not in it:
it is RFC 163 §9, proposed to the owner, and if accepted it comes as its own handoff.

1. **A refused `bundle import` writes nothing again** (review §2.1; blocks).
   - The received-index tail check must be decided **before the first object write**, with every other decision (the
     phase that ends at `bundle.rs:893`'s "Past this point only I/O"). Today it fires inside
     `append_received_index_entry`, after the objects and author keys are written.
   - **Test:** a torn received-index tail, then `bundle import`. It exits non-zero, and **every file under `.prikk/` is
     byte-identical** afterwards, not only the object containers.
   - **Control:** move the check back after the writes. The test goes red.
   - **Then check every other refusal RFC 163 added** against the same rule, for `bundle import` and `sync accept`, and
     list each one with where it fires relative to the first write.
2. **Refuse only when the operation will append to that file** (review §2.2).
   - **Author keys:** a key already recorded appends nothing, so its writer does not refuse over the author-key tail.
     This covers a commit by an already-recorded author, a rollback draft, and a `bundle import` or `sync accept` whose
     keys are all recorded. When at least one key would be appended, the refusal still comes in the pre-write phase, as
     item 1 requires.
   - **Trust keys and trust policy:** the same, for `trust maintainer add` and `remove`: refuse only when that file will
     be appended.
   - **Tests:** with a torn author-key tail, a commit by the recorded author succeeds and leaves the file byte-identical.
     A commit by a new author refuses.
   - **Controls:** as before, one per site.
3. **The received-index read** (review §2.3).
   - Correct the comment: the file grows with every import that records a pointer, until `compact`.
   - **Measure** the new replay's cost on a release build, at 1,000 and 10,000 received-index entries: time and bytes
     read per import.
   - Give the read a P1 scope row with its reason, or replace it with a header walk. Say which, and why.
4. **Text** (review §2.4):
   - **Known limitations:** retitle the N2 bullet to what remains (the four files have no repair verb, and `verify` says
     nothing about their tails).
   - **Disclose the garbage-shaped tails:** at these four files, a tail of zeros or random bytes is damage by their shape
     rule. `verify` fails, the reading commands refuse with "has a damaged entry; run doctor before reading", and no
     command repairs it (0.49.0, a tail defined by position). Add a `troubleshooting.md` entry for that message on these
     files that says exactly this, and **gives no truncation advice unless the offset it names is shown to be followed by
     nothing sound**.
   - **Troubleshooting:** remove `sync accept` from the received-index entry. After item 1 its "nothing … is written" is
     true, so a test holds it. Rewrite the author-key entry after item 2.
   - **CHANGELOG:** make it match all of the above.

**Before proposing:**
- the 14 gates on the exact final commit, in R1's scope;
- the matrix green;
- `reproduce.sh` v2 and `matrix.py` on a release build of the final commit. Explain every remaining I5 cell by name.

**Report:** `.git-exclude/review-request/rfc163-write-never-buries-report-v2.md`.

**ACCEPTED 2026-09-29** (`ddf1e82a` + `acef8f3a`; reviews `rfc163-write-never-buries-review-v1`, `-v2`). The architect
ran 14/14 gates and probes on the release build, with a control on the previous build: a refused import leaves every
`.prikk/` file identical, and a recorded author's commit is not refused. The reviewer's scripts show N1 and N2 closed;
the remaining I5 cells are the disclosed garbage-tail case. **RFC 163 at scope B is delivered.** The generation log
(§9) awaits the owner.
