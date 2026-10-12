# RFC 155 — implementation: `prikk archive`

**Live 2026-10-10, and it is next** (0.51.0 step 3). The owner accepted RFC 155 §9 and the noun `archive`: *"Both
accepted."* (2026-10-10).

## Task title and purpose

Build `prikk archive export|verify|import` and `prikk doctor --cancel-import`, exactly as RFC 155 §9 designs them. **§9 is
the specification; read it in full first.** Where this handoff and §9 differ, §9 wins: stop and report.

## The working rule

- **Four parts, one sitting each (90 min, stop at 2×),** in order, each with one final report listing every item with
  its status.
- **No file in `.git-exclude/review-request/` until a part is complete.** If you must stop, end with *"Continuing — not
  ready for review"*.
- **`date` at each part's start and end. Every run inside an R1 scope.**
- **Every rule that decides a state from files gets its case table first.**
- **Every control is perturbed, run red, and reverted, with its failure line in the report.** "The test is the
  control" is not one.
- **Every message is tested by following it.**

## Security, for every part: an archive is untrusted input

- **A section's name is never used as a filesystem path.** The importer and verifier map each section to a **fixed
  allow-list** of section kinds (object containers by type, the three ref families, author keys, maintainer record).
  An unknown, duplicated or missing section is a refusal naming it.
- **Every length read from the archive is checked before allocation or copy:** the DC-86 discipline, through
  `read_bounded_object_frame`'s precedent.
- **Every refusal writes nothing,** whether in stage 0 or in verify.

## Part E — export (report `rfc155-impl-E-report.md`)

1. **The `PREPO001` writer** (§9.2):
   - a file magic and version;
   - one section per carried content kind, in the container frame;
   - a trailing manifest and a fixed end trailer;
   - streamed, with memory independent of size.

   Document the byte layout in a new `docs/src/reference/archive-format.md`.
2. **The full lock** (§9.3): `acquire_container_locks` with all five, held for the whole run.
3. **The refusals before any write** (§9.1):
   - queued, unsealed commits in **any** session (name the session, the count, `prikk seal`);
   - an unresolvable slotted container (the existing texts, through `resolve_or_deduce`).
4. **The resolved content of the slotted families** (no slot letters, no generation log).
5. **`prikk archive export <file> [--format json]`** (`prepo-export-report-v1`). `--help`, and `commands.md`.

