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
- The committed release-signer set is empty, so no release currently satisfies the DC-35 signer gate.
- Tags through 0.17.7 predate this policy and must not be reported as passing its signer-authority
  audit.
- 0.18.0 does **not** predate the policy. Its tag carries the maintainer's ordinary OpenPGP signature
  but no allowlisted release-signer authority, and no authority transaction was performed; its changelog
  states so explicitly. It must not be reported as passing the
  signer-authority audit either. Releases made while the signer set is empty state this in their own
  release notes rather than relying on the pre-policy exemption above.
- DC-35 does not provide SBOMs, provenance attestations, mature key custody/rotation/revocation, or
  production-readiness evidence. Those remain later DC-43 work.
- **The DC-35 signer-authority gate and the official-release boundary this page describes are parked**
  (RFC 119 track A, 2026-08-25): the 43 signer oracle cases that enforce them do not currently run
  (parked, not deleted -- `release/oracle/parked-cases-v1.json`). This matches the empty
  `release-signers.toml` above: prikk has one maintainer publishing under their own key, an empty
  allowlist, and no release that has ever passed the signer audit, so nothing today depends on this
  gate running. The mechanism, rules, and workflow below remain the documented design -- not
  rewritten, not removed -- and revive together with the oracle cases the day prikk enters the
  official-release regime, which requires DC-35 criterion 4's signer bootstrap.
- **RFC 119 track B (2026-08-25) removed and parked more of the same apparatus.** The release-state
  suite (23 cases, the three-authority release-lane state machine superseded 2026-08-24) was removed
  outright -- `NEVER`, not parked, no revival condition, git history is the record. The `json-parser`
  and `schema-evaluator` suites (15 cases) were removed as release policy for the same reason; the
  code they tested (`json.rs`, `schema.rs`) stays, since release-evidence's own schema check still
  uses it. 16 of `release-evidence`'s 73 cases -- the ones exercising its embedded DC-35
  signer-governance sub-object -- were parked under the same revival condition as above (second batch
  in `release/oracle/parked-cases-v1.json`); the other 57 are unaffected. `differential-check` and the
  Python it invoked (`release/observe-policy.py`, `release/check-policy.py`,
  `release/policy_check/`) were removed outright, closing the DC-93/DC-94 Python-retirement arc RFC
  119 §3 traces to this same "system reasoning about itself" pattern.

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

New repositories use format **6** and schema-2 Blocks with replay-derived clean-state Merkle roots.

**Formats 1 through 5 are rejected at open.** Each is refused with an error naming the format found. The
earlier bounded legacy read-only mode for format 1 no longer exists — it was retired when format 1 was,
and there is no read-only fallback for any superseded format.

The format has moved repeatedly and deliberately: 1→2 (state Merkle roots), 2→3 (object containers),
3→4 (ref containers), 4→5 (trust containers, received-ref index, active ref metadata), 5→6 (compaction
slots and generation logs). Prikk is early implementation software and has not committed to format
stability; **each bump is a deliberate decision that every older repository becomes unopenable.**

There is no in-place or history-preserving migration between any two formats. To carry work across a
format change, use `prikk bundle export` on a version that still opens the old repository and
`prikk bundle import` into a new one. Do not copy `.prikk/` or edit `FORMAT` to simulate migration —
editing the marker does not change the on-disk shape it describes.

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
rise is a minor-version event: the release's `CHANGELOG.md` carries a `### Breaking change` entry
naming the specific dependency or language feature that forced it. A rise whose cause is not recorded
is indistinguishable from a rise for convenience, which this policy forbids.

Current-stable quality gates are separate from minimum-version compatibility. In particular, strict
Clippy runs on current stable because its lint set changes between compiler releases.

A patch release must not intentionally break a documented surface. An unavoidable correctness or
security break uses a minor release unless a committed emergency exception is accepted by maintainer
and architect before tagging. The exception cannot waive identity versioning.

## Source Version and Release Identity

All workspace crates use one selected version. At release-candidate preparation, every internal
registry dependency must use exact `=X.Y.Z` resolution for that release. Current development manifests
still use broad `version = "0"` requirements, so that future RC gate is not yet satisfied.

