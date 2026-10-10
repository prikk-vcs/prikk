# RFC 155 — design round: the repository-complete artifact

**Live 2026-10-10, and it is next** (0.51.0 step 2; the owner approved the plan: *"Approved."*, 2026-10-10).

## Task title and purpose

RFC 155 (accepted 2026-09-16) sets the direction: one file holds a whole repository verbatim (`PREPO001`). Export is
read-only and consistent. Import is all-or-nothing, even when the process is killed. Trust material travels as inert
data.

**This round answers how, from source and from small prototypes, with no product code.** It ends with a design the
owner reads and accepts before any implementation.

## The working rule

- **Two parts, one sitting each,** each with one final report.
- **No file in `.git-exclude/review-request/` until a part is complete.** If you must stop, end with *"Continuing — not
  ready for review"*.
- **Prototypes live in a scratch worktree, never on `main`.** Every run goes inside an R1 scope.
- **Every rule that decides a state from files gets its case table first:** the rows, each with its expected answer,
  and a control per alternative.

## Read first

- `rfcs/accepted/155-repository-complete-artifact.md`, all of it;
- RFC 156 §4 (the same id under different signatures is merged);
- RFC 154 (adoption);
- RFC 114 §5.2 (carry-forward);
- RFC 115 §3 (no repository-wide snapshot) and §6;
- DC-78 (bundle export, verify and import);
- RFC 168 (the recovery log and runs).

## Part D1 — facts, the format, export and verify (90 min, report `rfc155-design-round-D1-report.md`)

1. **What a repository holds, from source:**
   - every file under `.prikk/` and whether it belongs in the artifact: objects (all containers), refs (pointer index
     and ref log), received pointers, author keys, the trust policy and keys, the generation logs, the WAL and the
     witness, caches, the recovery log;
   - **for each:** carried, carried as inert data, or derived on import. Give the reason. Anything derived is rebuilt
     by the importer, never trusted from the file.
2. **The format, `PREPO001`:**
   - a layout that can be **streamed** on both ends, with memory bounded independently of the repository's size;
   - its sections; its framing, reusing the container frame where it fits; its checksums;
   - a manifest that lets `verify` check closure without random access, or an argued alternative;
   - the RFC 114 carry-forward test: what the format-7 build reads.
3. **The consistent read for export (R4, RFC 115 §3).** What view, under which lock, so that export taken during a
   concurrent `seal` yields the repository before or after the seal, never a mix?
   - Measure how long the lock is held at the release-gate corpus sizes.
   - If holding the lock for a whole export is too long, propose the alternative, with its case table.
4. **`verify` on the artifact (R2):** structure, closure, and every signature against the key material the artifact
   carries. It reports *internally consistent*, never *trusted*; a signature it cannot check is reported, not skipped.
5. **A prototype export and verify** in scratch, measured at the release-gate sizes: wall time and peak RSS against
   object count. That shows the bound, not just a claim of one.

## Part D2 — import, landing and identity (90 min, report `rfc155-design-round-D2-report.md`)

1. **All-or-nothing import, under a kill (R4).** Propose the mechanism: stage then commit, a journal, a recovery run,
   or other.
   - **The case table:**
     - an empty target, and a non-empty target;
     - a kill at each write stage: before the first write, mid-objects, after the objects and before the refs, mid-refs,
       after the refs and before the commit point, after the commit point;
     - a second import after each kill;
     - `doctor` in each interrupted state;
     - a concurrent writer;
     - disk full.
   - **Each row:** the target's state, untouched or complete; what `doctor` names; the way out.
2. **Landing (§5).**
   - **The default** is every ref under `remotes/`.
   - **`--adopt` needs RFC 154's adoption rule, which is not implemented yet** (RFC 154 follows 155). State what
     `--adopt` needs from RFC 154 and the smallest correct piece of it, then lay out the options:
     - ship `--adopt` in 0.51.0, with that piece;
     - ship landing under `remotes/` only, and `--adopt` with RFC 154.
   - **For each:** the carry-forward story (§5's three steps) and its risk. **Choose none:** the architect rules.
3. **Identity (R6 as superseded by RFC 156).**
   - **The same id with identical bytes:** a no-op, reported.
   - **The same id with different signatures:** RFC 156 §4's merge, or a refusal?
     - **RFC 155 §8 still says "refused and named",** while R6's own note says merged. Lay out both readings against
       RFC 156 §4's actual rules, from source. Choose none.
   - **Re-importing the same artifact** is a no-op, said so.
4. **Trust material (R1, §7.2):** how the inert trust record is stored and listed after import, and the exact command
   an operator uses to adopt from it. Import never changes the adopted set.
5. **`--format json` (R5):** every verb's outcome fields.
6. **The implementation parts:** a proposed split into one-sitting parts (export, verify, import, landing), each with
   its tests and RFC 155 §8's controls. **If import alone needs more than two sittings, say so plainly:** the owner may
   split the release.

## Explicit non-change scope

- No product code on `main` in this round.
- No change to `bundle`, `sync`, or any existing format.

## Prohibited shortcuts

- A design that holds the whole artifact in memory.
- An import that loops `bundle import` per ref; RFC 155 R4 rules that out.
- *"Atomic"* without a case table behind it.

## Required evidence

- **Per part:** the facts with code references, the prototype's measurements (the build profile named), the case
  tables, and the options laid out without a choice where this handoff says *"choose none"*.

**Part D1 ACCEPTED 2026-10-10** (review `rfc155-design-round-D1-review-v1`). **D2 gains six items,** detailed in that
review:
1. Author keys follow DC-78's recording, with conflicts refused; they are not inert. Only maintainer material is
   inert. State it from source.
2. Unsealed work in an active session: the facts, then options (refuse and name `seal`; warn; carry inert), with
   consequences. Choose none.
3. Export refuses in a state it cannot resolve (an ambiguous lost log, a damaged slot, a torn tail).
4. Locks fail fast:
   - (a) measure export on at least 1 GiB of blobs;
   - (b) a killed export's lock files;
   - (c) the option to lock briefly, copy the slotted containers, record object-container lengths, release, then
     stream. Its case table, and choose none.
5. Verify's staging: its location, disk cost, and cleanup on a kill.
6. The prototype exports a repository whose live slot is `b`.

**Not a bigger D2 (one sitting is 60–90 minutes): a third part, D3.** D2 stays as written (90 min,
`rfc155-design-round-D2-report.md`). **D3** takes items 1–6 above (90 min, `rfc155-design-round-D3-report.md`), after D2.
The design goes to the owner after D3.

**Part D2 ACCEPTED 2026-10-10** as design input (review `rfc155-design-round-D2-review-v1`).
- **Ruled:** the carried maintainer record is listed from the artifact, never stored in the repository and never given
  a new verb. The operator adopts with `trust maintainer add`.
- **A fourth part, D4 (60 min, `rfc155-design-round-D4-report.md`), after D3:**
  - **7.** The intent journal comes before the first object write, so `doctor` names every interrupted stage.
  - **8.** "Untouched", strict (hold the object lock; record container lengths; a cancel cuts back with a recovery
    save) or relaxed (unreferenced objects remain and are named). A case table for each; choose none.
  - **10.** The command surface, without overloading `prikk verify [path]`. Two or three namings; choose none.

**Next: D3, then D4.**
