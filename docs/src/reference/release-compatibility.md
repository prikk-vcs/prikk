# Release, Versioning, and Compatibility

This page defines Prikk's current pre-1.0 compatibility and official-release policy. It separates
source versioning, Git release identity, external distribution, and evidence so a release does not need
later housekeeping to become truthful.

For repository format details, see [repository layout and authority](./repository-layout.md). For
identity-bearing objects, see the [data model](./data-model.md). For persistence limits, see
[durability and crash recovery](./durability-recovery.md).

## Core Caveats

- Prikk is pre-1.0 experimental software, not a production Git replacement.
- Cargo APIs, CLI behavior, object schemas, and repository formats are not generally stable.
- No migration path, support window, LTS line, or 1.0 schedule is promised.
- The workspace version alone does not identify an official release.
- Current release checks are partly manual. `cargo deny` is not a configured gate. **`cargo audit`
  is**: it runs as a standing release gate locally (`--no-fetch`, `rfcs/EXECUTION-ORDER.md` §6 rule 9)
  and as a scheduled CI job every Monday against a **freshly fetched** advisory database
  (`.github/workflows/security-audit.yml`), so an advisory that arrives without any code change is
  still caught.
- **Releases are made by one maintainer under their own signing key**, by the procedure in
  [RFC 152](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/152-how-prikk-releases.md) — see [How prikk releases](#how-prikk-releases) below. In v0 there is
  **no second signer, no support window** (only the latest release gets fixes) **and no stability
  promise** for the object format, the CLI's JSON schemas or the library API. Each is absent on
  purpose and named here so nobody discovers it after depending on it.
- `release-signers.toml` is **empty because no multi-signer policy exists yet**, not because releases
  are unsigned: every release tag is signed by the maintainer key and verified before it is pushed.
  The file gains entries the day a second maintainer with a release key exists
  ([§ What changes as the project grows](#what-changes-as-the-project-grows)).

## Compatibility Surfaces

Prikk treats these as separate compatibility surfaces:

- Cargo crate source APIs and feature sets;
- command names, arguments, exit behavior, and human-readable output;
- canonical object schemas, signature preimages, ObjectIds, and identity domains;
- repository format, on-disk layout, and migration/refusal behavior; and
- source archives and published documentation.

Before 1.0, a minor release may intentionally change documented Cargo or CLI surfaces when release
notes identify the change. Repository-format changes additionally require an accepted governing RFC
with a new format/schema where applicable and explicit directional read, write, migration, and refusal
behavior.

SemVer and release notes cannot authorize silent identity mutation. A signature preimage, ObjectId,
canonical identity schema, or domain change requires accepted design authority, a new explicit version
or domain, refusal or migration behavior, and literal compatibility vectors. Existing identity
versions are never reinterpreted.

## Format Stability Contract

**RFC 114, answering badge criterion 2** — what minimum must never change for a verification claim
made today to hold in ten years. Stated as a promise a user can rely on:

> Any prikk release can read every object any prior **supported** release wrote, and verifies it to
> the same conclusion. Storage may require a migration step, which is documented and tested. Object
> identity and signatures never require one.

This splits every byte in a repository into two categories, and they have opposite rules.

**Frozen forever — verification-bearing, never changes once shipped:**

- The object-id preimage: `OBJECT_ID_DOMAIN` (`b"PRIKK-OBJECT-ID-v1"`) ‖ type code (u16 BE) ‖
  `schema_version` (u32 BE) ‖ payload length (u64 BE) ‖ canonical payload, hashed SHA-256.
- The canonical encoding of each `(object_type, schema_version)` pair that has ever been written by a
  shipped release.
- The signature preimage, per signer role.
- The algorithm identifiers themselves (Ed25519, SHA-256) — not merely the algorithms.
- The patch-set digest preimage (RFC 115 Stage 1, design-v1.md §5/D4): `PATCH_SET_DIGEST_DOMAIN`
  (`b"PRIKK-PATCH-SET-DIGEST-v1"`) ‖ count (u64 BE) ‖ each patch id, sorted ascending and
  deduplicated, 32 bytes each, hashed SHA-256. Not itself a stored object or an `ObjectId` — a
  comparison value, following `MerkleRoot`'s shape — but identity-bearing all the same: two prikk
  versions must produce identical bytes over the same patch set, or the "are these two repositories
  the same?" comparison it exists for means nothing across an upgrade.

**May change, and must carry a tested migration path:**

- Repository format version and on-disk directory layout.
- Container framing, the object index, the WAL.
- The bundle exchange format.

Freezing is not "never add a field." A schema version lives inside the object-id preimage, so a new
field means a new schema version and new ids for objects written under it — with no change whatsoever
to objects already written under an earlier schema version. The obligation is to keep every version
ever shipped decodable forever, hashing exactly the way it did on the day it was written — not to stop
evolving.

**If SHA-256 or Ed25519 is ever broken:** a broken algorithm gets a **new** algorithm identifier, used
by new objects going forward. Every object already written continues to verify under the identifier it
was written with, unchanged. An existing identifier is never redefined — doing so would retroactively
alter what past objects mean, which is exactly what this contract exists to prevent.

**The obligation covers shipped releases, not every commit.** A schema version or format that only
ever existed in an unreleased build is owed nothing under this contract; what shipped is permanent,
what merely existed in git history is not.

**Formats 1 through 5 are not supported under this contract** (owner decision, RFC 114 §5.3) — prikk
has never been used in production, so there is nothing to protect there. See
[Repository Format Transitions](#repository-format-transitions) below for what that means for each
retired format specifically.

## Repository Format Transitions

New repositories use format **7** and schema-2 Blocks with replay-derived clean-state Merkle roots.
Format 7 is format 6 plus one rule: an object id may hold several container records, and the last one
is authoritative — which is how one object carries several signers.

**Formats 1 through 5 are rejected at open.** Each is refused with an error naming the format found. The
earlier bounded legacy read-only mode for format 1 no longer exists — it was retired when format 1 was,
and there is no read-only fallback for any superseded format.

**Format 6 stays openable, and upgrades in place.** A format-6 repository opens and works as before,
holding one record per id: a second signer's copy of a stored object is refused, naming the upgrade.
`prikk format upgrade` moves it to format 7 in place: it takes the writer lock, refuses — changing
nothing — unless `prikk verify` would pass, and then rewrites only the `FORMAT` marker, atomically.
Nothing stored is rewritten, because every format-6 repository is already a valid format-7 one. The
upgrade is explicit, never automatic, idempotent, and one-way: **prikk 0.44.0 and earlier refuse a
format-7 repository at open**, so upgrade only once every prikk that opens the repository is newer.

The format has moved repeatedly and deliberately: 1→2 (state Merkle roots), 2→3 (object containers),
3→4 (ref containers), 4→5 (trust containers, received-ref index, active ref metadata), 5→6 (compaction
slots and generation logs), 6→7 (several records per object id). Prikk is early implementation software
and has not committed to format stability. Bumps up to 6 each made every older repository unopenable,
with no in-place migration; 6→7 is the first a repository crosses in place.

To carry work across one of the earlier transitions, use `prikk bundle export` on a version that still
opens the old repository and `prikk bundle import` into a new one. **Never edit `FORMAT` by hand** —
editing the marker does not change the on-disk shape it describes, and skips the verification
`prikk format upgrade` requires.

## Bundle Format Transitions

The bundle exchange artifact (`prikk bundle export`/`import`) carries its own magic and version,
independent of the repository format above. Bundles are always **exported** as `PBNDL003` (DC-44
increment 3, `bundle-manifest-handoff-v1.md`), which added a self-describing manifest section after
the author-key section — an addition `PBNDL002` has no room for and cannot be extended into
silently, so the bump is fail-closed on the write side: an older client meeting a newer bundle
refuses it outright with its own hardcoded magic check. The manifest names the on-disk repository
format and the exporting tool's own version, and states plainly that a bundle is one ref's closure
only — other refs in the source repository, if any, are not included, and the bundle makes no claim
about them. Both `prikk bundle export` and `prikk bundle verify` print it.

**`PBNDL001` and `PBNDL002` bundles are still accepted on import** (`PBNDL001` acceptance corrected
shortly after the `PBNDL002` bump shipped — `bundle-v1-import-regression-v1.md`; `PBNDL002`
acceptance is this bump's own §3 obligation, verified with real bytes, not a hand-built
approximation). This is read compatibility only, the same asymmetry every repository-format
transition on this page already has: read what an older client wrote, write only the current
format. This is also what keeps the repository-format migration path above actually usable — an old
repository can only be opened by an old prikk build, and that build only ever produces a `PBNDL001`
bundle, so refusing to import one would sever that migration in both directions at once. A
`PBNDL001` bundle decodes exactly like a `PBNDL002` one whose author-key section is empty, which in
turn decodes exactly like a `PBNDL003` one whose manifest section is absent: the Patches a
`PBNDL001` bundle carries read `Unverifiable`, the same outcome DC-53 already defines for any Patch
this repository never recorded AUTHOR key material for, and a `PBNDL001`/`PBNDL002` bundle's own
`verify` report simply states no manifest is present rather than printing absent fields as if they
had been checked.

The workspace's declared minimum Rust version is exactly 1.85.0. The locked product workspace must
check, test, and build on that toolchain:

```sh
cargo +1.85.0 check --workspace --all-targets --locked
cargo +1.85.0 test --workspace --locked
cargo +1.85.0 build --workspace --locked
```

**MSRV rise policy.** The floor above cannot go lower — it is the edition-2024 minimum — but it can
rise. It rises only when a dependency or language requirement forces it, never for convenience, and a
rise is a minor-version event: the release's `CHANGELOG.md` carries a `### Changed — breaking once:`
entry naming the specific dependency or language feature that forced it, and a line under `### Upgrading`. A rise whose cause is not recorded
is indistinguishable from a rise for convenience, which this policy forbids.

Current-stable quality gates are separate from minimum-version compatibility. In particular, strict
Clippy runs on current stable because its lint set changes between compiler releases.

A patch release must not intentionally break a documented surface. An unavoidable correctness or
security break uses a minor release unless a committed emergency exception is accepted by maintainer
and architect before tagging. The exception cannot waive identity versioning.

## Version Numbers and Release Identity

One version covers the whole workspace: the workspace `version` and the seven internal crate pins
carry it together. **In v0 a release with any runtime change is a minor bump** (0.41.0 → 0.42.0); **a
release that changes no runtime source** (installer, metadata, docs) **is a patch** (0.27.1, 0.31.1).
Breaking changes happen before 1.0; each is stated in the CHANGELOG as *breaking once*, with the
migration in one line, and is why the crates say *"API may change without notice before 1.0"*.

A release is its **signed, annotated tag**. Tags are unprefixed versions such as `0.42.0`, not
`v0.42.0`. List them in version order:

```sh
git tag --sort=-v:refname
```

Plain lexical sorting can incorrectly make older `0.9.x` tags appear newer than `0.17.x` tags. An
untagged build is a development build even when `prikk --version` equals the latest release.

## How prikk releases

A release is **a theme delivered**, not a date reached: the owner authorizes a cut and names its theme,
and `main` is the next release in progress — every commit on it has passed the fourteen local gates and
CI. The procedure ([RFC 152 §3](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/152-how-prikk-releases.md)):

1. **Readiness.** The dev team checks every `--help` synopsis and description against the binary, the
   CHANGELOG against the commits since the last tag, docs messages against the binary, the root-export
   diff, the package footprint and the memory ratio, and writes a smoke script covering every shipped
   feature that runs against any `prikk` binary. Findings are fixed as their own commits. The release
   commit is exactly three files: the version and its pins, the lockfile's member versions, and the
   CHANGELOG heading dated the day it is prepared.
2. **Review.** The architect reviews the sweep and the commit, runs the fourteen gates on that exact
   commit, and runs the smoke script against a release build of it.
3. **Push, then CI green** on every job, including the Windows and macOS suites — the only place
   platform code runs.
4. **Tag.** Annotated, GPG-signed with the repository's configured key, verified with `git tag -v`,
   then pushed. From 0.48.0 the tag's message is the subject `prikk X.Y.Z`, a blank line, then two
   lines:
   - `Release notes: https://github.com/prikk-vcs/prikk/releases/tag/X.Y.Z`;
   - a link to `CHANGELOG.md` pinned to the tag.

   The Release workflow builds four targets and publishes the assets, with notes generated from that
   version's CHANGELOG section. **A tag is never moved or re-signed**; a mistake ships as the next patch
   release. Tags before 0.48.0 keep their bare version message.
5. **Verify the artifact.** Download the Linux asset, check its sha256 against the published file, read
   its build-info (commit and tag), run its `--version`, and run the smoke script against it: what is
   checked is the bytes users get, not the tree they came from.
6. **Publish on the owner's word, per release.** The eight crates in dependency order from a detached
   worktree at the tag, each confirmed on the registry index; then `cargo install prikk --version X`
   and the smoke script once more against what crates.io serves.
7. **Tell the consumers.** The CHANGELOG is the record. A project building on something a release
   changed hears about it directly, starting from the section's `### Output changes`.

Each release section leads with what a reader must know, each block only when it has something in it:
`### Security` (with the advisory id where one is published), `### Upgrading` (what to do after
upgrading), and `### Output changes` (what a program reading the CLI will see differently). Then come the
`Added`, `Changed` and `Fixed` entries ([RFC 161](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/161-release-notes-a-reader-and-a-consumer-can-use.md)).

**What blocks a cut, without exception:** any red gate; any commit on `main` the architect has not
reviewed; CI not green on the release tree; a `--help` text that is false on the shipped binary.

## What protects a release, and what deliberately does not exist

**Protects:** signed tags on a key one known person holds; checksums and build-info published beside
every asset; `cargo audit` and the release-policy gates on every push in CI; the smoke script run
against the artifact, not the tree; publication by a person after the artifact is verified; and never
unpublishing — a bad version is superseded by a patch release, and yanked only on the owner's word
with a CHANGELOG line saying why.

**Does not exist, on purpose, in v0:** a second signer; a support window; a stability promise for the
object format, the CLI's JSON schemas or the library API. A checksum on a downloaded binary proves
integrity of transport. Whatever you obtain, verify its content with `prikk verify`.

## What changes as the project grows

- **A second maintainer with a release key.** On the owner's explicit word, in a commit naming the
  person and the fingerprint, `release-signers.toml` gains its first entries; from then on a release
  tag must be signed by a key in that set, and the release-policy tool gains that one check.
- **A first production user.** A support window is stated, `SECURITY.md`'s advisory path is exercised
  once as a drill, and the memory and cost measurements release prep already takes become numbers a
  user can rely on.
- **1.0.** The stability promise arrives in three named layers, each on its own RFC: the object format
  and exchange artifacts first (already versioned by schema), the CLI's JSON schemas second (already
  `-v1` everywhere), the library API last if ever.
- **Publication from CI.** Possible at any time and deliberately not done while one person's word is
  the control that matters most; revisited when a second maintainer exists.

## Release-Policy Checks

Run `cargo run --locked -p prikk-release-policy -- check` from the repository root to execute the
release-evidence schema and sequence fixture tables. It asserts date-time formats, rejects unknown
schema assertions, fails when computed validity differs from a fixture's expected outcome, and leaves
the worktree unchanged. It is one of the fourteen gates every candidate runs locally
(`rfcs/EXECUTION-ORDER.md` §6 rule 9), and CI's `policy` job runs it with the other three
release-policy gates on every push.

Every review and release record states the commands actually observed, and what was unavailable or
inapplicable. Nothing — `cargo audit`, registry publication, a GitHub Release, Pages — is reported as
passed unless it was observed for that exact release.

## Claim-to-Source Anchors

| Claim | Source anchor |
|---|---|
| Who releases, the procedure, what protects a release, and what grows with the project | [RFC 152](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/152-how-prikk-releases.md) |
| The earlier signer-transaction design and release controls, archived as not applicable to v0 | [DC-35](https://github.com/prikk-vcs/prikk/blob/main/rfcs/archive/DC-35-RELEASE-COMPATIBILITY-STATUS-CORRECTION.md), [DC-43](https://github.com/prikk-vcs/prikk/blob/main/rfcs/archive/DC-43-RELEASE-SECURITY-CONTROLS.md) |
| Identity changes require new explicit version/domain authority | [DC-34](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/DC-34-PUBLICATION-IDENTITY-AUTHORITY.md) |
| Format-1/format-2 compatibility and refusal boundary | [DC-40](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-40-STATE-MERKLE-FORMAT-TRANSITION.md) |
| RFC lifecycle and same-release transition | [RFC-000](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/000-rfc-lifecycle-policy.md) |
| Released change history | [CHANGELOG](https://github.com/prikk-vcs/prikk/blob/main/CHANGELOG.md) |

## Provenance

The compatibility, format and bundle sections implement RFC 114 and the DC-series format decisions.
The release sections describe RFC 152: they replaced, on 2026-09-13, the DC-35 signer-governance,
release-state, disputed-tag and evidence-snapshot sections written for a maintainer quorum the
project does not have. Nothing here authorizes a signer or claims that any release passed a check it
was not observed passing.