Outside an exact release tag, the Cargo version is a source compatibility line, not release identity.
An untagged build is a development build even if `prikk --version` equals the latest release. It must
not be represented as a release; a shared development artifact needs its exact commit and explicit
non-release build/source metadata.

Official Git tags are unprefixed versions such as `0.18.0`, not `v0.18.0`. List them in version order:

```sh
git tag --sort=-v:refname
```

Plain lexical sorting can incorrectly make older `0.9.x` tags appear newer than `0.17.x` tags.

An official release identity consists of an authorized signed annotated tag object, its peeled commit,
and the digest of every distributed payload artifact. A valid signature from an unlisted key is not an
authorized Prikk release signature.

## Release States

| State | Workspace source line | Latest released | Candidate | Changelog | RFC location | Git identity |
|---|---|---|---|---|---|---|
| Development | last release | last release | none | no target release claim | proposed/accepted | HEAD commit metadata; no release tag at HEAD |
| Release candidate | target | last release | target | candidate entry | accepted | reviewed RC commit; target tag absent |
| Released | target | target | none | final entry | shipped RFCs in done | authorized signed target tag peels to finalization commit |

Accepted RFCs remain in `accepted/` through implementation and RC review. They move to `done/` only in
the private finalization commit selected as the tag target. That finalization state must not be pushed
without its tag. An abandoned candidate returns every candidate field to development state and creates
no target tag or asset.

The authoritative field inventory is:

| Field | Tracked authority |
|---|---|
| Workspace source line | root Cargo metadata, `Cargo.lock`, normalized packages, `prikk --version` |
| Latest release | README and implementation status |
| Current candidate | ROADMAP and implementation status |
| Change state | CHANGELOG |
| RFC lifecycle | RFC status/location, inbound links, `rfcs/README.md` |
| Release identity/status | Git tag and append-only release evidence snapshots |

Unregistered duplicate release claims are audit failures.

Run `cargo run --locked -p prikk-release-policy -- check` from the repository root to execute the
release-evidence schema/sequence fixture tables (RFC 119 track A parked the signer/challenge suites
and track B removed the release-state suite outright, superseded by the workflow below; see Core
Caveats). The Rust gate asserts date-time formats, rejects unknown schema assertions, and fails when
computed validity differs from a fixture's expected outcome. It leaves the worktree unchanged.

## Required Release Workflow

This workflow is dormant until the project owner explicitly activates preparation for a named release.
Activation requires a reviewed tracked commit that atomically changes the release lane from `parked` to
`active` and records the same exact target version in `ROADMAP.md`, `MILESTONES.md`, and
`rfcs/IMPLEMENTATION-STATUS.md`. That commit must land before requesting a fingerprint or preparing a
bootstrap candidate. Discussion, implementation completion, roadmap targets, review recommendations,
and untracked messages do not activate release work. Before bootstrap begins, parking or retargeting
uses the same reviewed three-file transition; after bootstrap begins, the governance and hold rules
below control closure.

Release conditions attach to unshipped accepted increments. If a later version first ships an increment,
it inherits all release gates and lifecycle/status corrections assigned to that increment. Retargeting
must update the three schedule/status authorities and affected RFC target/status text together. Ordinary
design-first development may continue while the release lane is parked; once activated, every applicable
step below remains binding. If the three authorities disagree, the release lane is parked; see
`MILESTONES.md` under Baseline and release posture.

1. Obtain design and implementation acceptance in isolated commits.
2. Complete any signer bootstrap/change/recovery as an earlier isolated reviewed transaction. Confirm
   that no release hold remains active.
3. Prepare one RC commit: select the target version; set exact internal requirements; update lockfile,
   candidate changelog, README, ROADMAP, MILESTONES, RFC indexes/status, mdBook, and implementation
   status without claiming release.
4. Run and record the full applicable RC gates, package inspection, and adversarial RC review.
5. After RC acceptance, create one private finalization commit: remove candidate wording, set latest
   released, clear the candidate, move shipped RFCs to `done/`, and repair every link/status field.
6. On the clean finalization commit, rerun the complete deterministic gate suite. RC results do not
   substitute for this run.
7. Create an unprefixed signed annotated tag at that commit. Verify the authorized primary signer,
   signature, tag object, and peeled commit. Generate and inspect staged archive assets once.
