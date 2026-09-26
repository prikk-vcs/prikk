# A torn tail is a prefix of one frame — RFC 160 F3, handoff v1

**Live 2026-09-27, and it is next.** The owner ruled F3 into 0.48.0 (*"Yes."*, 2026-09-27, answering whether F3, and F4
if its fix needs no format change, go into 0.48.0 before release prep). **Next after this:** the F4 design round, then
0.48.0 release prep.

**Read first:**
- `rfcs/accepted/160-costs-that-follow-the-store-and-lengths-read-from-disk.md` §8;
- your report `.git-exclude/review-request/rfc160-recurrence-guards-report-v1.md` §6 and §10 (F3);
- `.git-exclude/reviewed/rfc160-recurrence-guards-review-v1.md` §4.

## The defect

Every framed reader treats a partial frame as a **torn tail**, the harmless remnant of an interrupted append. It does
so even when a sound frame follows the partial one. The architect reproduced the worst case on 0.47.0 and on this
build (`/home/nabbisen/.pgtmp/arch-seal/wal_repair_probe.py`):
- two queued commits, a 702-byte WAL, the second record intact at byte 351;
- the first record's length set to 2⁶²;
- `verify` exits 0 and `doctor` exits 0;
- **`doctor --repair-wal-tail` truncates all 702 bytes, "preserved 0 record(s)".**

A repair command deletes an intact queued commit. The same misreading leaves `verify` and `doctor` silent on the object
index, the author-key index and an undecodable trust-policy snapshot: the four `OPEN` rows of your CLI matrix.

## 1. What lands

1. **One rule, in one place, used by every framed reader** that has a trailing-partial notion. Those are the files that
   already call `frame_resync::resync_to_next_magic`: `foundation/{container,index,generation}.rs`, `wal.rs`,
   `refs/{container,pointer_index}.rs`, `received/received_index.rs`, `author/author_key_index.rs` and
   `trust_index.rs` (both of its formats). **Say if you find another.**
   - **A torn tail is a prefix of one well-formed frame, and nothing else.** If a **sound** frame starts anywhere in the
     remainder (magic, valid header, and a body that passes its checksum; a magic match alone is not enough, since
     payloads can contain magic bytes), then the partial frame is **damage**.
   - Damage becomes a failed item at its offset, decoding resumes at that sound frame, and `trailing_partial_bytes`
     counts only a true tail after the last sound frame.
   - **When the evidence is ambiguous, the answer is "damage", never "tail".** Example: a sound frame embedded inside
     the payload of a genuinely torn frame. The cost of a false "damage" is a refused repair and a manual step; the cost
     of a false "tail" is lost data.
2. **A repair never removes a sound frame.**
   - `doctor --repair-wal-tail` truncates only a true torn tail, exactly as today. On damage it **refuses**, and leaves
     the file byte for byte as it was. It names the damaged offset and the number of sound records after it.
   - `--repair-index` rebuilds from containers read under the new rule, so sound frames after damage are indexed.
   - The ref log's automatic truncation on publication (`refs/publication.rs:86-97`) keeps its prefix-match guard and
     also obeys the rule: no truncation when the partial frame is damage.
3. **Fixed-width formats** (object index records, the generation file): a header whose length is not the constant is
   malformed, whatever follows, since a torn append leaves a correct header.
4. **`verify` and `doctor` report every such case** with a finding and a non-zero exit. The four `OPEN` rows of the CLI
   matrix become non-zero, and the `OPEN` list becomes empty. An undecodable trust-policy snapshot is reported by
   `verify` too; today only `trust maintainer list` refuses it.
5. **Messages and docs:**
   - `commit`'s refusal after damage says the record is damaged, and points to `doctor` for diagnosis, **not** to the
     truncating repair;
   - `doctor`'s output never recommends `--repair-wal-tail` for damage;
   - `durability-recovery.md` (its lines 52 and 152) and `troubleshooting.md` say what the repair does and does not
     remove.

**Not in this round:** any new repair verb or flag (if a manual way out is needed for the ambiguous case, report it and
the architect rules); F1, F2, F4.

## 2. Controls — landed, each shown red

1. **The WAL case, end to end:** two records, the first length set to 2⁶². `doctor --repair-wal-tail` refuses, and the
   WAL is **byte-identical** after. `verify` and `doctor` exit non-zero, naming the damaged record, and the second
   record still replays. **Perturb:** the old classification. The WAL is truncated, and the test goes red.