**Tests:**
- §8's concurrent `seal` (before or after, never a mix);
- **a repository whose live slot is `b`**, which carries the live content (D3's need, closed here);
- each refusal, followed by the command it names, then export succeeding;
- RSS bounded, measured at the release-gate sizes and at 1 GiB of blobs (release build, idle machine, sampled at least
  3 times).

**Controls:**
- read slot `a` unconditionally (the slot-`b` test goes red);
- drop the queued-work check;
- release a lock early (the concurrent-`seal` test goes red).

## Part V — verify (report `rfc155-impl-V-report.md`)

1. **The streaming reader** (§9.4): structure from the trailer and manifest; closure in two passes with an id set.
2. **Every signature against the archive's carried material,** held in memory, through
   `verify_author_signatures_with`'s lookup closure for authors and the maintainer path for maintainer signatures.
   Every signature it cannot check is named.
3. **The output:**
   - *"internally consistent"*, never *"trusted"*;
   - the carried maintainer keys listed as *"carried, not adopted"*;
   - `--format json` (`prepo-verify-report-v1`) with structure, closure and signatures as separate fields.
4. **It writes nothing, on any exit path.**
5. **A fuzz target for the decoder** (trailer, manifest, section frames), beside the existing ones: it never panics, its
   work is bounded, and damage is reported, never accepted.

**Tests:**
- §8's flipped byte in any envelope, which names the object;
- a truncated archive;
- an unknown section kind;
- a duplicated section;
- a section path like `../x`, refused by kind and never touched as a path;
- the maintainer-key listing;
- a hash of a fixture directory before and after.

**Controls:**
- skip the closure pass (a dangling-reference archive goes green: red);
- trust a section name as a path (the traversal test goes red).

## Part I-A — import, stages 0 to 2, the journal and the cancel (report `rfc155-impl-IA-report.md`)

1. **Stage 0:** verify (Part V's code, not a copy), the author-key conflict check (`check_author_key_conflict`), the
   landing plan. A refusal writes nothing.
2. **The journal, the first write** (§9.5): the archive's identity, the plan, and the length of every appendable
   container. Every container lock is held from the journal to the commit point.
3. **Stage 1:** objects in dependency order, through the ordinary write path. **Stage 2:** author keys through
   `record_author_key_material`.
4. **`doctor`:** `PRIKK-DOCTOR-INTERRUPTED-IMPORT`, with *N of M refs landed*, naming both ways out.
5. **`prikk doctor --cancel-import <id> [--plan-only]`:** for every container the journal names, save the bytes past the
   journaled length (one RFC 168 run, before any truncate), truncate, and empty the journal.
6. **While a journal is present, every repository writer refuses,** except the import itself, the cancel and the
   recovery verbs, naming both ways out. Enumerate the writers from source, as RFC 163 did.

**The case table:** a kill before the journal; a kill right after it; a kill mid-objects; a kill after the objects;
then a resume from each; a cancel from each; a full disk.

**Tests:** each row; §8's *"the adopted set is unchanged"*; every writer refusing behind a journal; the cancel, then
`--recovery-restore` of its run.

**Controls:**
- write the journal after the objects (the kill-mid-objects row goes red: `doctor` names nothing);
- drop the writer refusal;
- truncate without the save.

## Part I-B — import, stage 3: landing, `--adopt`, identity (report `rfc155-impl-IB-report.md`)

1. **Stage 3:** each landing written idempotently. **The commit point:** the journal is emptied, and its name kept.
2. **The default landing:** `remotes/`.
3. **`--adopt`** (§9.6) lands a ref locally only if:
   - it is absent locally;
   - it is signed by an adopted key;
   - it is verified;
   - its CAS against *"absent"* holds.

   Every other ref goes to `remotes/`, with the reason named.
4. **Identity** (§9.7): RFC 156's merge through `admit_carried_signatures`, with dropped signatures named. Re-import
   reports *"already imported"*.
5. **`prikk archive import <file> [--adopt] [--format json]`** (`prepo-import-report-v1`).

**Tests:**
- §8's remaining controls: a non-adopted key lands in `remotes/`; re-import is a no-op, reported; the same id signed
  differently is merged, with a dropped signature named;
- **the round trip,** where `branch list` and the object ids are identical;
- **the carry-forward story end to end:** export from repository 1; `init` repository 2; verify lists the maintainer
  keys; `trust maintainer add`; `import --adopt`; every branch is a branch; verify exits 0.
- **Smoke:** a new section that runs the carry-forward story on the release build.

**Controls:**
- adopt without the adopted-key check (a non-adopted ref becomes a branch: red);
- adopt over an existing ref.

## For every part

- **The docs-debt grep:** `backup-restore.md`'s *"a restore gives `remotes/<ref>`"* and every page that says a whole
  repository cannot be carried.
- **CHANGELOG `## Unreleased`:** an `Added` entry per part, and Output-changes lines.
- **`scripts/gates.py`:** all 14, on the part's last commit. **`size-check`:** a new large file is declared, not
  hidden.

## Explicit non-change scope

- No change to `bundle`, `sync`, or the repository format.
- No RFC 154 beyond §9.6's narrowed `--adopt`.
- The `bundle import` memory finding is not fixed here.

## Prohibited shortcuts

- Holding the whole archive or every object in memory.
- Looping `bundle import` per ref.
- A second copy of verify's checks inside import.
- Building a test state by writing container bytes, when commands can build it.

**Part E ACCEPTED 2026-10-10** (`c61e6d4c`; review `rfc155-impl-E-review-v1`). **Part V gains one item:** export's report names objects by type and refs (local, received, tags), in text and JSON, rather than "sections: 13". **Next: Part V.**

**Part V approved in part 2026-10-11** (`365c3383`; review `rfc155-impl-V-review-v1`).

| part | items | budget | report |
|---|---|---:|---|
| **V2: the manifest, the fuzz property, the note** | **V1:** a checksum over every manifest byte, found from the trailer and checked before use; refuse an unknown `repository_format`, naming it; update `archive-format.md`. **V2:** find from source why 20,000 fuzz cases never caught a manifest flip that passes as consistent; show the property red on it before the fix and green after. **V3:** the *"internally consistent"* note (text and JSON) only when consistent. Controls: drop the manifest check; accept any format. Docs, CHANGELOG, 14 gates | 45 min | `rfc155-impl-V2-report.md` |

**Next: V2, then Import A.**

**V2 ACCEPTED, Part V CLOSED 2026-10-11** (`458c6c5f`; review `rfc155-impl-V2-review-v1`). **Next: Import A.**

**Part I-A approved in part 2026-10-11** (`e2f8a608`; review `rfc155-impl-IA-review-v1`). RFC 155 §9.5's journaled list is corrected: it now includes the object index and the author-key container.

| part | items | budget | report |
|---|---|---:|---|
| **I-A2: the lock held, the author keys, the open rows** | **IA1:** hold `ObjectStore` from the journal to the commit point, through an append that runs under a held lock, and refuse behind a journal for **every** session; a concurrent append test, plus a control. **IA2:** journal and cut back the author-key container (a test: import, cancel, then a different key for the same id succeeds). **IA3:** a kill mid-objects (failpoint), an injected I/O error (full disk), a cancel right after the journal; resume and byte-identical cancel for each. **IA4:** the adopted set unchanged, tested. Docs, CHANGELOG, 14 gates | 60 min | `rfc155-impl-IA2-report.md` |

**Next: I-A2, then I-B.**

**I-A2 ACCEPTED, Import A CLOSED 2026-10-11** (`516d260b`; review `rfc155-impl-IA2-review-v1`). **Next: I-B.** Where received refs land (from the I-A review): the archive's received pointers stay received under their own names; its local refs land under `remotes/<ref>`, or locally with `--adopt`.

**Part I-B approved in part 2026-10-11** (`a4d3c867`; review `rfc155-impl-IB-review-v1`).

| part | items | budget | report |
|---|---|---:|---|
| **I-B2: no writer slips past, and a real kill** | **IB1(a):** hold `RefPointerIndex`, `RefLog` and `ReceivedIndex` from the journal to the commit point, through under-held-lock publish and received-pointer writes. **IB1(b):** `acquire_container_locks` also refuses while a journal is present, with the narrow exemptions; enumerate the newly refused callers. `compact` and `--rebuild-pointer-index` take only container locks, so they currently bypass the journal refusal. Tests: compact and rebuild refuse behind a journal; a concurrent publish refuses. Controls run red. **IB2:** a CLI test with a real `SIGKILL` mid-objects; follow `doctor` to resume and to a byte-identical cancel; if stale locks block the way, `doctor` names them and `prikk unlock` first (no auto-steal); docs. CHANGELOG, 14 gates, release smoke | 90 min | `rfc155-impl-IB2-report.md` |

**Next: I-B2. RFC 155's implementation closes after it.**

**I-B2 ACCEPTED, and RFC 155's implementation CLOSED, 2026-10-11** (`f26ec154`; review `rfc155-impl-IB2-review-v1`). Ruled: `--repair-index` and `--repair-pointer-index-tail` refusing behind a journal is correct (each rewrites a journaled file; `--repair-tails` stays the exempt way out). **Next: 0.51.0 release prep.**

**External review 023 (2026-10-12): one finding blocks the tag** (assessment `external-review-023-assessment-v1`).
RFC 155 §9.6 is amended (the collision rule), and §9.2's trailer text is corrected.

| part | items | budget | report |
|---|---|---:|---|
| **I-C: the landing collision, and three small items** | **1.** The landing plan: each ref planned once, from its latest entry. On a collision between an exporter's own ref landing at `remotes/heads/X` and a carried received pointer `remotes/heads/X`, **the exporter's ref wins**; the received pointer is not landed and is named in the outcome (text and JSON) with its target, and its objects are still imported. Tests: 023's shape (a source that imported a bundle for its own `heads/main`), by default and with `--adopt` both succeeding and failing; `branch list` shows the exporter's target; the control (received written last) goes red. **2.** Any tail inside a section is damage: `verify` reports it and `import` refuses (an honest export ends each section at its sound length); test plus control. **3.** The section-count bound gets its own error text. **4.** A default import without the signer adopted says that `verify` fails on publication trust until `trust maintainer add` (or `--adopt`). Docs, CHANGELOG (the collision belongs under Fixed, since it never shipped; Output changes if needed), 14 gates, and the release smoke | 60 min | `rfc155-impl-IC-report.md` |

**Next: I-C.** Then a P3-style re-run (gates, matrix v6, `reproduce.sh`) on the new candidate, and letter 024.

**I-C approved in part 2026-10-12** (`721c364e`; review `rfc155-impl-IC-review-v1`). **Item 1, the collision, is
accepted.** Items 2–4 are not, and the review found one older defect that blocks the tag (N1).

RFC 155 §9.1 and §9.4 are amended (2026-10-12). **The rule for both parts: the archive classifies as the source
does.** Damage the source refuses, the archive refuses; what the source only warns about or leaves to trust, the
archive names.

| part | items | budget (stop at ×2) | report |
|---|---|---:|---|
| **I-D: classify as the source does** | **N1, signatures.** A signature by a key the archive does not carry is named (*"not checked: signed by a key this archive does not carry"*) and leaves the archive consistent. A signature by a carried key that does not verify is damage. Applies to maintainer and author signatures alike. The store already separates the cases (`…-UNTRUSTED` vs `…-INVALID-SIGNATURE`; `Ok(Some((_, false)))` vs `Err`). **Tests,** from real repositories: a source `bundle import`s a bundle signed by another key, untrusted, at a non-colliding name (`heads/other`). Then: `archive verify` exits 0 with the named lines; a default import succeeds; the destination's `prikk verify` reports the same `PRIKK-TRUST-PUBLICATION-UNTRUSTED` the source does; after `trust maintainer add` of that key it exits 0; a destination that already trusts it exits 0 at once; a flipped byte in a carried-key signature still refuses. Control: the not-carried case put back among the refusals goes red. **Item 2, redone.** Classify each object-container section's frames with the store's own rule (`foundation::container`, `ContainerRecordStatus::Failed { complete }`), **reused and streamed**: memory bounded by the largest frame, never the section. A complete frame that fails is damage, named by section and offset, and import refuses. Incomplete frames are named with their byte count; they never make the archive inconsistent and are never imported; import's report names them too. Remove *"an honest export never produces this"* from code, docs and CHANGELOG. **The test matrix.** For each shape: build it in a real source, record that source's own `prikk verify` outcome, export, and assert the archive agrees (a warning means named, the import proceeds and the destination's `verify` exits 0; an error means damage and the import refuses). Shapes: 16 bytes; 49 zero bytes; **100 zero bytes (023's shape)**; 30 text bytes; a complete last frame with one body byte flipped; a complete middle frame flipped with sound frames after it; a 16 MiB resync tail. Plus the twin: I-C's crafted tail (checksum recomputed) gets the same outcome as the same bytes written by a crash. Control: the old skip of an unsound frame restored, and the 100-byte and complete-flip rows go red. Prohibited: buffering a section whole, or a second copy of the store's classification. Docs (`archive-format.md`, `backup-restore.md`), CHANGELOG, 14 gates, release smoke | 90 min | `rfc155-impl-ID-report.md` |
| **I-E: the note, the count text, the manifest checksums** | **4.** The note is printed only for landed refs whose tip has no maintainer signature **this** repository trusts (its own policy, through `verify_trusted_publication_envelope`). It names each such ref, and gives the `--adopt` hint only when one of them is the exporter's own ref. CLI tests, text and JSON, for the review's three rows: untrusted prints the note; trusted without `--adopt` prints none; trusted with `--adopt` prints none. **3.** The count text names its subject, e.g. *"the archive manifest declares 20000 sections, over the bound of 10000"*; a test asserts the whole message. **The aside.** Each section's trailer checksum must equal its manifest entry, or it is damage. Test: flip one manifest entry's checksum and recompute the trailer's manifest checksum. Plus a control. **Small:** drop *"(RFC 155 §9.6)"* from the collision reason; name `CountOverBound` in `### Upgrading`. Docs, CHANGELOG, 14 gates, release smoke | 45 min | `rfc155-impl-IE-report.md` |

**Next: I-D, then I-E.** Then the P3-style re-run (gates, matrix v6, `reproduce.sh`) on the new candidate, and letter
024.

**I-D approved in part 2026-10-12** (`2c6a62ac`; review `rfc155-impl-ID-review-v1`). **N1 is accepted.** Item 2 is
not: the archive walk has no resync. A torn frame overrun by a later commit, which the source's own `prikk verify`
only warns about, makes the archive refused, and the objects after it are never found. **Ruled:** a streamed
re-derivation is allowed for 0.51.0 only when a differential test holds it equal to the store's decoder. One shared
primitive is 0.52.0 work.

| part | items | budget (stop at ×2) | report |
|---|---|---:|---|
| **I-D2: resync, held equal to the store** | **1. The whole rule, streamed.** After any frame that is not sound (a short read, a bad magic or version, a checksum mismatch), scan forward for the next sound frame within the store's budget semantics (exhaustion means damage, RFC 167 C3), and continue decoding from it, in verify **and** stage 1. A checksum mismatch over a fully present body is an interrupted append when a later sound frame starts inside its claimed range (`checksum_mismatch_is_complete`), and damage otherwise. Memory stays bounded by one frame plus a scan window, never the section. **2. The differential test, the gate for item 1.** A test-only harness runs `foundation::container::decode_container_records` over a section's bytes in memory and the archive walk over the same section, and asserts they agree on: the sound frames, by offset and object id; the offsets of complete failures; and the offsets of interrupted appends. Run it on: every I-D shape; **the review's overrun shape** (a real header plus 10 body bytes, then a later commit); a torn frame whose claimed length runs past the section's end, with sound frames after it; a bad magic in the middle with sound frames after it; a checksum mismatch whose next sound frame starts after its claimed end (damage, then decoding continues); a budget-exhausting run. **Plus a proptest:** random flips, truncations, insertions and inserted torn frames over a real multi-record container. Control: drop the resync and the overrun shape goes red in the differential test. **3. CLI test of the overrun shape:** the source's `prikk verify` exits 0; `archive verify` is consistent with an interrupted append named; the import succeeds; the destination reads the second file and its `verify` exits 0. **4. Text:** the CHANGELOG must not say *"reused, not re-derived"*; say the archive applies the store's rule, held equal by a test. Remove review history from user-facing text (*"N1"*, *"the architect's own correction"*, *"external review 023's own"*). `archive-format.md` defines *complete* with the overrun test. Docs, CHANGELOG, 14 gates, release smoke | 90 min | `rfc155-impl-ID2-report.md` |

**Next: I-D2, then I-E as written.** Then the P3-style re-run and letter 024.

**I-D2 approved in part 2026-10-12** (`91e46d3d`; review `rfc155-impl-ID2-review-v1`). The resync is right, and the
overrun shape is fixed (verified at the CLI). One misordered check remains, and the second proptest's restriction hid
it. **I-E gains item 0. Its budget becomes 75 min.**

| part | items | budget (stop at ×2) | report |
|---|---|---:|---|
| **I-E item 0: the last differential gap** | **(a)** In `parse_frame_at_streamed`, check *"claimed body past the section's end"* (→ `TrailingPartial`) **before** the per-object bound, as `parse_frame_at_reporting` does. The bound then guards only bodies that fit. **(b)** Remove the restriction from `proptest_torn_frame_shaped_insertion_agrees_with_the_store`: any insertion offset, `claimed_len` over all of `u64`. Keep its committed default case count, and run 5,000 cases once and report it. Add the review's minimal case (offset 10, `claimed_len = 0`) as a named shape test. **(c)** The harness names its one allowed exception exactly: a frame whose body fits the section but exceeds the bound. Any other `ObjectOverBound` fails the test. **(d)** A real control: disable the resync in the archive walk, and the overrun shape plus the proptest go red. Report the failure line, then revert. **(e)** A CLI test of the overrun shape (the handoff's I-D2 item 3, as the review ran it): source `verify` exits 0, `archive verify` exits 0, import succeeds, the second file reads back. **(f)** An interrupted append is printed as one (*"interrupted append at byte offset N … not imported"*), never as *"failed its own checksum"* | inside I-E | `rfc155-impl-IE-report.md` |

**Next: I-E (item 0, then items 4, 3, the aside and the small items), 75 min.** Then the P3-style re-run and letter
024.

**I-E approved in part 2026-10-12** (`c3af85f3`; review `rfc155-impl-IE-review-v1`). Item 0, item 3, the manifest
checksum and the small items are accepted, and so is 0(a)'s hard refusal of an over-bound candidate that fits the
section. Two things remain: the trust note misses a collided received pointer whose objects `verify` still reports,
and the 0.51.0 notes describe the archive's pre-release defects as if they had shipped (the architect's I-C wording).