8. Require a successful atomic-push capability check. Publish branch and tag only with
   `git push --atomic <remote> <branch> <tag>`. Unsupported atomic push aborts; there is no non-atomic
   fallback. Atomic publication of commit and tag is the release event.
9. Publish staged immutable assets and crates from the exact clean tagged tree, then record external
   status. GitHub Release, crates.io, and Pages are asynchronous distribution, not Git release identity.

The finalization and external steps form one controlled transaction. A local failure before atomic
push may be corrected or abandoned without publishing false state. After atomic push, the release
exists even if distribution is pending or partial. Published identities are preserved; retry only
missing outputs or supersede with a new version.

## Release Signer Governance

[`release-signers.toml`](https://github.com/prikk-vcs/prikk/blob/main/release-signers.toml) is the strict
commit-local allowlist. The current empty array authorizes nobody and blocks official release. The file
supports multiple full uppercase OpenPGP primary fingerprints; two active operators are encouraged
when available but are not required at the current project scale.

The signer file is not the ultimate trust root. Reviewed protected-branch governance authorizes signer
policy changes, an allowlisted private key authenticates a tag, hosting and registry administrators
control publication, and evidence binds those independently administered outputs. Administrator
override is an incident, not ordinary authority.

Every bootstrap, addition, replacement, or removal is isolated before RC finalization and approved by
two distinct natural persons: one repository maintainer/administrator and one independent architect or
security reviewer. A maintainer may approve admission of their own key in the maintainer role but
cannot supply the independent approval. Automation supplies neither identity. Existing-signer approval
is useful continuity evidence, never a recovery veto.

The release-state audit uses one canonical governance record for the transaction, signer-set effect,
proofs, approvals, authority blobs, public record, and hold. Independent records from different
transactions cannot be combined to authorize a development-stage authority change.

A new fingerprint requires a fresh, versioned, transaction-bound, expiry-bounded non-secret signed
challenge. Proof applicability is derived from normalized old/new fingerprint sets:

| Transaction effect | Required authority proof |
|---|---|
| Bootstrap, addition, replacement | `verified` for every introduced fingerprint |
| Removal-only | `not-applicable` with reason |
| Classification-only, unchanged authority | `not-applicable` with reason |

Authority proof and later release-tag verification are distinct evidence. Strict signer grammar and
positive/forbidden cases are defined in the [release policy data](https://github.com/prikk-vcs/prikk/tree/main/release).

## Loss, Compromise, and Disputes

- All-key unavailability/unusability triggers loss recovery.
- Any suspected compromised authorized key triggers compromise containment.
- Any material signer, authority transaction, tag, or release dispute triggers dispute containment.

Each trigger immediately holds new official tags and incomplete/future distribution. Initial bootstrap
uses the same controls. The incident opens a durable public record, obtains the two accountable
approvals, records transaction-appropriate proof, makes an isolated governance change/record, and keeps
publication blocked for at least 72 hours after evidence becomes public. Architect/security re-review
must accept containment/classification and explicitly lift the hold.

An active incident snapshot records a null hold end and lift. A later append-only snapshot may fill its
classification, end, and explicit lift after the minimum interval. Filled governance fields cannot be
rewritten, and an active hold cannot coexist with distribution `complete`.

A disputed published tag is classified by the same two-person process as `valid-at-publication`,
`never-authorized/hostile`, or still `disputed`. Only the first two can receive an explicit hold lift;
`disputed` remains held. Emergency administrator quarantine is containment, not normative status.
Valid releases are never retagged or replaced. A hostile identity is quarantined with forensic/public
incident evidence, its version/name is burned, and it is never reused.

This governance controls only official upstream Prikk tags, assets, and package namespaces. It does not
restrict contributions, reviews, Apache-2.0 forks, downstream builds, or downstream releases under
distinct identities.

## Archives, Crates, and Completion

The source archive is `prikk-vX.Y.Z.tar.gz`, including the `v` that Git tags omit. Tracked files appear
at archive root. Generation uses deterministic gzip metadata and a new no-clobber staging directory.
The checksum asset is `prikk-vX.Y.Z.tar.gz.sha256`; it contains lowercase SHA-256, two ASCII spaces, the
archive basename, and LF. The digest covers compressed bytes. Published names and bytes are immutable.

The package graph is derived from normalized manifests. Its current publication levels are:

1. `prikk-error`, `prikk-hash`
2. `prikk-crypto`, `prikk-object`
3. `prikk-replay`
4. `prikk-store`
5. `prikk`

Before tagging, staged packages must build against an isolated local registry without path overrides.
External publication waits for each predecessor to become registry-visible. For every crate, staged
`.crate` SHA-256, registry-index checksum, and fetched-byte SHA-256 must match. A mismatch is partial,
blocks dependents, preserves the published version, and requires a superseding version.

Distribution is `pending`, `partial`, `complete`, or `superseded`. `complete` requires:

- archive and checksum attached under fixed names with matching recorded bytes;
- every expected crate visible with equal staged, index, and fetched checksums;
- the release page published rather than draft; and
- Pages deployed at the peeled commit, or a reasoned pre-publication review ruling it inapplicable.

Configured Pages failure or delay remains pending/partial and cannot be waived afterward.

## Evidence Snapshots

Release evidence snapshots are immutable assets named
`prikk-X.Y.Z-release-evidence-NNN.json`. Sequence starts at `001`, remains contiguous, and links each
predecessor by name and the SHA-256 of its exact observed published bytes. Whitespace, key order, and
final newline are part of that immutable asset identity; re-serialized JSON is not substituted. The
highest valid snapshot is authoritative; absence means pending. Snapshots preserve cumulative attempts,
including failed evidence attachments, and every successor adds at least one newly sequenced attempt.
They never change prior identities or observed tag verification. Each parsed snapshot is validated
against the JSON value decoded from the same exact bytes whose digest participates in the chain.

Tag verification is coherent in every distribution state: `not-observed` has no detail, `verified` has
all signer/authority/verifier detail, and `failed` has authority and verifier detail with an optional
fingerprint when extraction was ambiguous.

The strict structural schema is
[`release-evidence-v1.schema.json`](https://github.com/prikk-vcs/prikk/blob/main/release/schemas/release-evidence-v1.schema.json).
Semantic checks additionally enforce predecessor continuity, immutable identity, normalized signer-set
effects, two-person approvals, proof applicability, cumulative attempt history, crate checksum equality,
and exact completion outputs.

Source archives and `.crate` files are payload artifacts whose digests bind release identity. Checksum
files and evidence snapshots are integrity/status metadata, avoiding a self-referential digest rule
while keeping published names and bytes immutable.

## Gates and Evidence Honesty

Applicable deterministic release gates include:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --locked
mdbook build docs
git diff --check
```

Release review also checks clean commit/tag identity, exact internal requirements, normalized packages,
isolated-registry builds, RFC/status/link consistency, signer governance, archive/checksum grammar,
release-state fixtures, and evidence sequence/completion rules.

A policy list is not passing evidence. Every review/release record must state commands actually
observed, unavailable or inapplicable checks, and environment limits. Never report `cargo audit`,
`cargo deny`, crash/reboot testing, registry publication, GitHub Release publication, or Pages
deployment as passed unless observed for that exact release.

## Claim-to-Source Anchors

| Claim | Source anchor |
|---|---|
| Compatibility, state, signer-governance, and distribution rules | [DC-35](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/DC-35-RELEASE-COMPATIBILITY-STATUS-CORRECTION.md) |
| Identity changes require new explicit version/domain authority | [DC-34](https://github.com/prikk-vcs/prikk/blob/main/rfcs/accepted/DC-34-PUBLICATION-IDENTITY-AUTHORITY.md) |
| Format-1/format-2 compatibility and refusal boundary | [DC-40](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-40-STATE-MERKLE-FORMAT-TRANSITION.md) |
| RFC lifecycle and same-release transition | [RFC-000](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/000-rfc-lifecycle-policy.md) |
| Strict signer/evidence policy data and fixtures | [release policy data](https://github.com/prikk-vcs/prikk/tree/main/release) |
| Released change history | [CHANGELOG](https://github.com/prikk-vcs/prikk/blob/main/CHANGELOG.md) |

## Provenance

This reference implements DC-35's policy/documentation surface. It does not authorize a signer, change
Cargo requirements, create a release candidate, or claim that any release transaction or external
distribution gate has passed.