2. **Crash recovery unchanged:** a true torn tail (a prefix of the last frame) is still truncated by the repair, and
   still tolerated by readers, for the WAL, the object index and the ref log. **Perturb:** treat every partial frame as
   damage. It goes red.
3. **The ambiguous case:** a torn frame whose payload holds an embedded sound frame. The repair refuses, and nothing is
   deleted.
4. **Every framed reader, one table** (reuse P4's harness): a partial frame followed by a sound frame is a failed item,
   the sound frame decodes, and `trailing_partial_bytes` is 0. **Perturb:** the shared rule. Every row goes red.
5. **Fixed width:** an index or generation header with a non-constant length is malformed. **Perturb:** accept it. It
   goes red.
6. **The CLI matrix:** the four former `OPEN` rows exit non-zero with a finding, on sealed and unsealed repositories.
   **Perturb:** the rule. The rows return to exit 0 and the test goes red.
7. **Ref publication:** a damaged early ref-log record with sound records after it. A publish does not truncate.

## 3. Measurement — units and budgets, up front

| unit | what | budget (stop at ×2) |
|---|---|---:|
| G2 | the CLI matrix on this build and on 0.47.0 | 5 min |

This is a correctness round, and nothing else is measured.

## 4. CHANGELOG, in RFC 161's shape

- **`### Fixed — …`:** the data loss stated plainly. `doctor --repair-wal-tail` could delete intact queued commits
  after a damaged record. Give the affected versions.
- **`### Output changes`:** `verify` and `doctor` now exit non-zero, with a finding, on the four files where they exited
  0, and `commit`'s refusal text changes.

## 5. The report

`.git-exclude/review-request/torn-tail-is-one-frame-report-v1.md`:
- the gates on the exact final commit;
- the reader table: file, before, after;
- each control with its perturbation;
- G2 against 0.47.0;
- the ambiguous cases you found, and how each resolves;
- anything here that is not true at source.

**Windows:** the architect pushes, and the round closes on a green Windows mutation suite.

## Addendum 1 — 2026-09-27: keep every removed byte, accept the phantom, and an interrupted append is not damage

Report: `.git-exclude/review-request/torn-tail-is-one-frame-report-v1.md`; review
`.git-exclude/reviewed/torn-tail-is-one-frame-review-v1.md`. **What you delivered is accepted.** The architect re-ran the
gates (14/14 on `77d2d0d1`), reproduced the refusal with the WAL byte-identical, and perturbed the rule under a cap.
**Nothing is pushed until this addendum lands.**

1. **The WAL repair keeps every byte it removes** (your §7.1: neither (b) nor (c)).
   - Before `doctor --repair-wal-tail` truncates anything, it writes exactly the bytes it will remove to a recovery file
     under `.prikk/`. Choose the name; it names the session and the offset, and it is **not** `quarantine/`, which is
     retired.
   - The write is durable (file and directory synced), under the same lock, and **only then** comes the truncation. The
     output names the file.
   - `verify` ignores the file; `repository-layout.md` lists it as never authority; `durability-recovery.md` says how to
     look at it.
   - This covers the lone damaged record and the true torn tail alike.
   - **Controls:**
     - a lone damaged WAL record: the repair truncates it, and the recovery file holds **exactly** the removed bytes,
       byte for byte;
     - a true torn tail: the same property;
     - a failpoint between the save and the truncation leaves the WAL untouched and the recovery file complete;
     - **perturb:** truncate without saving. It goes red.
2. **The phantom object from `--repair-index` over an embedded frame: accepted** (your §7.2). One sentence in the
   recovery reference: what it is, that nothing references it, and that it changes no state.
3. **An interrupted append followed by later writes is not damage when nothing names it** (your §7.3, widened). The
   architect measured it (`/home/nabbisen/.pgtmp/arch-seal/torn_then_append_probe.py`): a crash-torn blob frame, with
   later commits appended past it.
   - **On 0.47.0 `verify` already exits 1 for good** when the later bytes run past the torn frame's claim ("container
     checksum mismatch", and no repair).
   - When they fall short, 0.47.0 silently skipped the later objects ("1 scanned").
   - **The rule:**
     - **object containers:** a frame that fails to parse (a partial before a sound frame, **or** a checksum or shape
       failure) **and that no index entry names** is an **interrupted append**. `verify` and `doctor` report it as a
       warning line (offset, and that nothing references it), and the stage does not fail. **A frame an index entry
       names stays `Failed`** (RFC 160 §7, unchanged);
     - **the WAL:** no change. Append refuses past a tail, so an interior partial frame is always damage;
     - **the ref log:** determine what durably names a ref-log record (the pointer index, a ref's resolved tip), and
       apply the same principle. If it cannot be decided safely, keep `Failed` and report why. **Never silence a record
       that something names.**
   - **Controls:**
     - both of the probe's shapes: `verify` exits 0 with the warning, and **every** later object is scanned and read;
     - the same torn frame, but named by an index entry: `Failed`, exit 1;
     - **perturb:** treat an unindexed frame as failed. The first control goes red. Treat an indexed one as
       unindexed. The second goes red.

**Units:** G2 re-run, budget 5 min; nothing else is measured. **CHANGELOG (RFC 161 shape):**
- `### Output changes`: `verify` exits 0, with a warning, on a repository whose only fault is an interrupted append;
  and the repair now writes a recovery file;
- `### Fixed`: the crash-then-write false failure, with 0.47.0 named as affected.

**Run every perturbation under an address-space cap and a timeout**, as you now do. Gates on the exact final commit.
Report: `.git-exclude/review-request/torn-tail-is-one-frame-report-v2.md`.

## Addendum 2 — 2026-09-27: the behavior is accepted; the public additions must not break callers silently

Report v2 and review `.git-exclude/reviewed/torn-tail-is-one-frame-review-v2.md`. **The behavior is accepted.** The
architect reproduced the recovery file (exactly the removed bytes), both interrupted-append shapes (exit 0, every
object scanned) and the refusal (byte-identical), and perturbed the save under a cap. **Your §6 rulings:**
- **ref log:** `Failed` for 0.48.0, and your rule goes to 0.49.0 with F1;
- **`recovery/` on demand:** accepted as the one exception;
- **the public additions:** below.

1. **`InterruptedAppend` is `#[non_exhaustive]`** (the release-prep template, item 4: every new report type).
2. **`RepositoryVerification` and `WalRepair` become `#[non_exhaustive]` in this round.** They gained public fields,
   which already breaks struct-literal construction and exhaustive destructuring outside the crate. Making them
   non-exhaustive now means the next field does not break anyone again (0.44.0 did the same for `BundleImportReport` and
   `AcceptReport`). Fix any in-workspace construction that needs it; report what changed.
3. **CHANGELOG, in RFC 161's spelling:** move the two field additions and the new type out of Output changes into
   `### Changed — breaking once: RepositoryVerification and WalRepair gain fields and are now #[non_exhaustive]` (a short
   paragraph), with one line under `### Upgrading` (construct them only through the crate; match with `..`).
4. **The Windows proof for `recovery/`:** name, in the report, the CLI test that exercises the recovery file. The
   architect reads it green by name in the Windows mutation suite before the round closes.

5. **Run your gates, tests and perturbations in their own cgroup scope, from now on** (RFC 160 §9, R1): `systemd-run --user
   --scope -p MemoryMax=32G -p MemorySwapMax=0 timeout <limit> …`. A runaway is then killed inside its scope, never the
   terminal. The architect proved it on this machine: exit 137, and the parent survives. The address-space cap you use
   for perturbations stays as well. Say in the report that the gate run used the scope.

Gates on the exact final commit. Report: `.git-exclude/review-request/torn-tail-is-one-frame-report-v3.md`. Then the
architect pushes the round.

**ACCEPTED 2026-09-27** (`a0d8e548` … `03fdf238`; reviews `torn-tail-is-one-frame-review-v1`, `-v2`, `-v3`; 14/14 gates
re-run by the architect on `b2e8e4cf` in a capped cgroup scope, 2414 / 0 / 54). **Item 5 of Addendum 2 was added after the
team had delivered.** It is carried in review v3, where they will read it. **Closes when the Windows mutation suite is
green on the pushed commit**, reading `a_repair_of_a_lone_damaged_record_saves_the_record_byte_for_byte` and
`a_true_torn_wal_tail_is_still_tolerated_and_still_truncated_by_the_repair` by name.
