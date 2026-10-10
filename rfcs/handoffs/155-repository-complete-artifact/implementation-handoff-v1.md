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
