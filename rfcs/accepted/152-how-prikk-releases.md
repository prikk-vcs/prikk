# RFC 152 — How prikk releases: authority, procedure, and what grows with the project

**Status.** **ACCEPTED by the owner 2026-09-13** ("It is now accepted."). §6 applied the same day: DC-35 and DC-43 archived, `EXECUTION-ORDER.md`'s release text points here, the docs and the release-notes paragraph are handed off (`152-how-prikk-releases/docs-and-release-notes-handoff-v1.md`), `MILESTONES.md`'s row is the owner's. Written as the second draft. The first was written as an
amendment to earlier governance texts and was rejected by the owner for exactly that: it argued with
documents nobody uses instead of describing the project. This draft starts from the project as it is and
where it is going. Awaits the owner's acceptance. `release-signers.toml` is not touched by anything here.

## 1. Who the project is

- **The owner** sets goals, priorities, themes and milestones; accepts RFCs; authorizes every release cut
  and every publication; decides which letters go to other projects; holds the repository's signing key.
- **The architect** designs (RFCs and handoffs), reviews every implementation against source and
  binary, keeps the ROADMAP and release schedule, executes cuts on the owner's word, and speaks to other
  projects in letters the owner sends.
- **The dev team** implements handoffs, reports for review, and since 0.41.0 prepares releases: the
  readiness sweep, the smoke script, the three-file release commit. It never pushes, tags or publishes.
- **Outside the repository:** an external architect who reviews on request; stikk, a consumer that
  drives prikk through the CLI and its JSON and re-baselines every release; brygge, a future history-import
  tool project; and crates.io users of the eight published crates.

The same one person has owned the project and held its key since the first release. Every rule below
is written for that shape and says what changes when the shape changes.

## 2. What a release is

