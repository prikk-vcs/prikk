# RFC 164 round 2 — refuse before the first write, and a remnant is not damage (Rules D, E)

**Live 2026-10-01, and it is next.** Round 1 (Rules A, B, C, with §9 and §9.2) is ACCEPTED: review
`.git-exclude/reviewed/rfc164-round-1-review-v4.md`. **Read RFC 164 §5 first**
(`rfcs/accepted/164-every-appended-file-has-a-way-out.md`), and §9 to §9.2 for what a tail is now.

**Three rules carried from round 1, for every item below:**
- **A cut is a question to the architect before delivery**, in a review request. It is not a disclosure afterwards. A
  cut delivered without a ruling is treated as not done.
- **Sweep the region, not one sample.** Faults go at every offset or every write ordinal the item names, and readers are
  checked, not only exit codes.
- **Every site gets its own control**, reported with the rows it reddens. A site whose removal reddens nothing is a
  finding.

## 1. Rule D — refuse before the first write

1. **Classify every writer from source** that appends to a guarded file: the WAL, the pointer index, and the seven
   Rule-A files. At least `commit` (including a new author's first commit), `seal`, `branch create`, `tag create`,
   `merge`, `sync accept`, `sync seal`, `bundle import`, `trust maintainer add/remove`, `compact`, and any other.
   - For each, give the guarded files it appends to, and **where its tail check runs today relative to its first
     content-object write.**
2. **Each one checks every guarded file it will append to at the start of the command, before any content object is
   written**, as `bundle import` already does (RFC 163 §10).
   - **The check is §9.2's**: a tail, or a complete damaged record, refuses. Use one shared entry point, not a copy per
     writer.
   - **A refusal writes nothing at all**: no object, no index entry, no recovery file. Compare the `.prikk/` tree
     before and after, the way `arch-seal/rfc163_other_writers_probe.sh` does.
3. **Tests:**
   - every writer × every guarded file it appends to × four faults: a torn prefix, 100 zero bytes, 100 random bytes,
     and a flipped byte in the last complete record. Assert a refusal, and **an identical tree**;
   - **control, per writer:** move its check after its first content write. Its rows go red.

## 2. Rule E — a remnant is not damage

1. **Classification, in `verify` and `doctor`.** A stored object whose references are missing is:
   - **damage** if something committed reaches it: a ref, a received pointer, a queued patch, or a sealed block reached
     from them;
   - otherwise an **unreferenced remnant**: a warning naming the object and what it lacks, saying "re-run the import if
     you still have the bundle; otherwise it is harmless". `verify` exits 0 over it.
2. **No command removes a remnant in 0.49.0** (RFC 164 §5).
3. **Security, explicitly. Rule E must never hide real damage:**
   - reachability is computed from committed state only, never from the object's own claims;
   - a remnant that later becomes reachable (a ref moved to it, a pointer received) turns into damage at once.
4. **Tests:**
   - the killed-import shape from step 0 (`arch-seal/import_kill_classify.py`): every dangling remnant classifies as a
     remnant, and `verify` exits 0;
   - synthetic rows: an unreferenced block missing its patch is a remnant; the same block reached by a branch, a
     received pointer, a queued patch, or a sealed block's parent is damage. **One row per kind of reacher**;
   - a remnant made reachable afterwards: `verify` 0, then 1;
   - **controls:** (a) classify every missing reference as a remnant: the reached rows go red; (b) drop one kind of
     reacher from the reachability walk, one at a time: its rows go red.

## 3. Carried from round 1's review

- **CHANGELOG `### Security` and `current-state.md`:** "every reader" becomes what was measured.
  - The pointer index and its generation log were measured with `branch list`; the trust policy with
    `trust maintainer check`.
  - Or show from source that every ref reader resolves through one replay, and say so as code, apart from the
    measurement.
- **Report v4 said `rfc164_all_files_reader_probe.sh` ran 527 offsets.** The architect's run on the identical binary
  ran 747. Say why in one line (a skipped file, or a different fixture?).

## 4. Text

- `durability-recovery.md`: Rule D (refuse before the first write) and Rule E (remnant against damage).
- `troubleshooting.md`: the remnant warning, keyed on its exact text.
- `current-state.md`: step 0's bundle-less case leaves the known limitations, or shrinks to what remains.
- CHANGELOG `## Unreleased`: `### Changed` and `### Output changes` (the new warning, refusals that move earlier).

## 5. Gates, units, report

- **The 14 gates on the final commit**, in R1's scope.
- **On a release build of the final commit:**
  - the architect's `rfc163_*` probes and `rfc164_all_files_reader_probe.sh`;
  - `import_kill_classify.py`, 300 kills: every dangling result classified as a remnant, and `verify` 0;
  - `matrix.py` v4 compared with `matrix-5e50a661.txt`, every changed cell explained with its own replay.

| unit | what | budget (stop at ×2) |
|---|---|---:|
| U1 | Rule D: classification, the writer × file × fault rows, the controls | 60 min |
| U2 | Rule E: classification, the rows, the controls | 60 min |
| U3 | the release-build probes and `matrix.py` | 45 min |

**Report:** `.git-exclude/review-request/rfc164-round-2-report-v1.md`.
