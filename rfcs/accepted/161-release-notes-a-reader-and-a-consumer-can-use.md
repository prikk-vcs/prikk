# RFC 161 — Release notes a reader and a consumer can use

**Status.** **ACCEPTED by the project owner 2026-09-26** (*"Yes. Accepted."*). The question was: *"shall I write this up
as a proposed RFC, with the 0.48.0 part scoped for your acceptance?"*, following the architect's findings on release
notes and changelogs. **The architect's reading, stated so it can be corrected:**
- the direction below is accepted as proposed: §3's section shape, §4's checks and §5's archive;
- **the 0.48.0 part (§6.1) applies from 0.48.0 release prep**;
- the checks and the archive move (§6.2) are a later round, scheduled by the architect and not before 0.48.0 ships;
- nothing here is adopted because another project has it. The reference was peisear's
  `docs/src/development/changelog-and-releases.md`, and each rule below stands on this project's own evidence (§1).

Author-review independence: the architect proposes and reviews. The mechanical checks of §4 are each shown red by
breaking the rule they check.

## 1. What is right today, and what is not

**Right, and kept:**
- **One source.** `CHANGELOG.md` holds the release notes.
- **The GitHub Release page is generated, never hand-written.** `tools/release-policy` assembles it from the CHANGELOG
  section, the published build-info files and an integrity statement, and refuses to describe an asset that is not
  there (RFC 107).
- **Every released tag keeps its dated heading** (RFC 127's gate).
- **The Release page stays.** prikk ships signed binaries, and that page is where the 16 assets live, so peisear's "no
  Release page" rule does not fit here.

**Not right:**
1. **Security fixes are not labelled as security.** 0.44.0 fixed the refused-import defect that advisory
   GHSA-px5q-233r-6hq5 describes. Its entry is `### Fixed — a refused bundle import or sync accept now changes
   nothing`, and 0.44.0's section never says "security" or names the advisory. The advisory id appears once in the
   whole file, in 0.45.0's section. `### Security` has never been used.
2. **Nothing gathers what a user must do.** A section is long (0.47.0: about 117 lines of paragraphs), and actions sit
   mid-paragraph. 0.48.0 already has two:
   - the first `seal` after upgrading walks the whole history once;
   - two refusal messages change their text.
3. **Changes to machine-readable output have no home.** stikk and planeter consume the CLI's text and JSON (stikk's
   contract is the CLI only). Such changes are scattered across `Changed` and `Fixed`, for example 0.44.0's
   `content_changed`/`mode_changed` becoming `null`. The consumer letters are rebuilt by hand each release.
4. **The written convention and the practice have drifted, unchecked.** `release-compatibility.md` says an MSRV rise
   carries a `### Breaking change` entry. Recent releases write `### Changed — breaking once for Rust callers`, and
   older ones used nine `### Breaking…` headings.
5. **Tags carry no pointer.** A signed tag's message is its version and nothing else.
6. **The file keeps growing:** 3,911 lines, 266 KB, 77 sections.

## 2. Principles

- **A section leads with what a reader must know, then what changed.** A block is written only when it has something
  in it. "Nothing changed here" is said plainly, and the project's own mistakes are named, as every release handoff
  already asks.
- **What a check can read is checked. What only a reader can judge stays a review judgement**, and is not dressed up as
  a check.

## 3. The shape of a release section

From 0.48.0, a dated section `## X.Y.Z — YYYY-MM-DD` opens with these blocks, in this order, **each only when it has an
entry**:

1. **`### Security`** — one bullet per security-relevant fix. Each names:
   - the advisory id, if one is published;
   - the affected versions;
   - what a user should do;
   - a pointer to the full entry below.

   A fix of behavior reachable by **untrusted input** (a received bundle, an exchange artifact, a peer's objects) is
   security-relevant whether or not an advisory exists.
2. **`### Upgrading`** — one line per thing a user or a script must know or do after upgrading: a one-time cost, a
   changed default, a migration, a breaking change's one-line migration.
3. **`### Output changes`** — one line per change to what the CLI prints that a program may read: JSON fields and
   values, exit codes, message text, listing membership. Each gives the command, the before and the after. **The
   consumer letters are drafted from this block.**

Then the entries, as today: `### Added — …`, `### Changed — …`, `### Fixed — …`, each a heading that says the
user-visible effect, then its paragraph.

**One spelling for a breaking change:** `### Changed — breaking once: <what>`, with its one-line migration also listed
under Upgrading. `release-compatibility.md`'s MSRV sentence is corrected to match. No other `###` heading kinds are
used in a section from 0.48.0 on.

## 4. What is checked (the later round)

`tools/release-policy`, in the gates:
1. **Heading vocabulary:** from 0.48.0, a dated section uses only §3's headings, in §3's order.
2. **Advisories are named where they were fixed.** A table of published advisories (id, fixed-in version) lives in
   `SECURITY.md`. Each id must appear under `### Security` in the section of the version that fixed it.
3. **The tag names its notes.** Before a tag is pushed, a release-policy command reads the local tag's message. It
   requires the subject `prikk X.Y.Z`, then a line `Release notes: https://github.com/prikk-vcs/prikk/releases/tag/X.Y.Z`
   and a line with `CHANGELOG.md` pinned to the tag. That makes it checked, where peisear's own rule is written down
   but not checked. **Tags before 0.48.0 are not rewritten:** a tag is never moved.
4. **The archive (§5)**, with RFC 127's gate taught to find each released version's heading in `CHANGELOG.md` or in
   its archive.

**Each check is shown red** by breaking its rule: a stray heading kind; an advisory missing from its fixing section; a
tag message without its link; a version missing from both files.

## 5. Keeping the file a readable size

`CHANGELOG.md` holds the current series of ten minor versions and their patches. When 0.50.0 ships, 0.40–0.49 moves
**verbatim** to `changelog/0.40-0.49.md`. The first move also archives every earlier series, one file per series, and
`CHANGELOG.md` ends with links to them. An archive is written once and never edited after. **Known cost:** a link
someone made to an anchor of `CHANGELOG.md` on `main` breaks when its series moves. Links pinned to a tag do not.

## 6. Increments

**6.1. 0.48.0 — in release prep, no new tooling:**
- **The dev team** writes 0.48.0's section in §3's shape. Security, if any; Upgrading, including the first-seal
  sentence and the two refusal messages; Output changes; then the entries.
- **0.44.0's section gets one dated annotation**, not a rewrite. It is a first line under its heading: *"Security note,
  added 2026-MM-DD: the first `### Fixed` entry below is the fix for GHSA-px5q-233r-6hq5."*
- **The prep round lists every fix in 0.40–0.47 reachable by untrusted input**, for the architect's ruling on which
  get the same annotation.
- **The architect** cuts the 0.48.0 tag with §4.3's message.
- The release-prep template and `release-compatibility.md` carry §3's shape and §4.3's tag message (done with this
  RFC).
- **Not done without the owner's word:** editing a published GitHub Release page. That means 0.44.0's stays as
  published.

**6.2. Later, after 0.48.0: the checks of §4, and §5's archive, as one round.** The heading-vocabulary check applies from
0.48.0's section, which §6.1 writes by hand first.

## 7. Not in scope

- A `### Highlights` block. Peisear removed theirs as borrowed rather than decided. Upgrading and Output changes are
  this project's own answers to its own evidence (§1.2, §1.3).
- Generating the CHANGELOG from commits: the prose is written for a reader.
- Rewriting any past section beyond §6.1's annotations.