A release is a **theme delivered**, not a date reached. The theme is named in the ROADMAP when the
owner authorizes it (0.40.0 "setup and keys survive a reboot", 0.41.0 "signing readiness", 0.42.0 "the
current branch", 0.43.0 "sealed snapshots"). Small fixes that consumers are waiting on ride with the
next theme, or the theme is cut early when a consumer is waiting on something already delivered — the
architect proposes, the owner decides. Nothing ships between releases: `main` is the next release in
progress, and every commit on it has passed the fourteen gates and CI.

**Version numbers in v0:** a release with any runtime change is a minor bump (0.41.0 → 0.42.0); a release
that changes no runtime source (installer, metadata, docs) is a patch (0.27.1, 0.31.1). Breaking changes
happen, are stated in the CHANGELOG as *breaking once* with the migration in one line, and are the
reason the crates say *"API may change without notice before 1.0"*.

## 3. The procedure — what has been done twenty-odd times, written down

1. **Readiness**: the dev team runs the release-prep handoff (`119-…/release-prep-handoff-template.md`):
   every `--help` synopsis and description against the binary, CHANGELOG completeness against the
   commits since the last tag, docs messages against the binary, the root-export diff, the package
   footprint, the memory ratio, and a smoke script covering every shipped feature that runs against any
   `prikk` binary. Findings are fixed as their own commits. The release commit is exactly three files:
   version and the seven pins, the lock's member versions, the CHANGELOG heading dated the day it is
   prepared.
2. **Review**: the architect reviews the sweep and the commit, runs the fourteen gates on that exact
   commit, and runs the smoke script against a release build of it.
3. **Push, then CI green** on every job, including the Windows and macOS suites, which are the only
   place platform code runs.
4. **Tag**: annotated, GPG-signed with the repository's configured key, message the bare version,
   verified with `git tag -v`, then pushed. The Release workflow builds four targets and publishes the
   assets. **A tag is never moved or re-signed**; a mistake ships as the next patch release.
5. **Verify the artifact**: download the Linux asset, check its sha256 against the published file,
   read its build-info (commit and tag), run the binary's `--version`, and run the sweep's smoke script
   against it. What is checked is the bytes users get, not the tree they came from.
6. **Publish on the owner's word, per release**: the eight crates in dependency order from a detached
   worktree at the tag, each confirmed on the sparse index (present, not yanked); then
   `cargo install prikk --version X` and the smoke script once more against what crates.io serves.
7. **Tell the consumers**: the CHANGELOG is the record; a letter goes to stikk when a release changes
   something they build on, re-read against the shipped binary before it is sent.

**What blocks a cut, without exception:** any red gate; any commit on `main` the architect has not
reviewed; CI not green on the release tree; a `--help` text that is false on the shipped binary. None of
these has a "but".

## 4. What protects a release today, and what does not

Protects: signed tags on a key one known person holds; checksums and build-info shipped beside every
asset; `cargo audit` and the policy gates on every push in CI; the smoke script against the artifact,
not the tree; publication by a person after the artifact is verified; never unpublishing (a bad version
is superseded by a patch release, never yanked silently — yank only with the owner's word and a CHANGELOG
line saying why).

Does not exist, on purpose, in v0: a second signer; a support window (only the latest release gets
fixes); a stability promise for the object format, the CLI's JSON schemas, or the library API. Each is
named in the crate descriptions and `SECURITY.md`, so nobody discovers it after depending on it.

## 5. What grows when the project grows

- **A second maintainer with a release key.** The day the owner adds one, `release-signers.toml` stops
  being empty — by the owner's explicit word and a commit naming the person and the fingerprint — and
  from that day a release tag must be signed by a key in that set; the release-policy tool gains that
  one check. Nothing about quorums: two people who each may release is enough until the owner says
  otherwise.
- **A first production user.** Then a support window is stated (which releases get fixes and for how
  long), `SECURITY.md`'s advisory path is exercised once as a drill, and the memory and cost measurements
  the release prep already takes become numbers a user can rely on.
- **1.0.** The stability promise arrives in three named layers, each on its own RFC when its time comes:
  the object format and exchange artifacts first (they are already versioned by schema), the CLI's JSON
  schemas second (already `-v1` everywhere), the library API last if ever.
- **Publication from CI.** Possible any time (RFC 141 has the pieces) and deliberately not done while
  one person's word is the control that matters most; it is revisited when a second maintainer exists.

## 6. What this RFC replaces

DC-35's signer-transaction design and DC-43's controls were written for a project with a maintainer
quorum. Both are archived as not applicable to v0 when this RFC is accepted; their content is not
rewritten and is available if §5's first bullet ever needs it. `EXECUTION-ORDER.md`'s release section
points here. `MILESTONES.md`'s row on the signer bootstrap is the owner's to reword — this RFC suggests
*"release governance: RFC 152"*. `docs/src/reference/` release and compatibility text says what §3 and
§4 say. `release-signers.toml` is unchanged.

## 7. Amended 2026-09-27, after the external review of `bb81b0fb` (letter 014)

**Where the project stands** (owner, 2026-09-27): *"We are out of 'early implementation' but not in 'stable version'."*
**0.48.0 is cut without the word "stable".** The release notes and the consumer letters say precisely which promises
hold. The definition of "stable" is the owner's, and the external review's six promises are the starting proposal.

**The process changes, approved by the owner** (*"Approved."*), and **which of them are permanent.** The owner asked
whether they hold until 0.48.0 or for good, noting that external reviewers are temporary. They are not exclusive to this
project (the owner confirmed this 2026-09-27: *"They are NOT exclusive to our project."*). Designing and reviewing is the architect's role, and after 0.48.0 the architect asks the owner, or external
help, when it is actually necessary. **The architect's reading, stated so it can be corrected:**

| rule | lasts | why |
|---|---|---|
| a fix round carries only the fix; new classifications, outputs or costs get their own RFC section and review | **permanent** | it costs nothing, and both new defects of the F3 cycle came in through a fix round |
| a round that needs a second addendum stops for a design re-read (an RFC) before the next handoff in that area | **permanent** | internal |
| an RFC that changes what a command reports or costs is presented for reading, and accepted in a later exchange | **permanent** | internal (RFC 162 was the first) |
| the architect's own commits to code, `docs/src` or workflows are reviewed by the development team before the architect pushes them; RFC records, handoffs and reviews are exempt | **permanent** | the one internal check of the architect; the three architect errors of the cycle were all of this kind |
| a release is cut from a **candidate**: the release commit passes the gates, CI and the crash-and-corruption matrix before it is tagged; a finding against an invariant restarts the candidate | **permanent** | mechanical, needs no outside party |
| the release workflow refuses a tag whose commit has any CI job not concluded successfully | **permanent** | mechanical (R2–R4 round, item 4) |
| an **independent adversary** tries to break the candidate (the external architect re-runs their script and matrix) | **0.48.0**, then **when necessary** | external help is temporary. After 0.48.0 the matrix, growing as findings arrive, carries the invariants. The architect asks the owner for an outside adversary before any claim of stability, a format change (format 8), or a new trust boundary |
| the candidate is exercised by both consumers against their own suites | **when a stability claim is made** | it depends on the consumers' time, and it belongs to the word "stable", not to every release |
