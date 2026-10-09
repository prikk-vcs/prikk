# RFC 165 round 2 — the ways out: `prikk ref complete`, the ref log speaks, `doctor --rebuild-pointer-index` (R4, R5, R6)

**Live 2026-10-02, and it is next.** Round 1 (R1–R3) is ACCEPTED and pushed (`522f8c0f`, review
`.git-exclude/reviewed/rfc165-round-1-review-v2.md`).
- **Read all of RFC 165 first**, `rfcs/done/165-ref-publication-one-read-a-log-that-speaks-and-a-way-out.md`, and
  above all **K1–K7 in its Status.** The owner accepted this design with *"we had better be careful around such
  design"*.
- **This round changes what a ref means.** Every item below names the K-rule it serves, and an item without its K-tests
  is not done.

**The rules carried from earlier rounds:**
- **A cut is a question to the architect before delivery.** At ×2 of a unit's budget, file a question in
  `.git-exclude/review-request/` and wait for the answer.
- **Sweep the region:** every write ordinal, or the reached state asserted before anything is asserted about it. Never a
  bare failpoint ordinal.
- **Every site gets its own control, and a control must be able to fail.**
- **Build from the final commit in a clean worktree, and state the sha256.** Name the filesystem with every timing.
- **The architect's probes** are in `/home/nabbisen/.pgtmp/arch-seal/`.

## 0. FIRST, before anything else: CI on `main` is not green (added 2026-10-02, before any delivery)

CI run `36936365842` on `522f8c0f`:
- **`msrv-1.85.0` was cancelled at its 20-minute limit,** in its Test step. It took 6 to 9 minutes on the three runs
  before;
- `stable` took 17 minutes, against 8 before.

**Cause, measured by the architect:** `refs/tests/one_read_per_publication.rs` takes **239 s by itself**, single-threaded,
on a 32-core machine. It builds 1,024 generations in a debug build. The store's library tests went from 106 s to 380 s
locally (`690cb778` to `522f8c0f`).

1. **The default suite runs the read-count tests at generations 4 and 64 only.**
   - The property is flat (one read at every depth), and 64 shows it.
   - **1,024 becomes an `#[ignore]` measurement,** run in U5 on the release build and reported there.
2. **Do not raise any CI timeout.** A timeout is a cost budget, and this round must return to it.
3. **Before anything else is proposed:**
   - the store's library-test time locally, before and after this item;
   - **a green CI run on a push of this fix alone.** The architect pushes it as soon as it is gated, ahead of the rest of
     the round. So deliver it first, as its own commit and its own short report:
     `.git-exclude/review-request/rfc165-round-2-ci-fix-report-v1.md`.

## 1. R5 first: what a ref-log tail is (K2)

R4 and the rebuild both depend on this, so it comes first.

1. **§9.2 for the ref log.** At the first position after the last sound record, if the stored checksum verifies over
   the claimed length or the length to the end of the file, the bytes are a **complete record**: damage, never a tail.
   - Use the shared `complete_by_checksum` helper (RFC 164 round 1), as the other six decoders do.
   - `has_interior_damage` and `ref_log_container_tail` follow it. **Today they treat any terminal `Failed` as a tail
     "regardless of shape"** (review v2 §2). That must end in this round.
2. **The classification, one table, used by `verify`, `doctor`, the precondition and the repair alike:**
   - **a torn last record while the pointer leads:** an interrupted publication, completed by R4, never truncated;
   - **a tail with no lead** (zeros, garbage, an incomplete header): a tail. Rule B reports it as a warning, and
     `--repair-tails` covers the ref log, saving what it removes, all-or-nothing as for the other files;
   - **a complete damaged record, anywhere:** damage. `verify` fails, and both the repairs and the readers refuse. The
     way out is a copy.
3. **Tests:**
   - the whole-record sweep: every offset of the last ref-log record, flipped, never decodes to a tail;
   - the RFC 164 shapes (torn, zeros, random, a flipped byte in the last complete record), × lead or no lead, × the
     repair before or after a write;
   - I1–I6, I6 included: no reader returns an older state;
   - **controls:** §9.2 bypassed at the ref log, which turns the flip rows red; the ref log removed from
     `--repair-tails`, which turns the tail rows red.

## 2. R4: `prikk ref complete <ref>` (K1, K2, K3, K4, K5)

1. **The rule (RFC 165 R4), every condition evaluated before any write.** It refuses and writes nothing if any of these
   cannot be evaluated, or fails:
   - (a) the leading RefState verifies under the current trust policy, signed by an adopted maintainer key;
   - (b) it chains: its ref name, its previous state equal to the log's tip, the next sequence;
   - (c) its target exists, with the kind the ref requires;
   - (d) for `seal` and `sync seal`, the WAL evidence matches;
   - (e) **no complete damage anywhere in the ref log or the pointer index** (§1).
2. **K1:**
   - `--plan-only` prints the plan and changes nothing: the ref, the leading RefState id, its signer key id, its target
     and kind, the log tip it chains to, the sequence, the completing key, and any partial tail it would remove;
   - a real run prints the same plan before writing.
   - **Test:** `--plan-only` leaves the tree byte-identical, and its plan equals the real run's plan.
3. **The write:** one signed ref-log record, by the completing key, through `publish_locked`'s existing completion path
   (`finish_interrupted_publication_with_object_store`). Never a new code path for the append itself.
4. **Reporting:**
   - a completable lead reports `PRIKK-VERIFY-REF-POINTER-LEADS-LOG`, and `doctor` names `prikk ref complete <ref>`;
   - a lead that fails (a)–(e) stays `PRIKK-VERIFY-REF-DIVERGENCE`, and `doctor` names the rebuild (§3);
   - `require_retained_evidence` becomes condition (d), for WAL-consuming publications only.