| part | items | budget (stop at ×2) | report |
|---|---|---:|---|
| **I-F: the note says what `verify` will say; the notes read as one feature** | **1. The note.** Name every planned entry whose tip has no maintainer signature this repository trusts, **landed or not**. A not-landed (collided) entry says its history was imported without a ref, because the exporter's ref holds the name. Each line names the signing key id. Singular and plural follow the count. The `--adopt` hint appears only for the exporter's own refs. Compute `publication_trusted` for collided entries too. CLI tests, text and JSON, for the review's five rows: r1 names both; **r2 (trusts only the exporter, no `--adopt`) names the collided pointer**; r3 names the received pointer; r4 and r5 print nothing. **2. The 0.51.0 notes.** Remove `### Fixed — archive import could silently drop a ref…` and the three archive `### Output changes` bullets (landing fields, the count text, interrupted-append lines). Fold the final behaviour into the `archive` Added entry, briefly: the collision rule; interrupted appends named, never refused; a complete damaged frame refuses; a signature by a key not carried is named; the trust note; each section bound to its manifest entry. The step-1 Fixed entry stays. **3. The three wording fixes held for the tag** (review `release-0.51.0-prep-P3b-review-v1`): *"because the tail guard ran first"* in the `ref complete` line; *"nothing is damaged"* in the `compact --pointer-index` line; *"external reviews 020 and 021"*, and `VERIFY-OK` added to the step-1 Fixed entry (six). After this, the date commit at the tag changes only the date. `release-policy-check` and `reference-check` clean; 14 gates; release smoke | 60 min | `rfc155-impl-IF-report.md` |

**Next: I-F. Then P4** in `rfcs/handoffs/119-release-policy-reset/release-0.51.0-prep-handoff-v1.md`.