5. **The stuck entry points:**
   - `branch create`, `tag create`, `sync adopt-tag` and `merge`, when blocked by their own ref's interrupted
     publication, name `prikk ref complete <ref>` in their refusal;
   - **`sync seal` recognises its own interrupted publication,** so its retry completes through its existing no-op path;
     test it.
6. **K3, the negative tests:** each condition (a)–(e) gets a constructed case that fails only that condition. Each must
   refuse and write nothing, and each has a **control** that removes the condition and turns the test red. The cases:
   - a lead signed by a key that is not adopted, and one signed by a key adopted then revoked;
   - a previous state that is not the log tip;
   - a sequence gap, and a repeated sequence;
   - a missing target, and a target of the wrong kind (a Tag object for a branch);
   - a WAL mismatch, for `seal`;
   - complete damage elsewhere in the log.
7. **K4, it is a writer:**
   - failpoints at every write ordinal of `ref complete` itself. Each crash state is classified by the same table, and a
     second `ref complete` then finishes it;
   - **a race:** `ref complete` against an ordinary `seal` of another ref, and against a `commit`, under the existing
     lock-race harness. One writer wins and nothing is lost.
8. **K5:** no command calls `ref complete` implicitly. **Test:** `doctor` (every mode) never writes a ref-log record.

## 3. R5: `prikk doctor --rebuild-pointer-index` (K1, K2, K3, K4, K5)

1. **Structural, never trust-filtered:** each ref's newest record by its chain, integrity checked. **No signing.**
2. **It refuses, writing nothing**, if:
   - the ref log has damage or a tail (repair first);
   - **any lead is completable** (complete first): a rebuild would drop an authorized transition;
   - any input cannot be evaluated (K2).
3. **It writes** the rebuilt index into the other slot and switches the generation log, as compaction does. The switch
   is atomic, and the old slot stays until the next compaction.
4. **It drops leads that fail R4's rule, and names each one** in its plan and its output.
5. **K1:** `--plan-only` prints, per ref, its state before and after, and every dropped lead. A real run prints the same
   plan first. The same equality test as R4.
6. **Tests:**
   - byte-identical to the live index on synthetic and CLI-built repositories at two depths. The design round's
     comparison, made permanent;
   - **a revoked key does not move a ref;**
   - a complete damaged pointer-index record: the rebuild serves it, and the ref ends where its newest sound log record
     says;
   - refusal over a completable lead, and over ref-log damage;
   - **controls:** remove the completable-lead refusal, and the rebuild drops an authorized transition (the test goes
     red); add a trust filter, and the revoked-key row goes red.
7. **K4:** failpoints at every write ordinal of the rebuild, including the generation-log switch. Each crash state
   reads as either the old or the new index, never a mix. A race against `seal` and `commit`.
8. **K5:** never run implicitly.

## 4. R6: text

- Quote the binary's real lines in `troubleshooting.md`: the three divergence texts, the pointer-leads texts with and
  without a tail, the new tail and refusal texts.
- Correct the missing-reference forms to `object <owner> references missing <role> <id>`.
- `commands.md`, `--help` (the help-inventory gate), and `durability-recovery.md`: both verbs, K1's plan, and what each
  refuses.
- `current-state.md`:
  - N3 and M4 leave the known limitations;
  - what remains: a complete damaged last ref-log record needs a copy, and multi-field corruption awaits format 8.
- CHANGELOG `## Unreleased`: `### Added` (both verbs), `### Changed`, and `### Output changes` (codes, refusals, the new
  lines).
- **Carried from RFC 164's release-prep list:** `compact`'s live-slot refusal entry, and the generation-log refusal's
  advice becoming `prikk doctor --repair-tails`.

## 5. Gates, units, report

**Before proposing:**
- the 14 gates on the final commit;
- on a release build of it (sha256 stated):
  - the architect's probes in `/home/nabbisen/.pgtmp/arch-seal/`: the three `rfc164_*`, the seven `rfc163_*`, and
    `rfc165_ref_log_tail_shapes_probe.sh`. **Its flip rows must now say damage, not tail;**
  - `matrix.py` v4, every changed cell explained with its own replay;
  - `merge_kill_probe.py`, 300 kills: every stuck merge state is completable by `ref complete`, or classified and named.

| unit | what | budget (stop at ×2) |
|---|---|---:|
| U1 | §1: the ref log under §9.2, the classification table, its tests and controls | 60 min |
| U2 | §2: `ref complete`, the rule, K1, the entry points | 90 min |
| U3 | §2: K3's negative tests and controls, K4's failpoints and race | 90 min |
| U4 | §3: the rebuild, K1, its tests, controls, K4 | 90 min |
| U5 | §4 text; the probes, `matrix.py` and the merge kills on the release build | 60 min |

**Report:** `.git-exclude/review-request/rfc165-round-2-report-v1.md`, with each unit's start and end time.

**K7:** a second addendum on this round, or any finding that would change R4's rule or R5's rebuild, goes back to the
owner before the work continues.

**ACCEPTED and CLOSED 2026-10-03.** Item 0 `9b9ac7e7`; U1 `c9684990`; U2 `1b309136`; U3 `cb53e972`; U4 `2b7e2804`;
U5 `83a42498`. Reviews `rfc165-round-2-ci-fix-review-v1`, `-u1` … `-u4-review-v1`, `rfc165-round-2-review-v1`.
**RFC 165 is fully delivered.**
