# RFC (accepted) - DC-35 Release Compatibility and Status Correction

**Status.** **ARCHIVED 2026-09-13 — superseded by RFC 152 ("How prikk releases") on the owner's ruling that its two-person signer rules do not fit a one-owner project in v0.** Its transaction design is kept here for the day a second maintainer holds a release key (RFC 152 §5). `release-signers.toml` is unchanged. Earlier: Governance amendment accepted after architect design re-review v3 on 2026-07-15;
implementation accepted after architect repair re-review v3 on 2026-07-16 with no signer admitted.
**Owner ruling.** Repository-governed signer changes, non-mandatory existing-key approval, and the
72-hour break-glass hold were approved on 2026-07-15.
**Target milestone.** M1 - required before the 0.18.0 release candidate.
**Tracks.** TASK-13 and architect review N3.
**Touches.** Release/compatibility reference, implementation-status correction, release-signer
fingerprint authority, README/ROADMAP links, and release-state bookkeeping. This increment is policy
and documentation only; required manifest changes occur in the later release-candidate increment.

## Problem

Release rules and compatibility limits are distributed across README, ROADMAP, release reviews, Cargo
metadata, and current-state references. The implementation status also contradicts the released public
merge surface by omitting `merge-plan` and describing public merge evidence as absent.

Past release reviews repeatedly found a second failure mode: a candidate, finalization commit, tag,
and published release can each describe a different state. Post-release housekeeping then leaves a
window where the published tag or archive permanently contains stale README, changelog, roadmap, RFC,
or version claims. The release process needs one state matrix and one finalization transaction rather
than informal cleanup after publication.

## Design

Add `docs/src/reference/release-compatibility.md` as the public current-state policy. It must document:

- pre-1.0 semantic-version expectations and the absence of stable Rust API, CLI-output, object-schema,
  or repository-format guarantees unless a later RFC says otherwise;
- unprefixed Git tags such as `0.18.0`, while release archives use `prikk-v0.18.0.tar.gz`;
- the design, implementation review, release-candidate review, final release-state flip, tag, package,
  and publish sequence;
- the requirement that README, CHANGELOG, ROADMAP, RFC status/location, implementation status,
  workspace version, lockfile, and relevant mdBook pages already describe the release at publication;
- immutable release assets: a bad published artifact is superseded, never replaced under the same name;
- current manual gates and the distinction between observed evidence and policy.

Correct the merge-surface statements in `rfcs/IMPLEMENTATION-STATUS.md` against released 0.17.7
behavior. Link the new reference from relevant public and maintainer documentation without turning the
README into an internal development log.

### Compatibility contract

The reference must distinguish these surfaces explicitly:

- Cargo crate source APIs and feature sets;
- command names, arguments, exit behavior, and human-readable output;
- canonical object schemas, signature preimages, and ObjectIds;
- repository format, on-disk layout, and migration behavior;
- release archives and published documentation.

Before 1.0, an explicitly documented minor release may break Cargo APIs/features or CLI commands,
arguments, exits, and human output. Repository-format changes additionally require an accepted
governing RFC that assigns a new format/schema as applicable and defines directional read, write,
migration, and refusal behavior. Identity-bearing changes are not authorized by SemVer or release
notes: a signature preimage, ObjectId, canonical schema, or domain change requires an accepted RFC, a
new explicit version/domain, refusal or migration behavior, and literal compatibility vectors. Existing
identity versions are never silently reinterpreted.

A patch release must not intentionally break a documented surface. An unavoidable correctness or
security break uses a minor release unless a committed emergency exception is accepted by the
maintainer and architect before tagging. The exception must identify the affected surface, governing
RFC/schema authority, rejection or migration behavior, evidence, and user action; it cannot waive the
new-version/domain rule for identity-bearing bytes.

All workspace crates use one selected version and every internal registry dependency uses exact
`=X.Y.Z` resolution for that release. Lockstep means exact resolution, not merely inheriting one package
version in the workspace. No support window, LTS line, or compatibility promise is inferred from
continued readability of a historical format.

### Release-state matrix

The workspace version is a source compatibility line, not sufficient release identity. Outside the
exact release tag, a build is a development build even when its Cargo version equals the latest
release. An untagged build must not be distributed or described as a release, and any shared
development artifact must expose its exact commit and non-release status in basic build/source
metadata. Release identity is the authorized signed annotated tag object, its peeled commit, and the
digest of each distributed payload artifact.

The page must define these states:

| State | Workspace source line | Latest released | Current candidate | Changelog | RFC location | Git identity |
|---|---|---|---|---|---|---|
| Development | last release | last release | none | no unreviewed release claim | proposed/accepted | HEAD commit provenance; no release tag at HEAD |
| Release candidate | target version | last release | target version | target entry marked release candidate | accepted | reviewed RC commit; no target tag |
| Released | target version | target version | none | final target entry | shipped RFCs in done | signed annotated target tag peels to finalization commit |

An accepted RFC remains under `accepted/` during implementation and release-candidate review. It moves
to `done/` only in the finalization commit that becomes the release tag target. A rejected or abandoned
candidate returns all candidate fields to development state in a reviewed commit; it is not described
as released and creates no target tag or release asset.

The implementation must maintain an explicit field-to-asset inventory. At minimum, workspace source
line maps to root Cargo metadata, `Cargo.lock`, normalized package manifests, and `prikk --version`;
latest release maps to README and implementation status; candidate maps to ROADMAP and implementation
status; change state maps to CHANGELOG; RFC state maps to RFC status/location, inbound links, and
`rfcs/README.md`; release identity maps to the Git tag and release evidence snapshots. An unregistered
duplicate release claim is an audit failure.

The table-driven audit must test every valid row and reject mixed rows, including: candidate and latest
release both naming the target; released/latest/done claims without the exact tag; a tag that peels to
a different commit; a tagged tree retaining candidate wording or shipped RFCs in `accepted/`; broad or
mismatched internal Cargo requirements; distribution-complete claims with missing outputs; and an
abandoned candidate retaining any target claim. The finalization row without a public tag is allowed
only as a private local transaction and must never be pushed separately.

The audit is tracked release-gate tooling, not Prikk product/runtime behavior. DC-35's no-executable-
product-feature boundary does not prohibit the deterministic validator required to enforce these
release-policy tables and evidence contracts.

The audit must also reject release authority changed inside an RC/finalization commit, an unauthorized
or ambiguous signer, self-approved signer admission, missing or untyped governance/proof state, an
active release hold, and a disputed or hostile tag represented as an official release. A transaction
that introduces a fingerprint requires verified proof of possession. `not-applicable` proof is valid
only for removal-only or classification-only transactions with a recorded reason and no introduced
fingerprint; the audit rejects it when any fingerprint is introduced.

External distribution is a separate dimension from Git release state: `pending`, `partial`, `complete`,
or `superseded`. Atomic publication of the finalization commit and tag is the release event. Absence of
a release evidence snapshot means distribution is `pending`, never complete.

### Required workflow

The policy must specify this order:

1. Obtain design and implementation acceptance and record their isolated commits. Complete any signer-
   authority bootstrap/change/recovery as an earlier isolated reviewed transaction and verify that no
   release hold remains active.
2. Prepare one release-candidate commit: select the version; update workspace metadata, every internal
   registry requirement to exact `=X.Y.Z`, and the lockfile;
   add the candidate changelog entry; update README, ROADMAP, MILESTONES, RFC indexes/status, relevant
   mdBook pages, and implementation status; keep latest-released and RFC lifecycle claims honest.
3. Run and record the full applicable gates against that exact commit, including package inspection,
   then obtain adversarial release-candidate review.
4. After RC acceptance, prepare the finalization commit: remove candidate wording, set latest released,
   clear the active candidate, move shipped RFCs from `accepted/` to `done/`, and make every tracked
   release asset describe the final state.
5. On the clean finalization commit, rerun and record the full deterministic release gate suite,
   including package/local-registry, mdBook/link, RFC lifecycle, version/lockfile, and release-state
   checks. RC evidence is not substituted for this run.
6. Create the unprefixed signed annotated tag at that exact commit and verify its signature and peeled
   commit against the committed release-signer authority. Generate and inspect the archive/checksum
   once in a new no-clobber staging directory. Verify the tag/archive release-state row before any
   remote publication.
7. Require a successful atomic-push capability check, then publish branch and tag together with
   `git push --atomic <remote> <branch> <tag>`. If the remote does not support atomic push, abort; there
   is no branch-first, tag-first, or multi-command fallback. Verify the remote tag object and peeled
   commit. This atomic publication is the release event.
8. Publish the staged archive/checksum and workspace crates from the exact clean tagged tree. Publish
   crates in manifest-derived dependency order, waiting for registry visibility before each dependent.
   Record every external result in append-only release evidence and verify the release page and Pages
   deployment without treating either as Git release identity.

Steps 4-8 are a controlled release transaction, not post-release housekeeping. If local finalization
fails before atomic push, correct or abandon the private transaction without publishing false state.
After atomic push, the release exists even when distribution is pending or partial. Preserve every
published identity, append evidence for each attempt, and retry only missing immutable outputs or
supersede with a new version. Never retag a published version, replace an archive or evidence snapshot
under the same name, or overwrite a published crate version.

### Package and artifact authority

The internal package graph is derived from normalized manifests, not maintained only as prose. For the
current graph its publish levels are: `prikk-error` and `prikk-hash`; then `prikk-crypto` and
`prikk-object`; then `prikk-replay`; then `prikk-store`; then `prikk`. RC and finalization evidence must
inspect every normalized manifest for exact selected-version requirements. Before tagging, an isolated
local registry must be populated from the staged package archives in graph order and each packaged
crate must build against that registry without workspace path overrides. During external publication,
the same graph is used and each predecessor must be observed in the registry before a dependent is
attempted.

The source archive is exactly `prikk-vX.Y.Z.tar.gz`; its checksum asset is exactly
`prikk-vX.Y.Z.tar.gz.sha256`. The digest covers the published compressed bytes. The checksum file is one
lowercase 64-hex SHA-256 digest, two ASCII spaces, the archive basename, and LF. Archive generation uses
tracked bytes from the peeled tag commit at archive root and deterministic gzip metadata (`gzip -n` or
an equivalently reviewed command). Generation occurs once in a new staging directory that refuses
existing output names. An unpublished abandoned staging transaction may be discarded; a published
name is immutable.

### Distribution evidence

The release page carries immutable, monotonically numbered JSON evidence snapshots named
`prikk-X.Y.Z-release-evidence-NNN.json`. The highest valid sequence is authoritative; snapshots are
appended, never replaced. Absence means `pending`. Every snapshot is keyed by version and records:

- the tag name, tag-object id, tag-signature verification result, and peeled commit id;
- archive and checksum names, compressed-byte SHA-256, checksum grammar result, and archive-root result;
- normalized crate names/versions/exact internal requirements, graph order, staged/registry/fetched
  checksums, equality result, and per-crate publication and registry-visibility status;
- release-page status and, separately, Pages workflow/deployed-revision status;
- signer-governance transaction type/action or classification, approvals, typed proof state/result or
  `not-applicable` reason, hold interval, and old/new authority-file blob ids when bootstrap, recovery,
  containment, dispute, or authority change applies;
- overall `pending`, `partial`, `complete`, or `superseded` status, attempt time, and prior snapshot.

The snapshot grammar must reject missing required fields, unknown fields, invalid state transitions, a
`complete` state with any missing/failed output, and identity changes from an earlier snapshot. A failed
attachment leaves the normative state `pending` or at the last snapshot; recovery appends the next
snapshot. Partial publication never changes the Git release identity and never permits replacing an
existing output.

### Release-signer authority

Official release authority is a composition, not a self-authenticating signer file: reviewed protected-
branch governance authorizes policy and signer changes; `release-signers.toml` is the commit-local
allowlist; an allowlisted private key authenticates a tag; hosting/registry administrators control
publication capabilities; and release evidence binds those outputs. Hosting administrators remain the
ultimate technical override. Overrides are governance incidents, not ordinary authority changes.

DC-35 implementation must add a tracked root `release-signers.toml` containing a schema version and
supporting multiple concurrently authorized full uppercase OpenPGP primary fingerprints. Multiple
independent active operators are encouraged when available but are not an M1 precondition. Local Git
configuration, an imported key, or control of an existing signer alone is not authority. No private or
secret key material belongs in the repository, review request, challenge, or evidence.

Every bootstrap, addition, removal, or replacement is an isolated public change completed before RC
finalization. It requires approval by one repository maintainer/administrator and one independent
architect or security reviewer who are two distinct natural persons for every authority transaction.
A proposed signer who is the repository maintainer/administrator may supply the maintainer approval for
their own admission, but cannot supply or impersonate the independent approval. The second person must
independently review the signer proof and authority change. Automation may verify evidence but cannot
occupy either accountable approval identity. Approval by an existing authorized key is recorded when
available but is supplementary and never a veto.

The protected default branch and public review history are the normative authority-change/recovery root;
observed branch-protection review controls or an explicitly reviewed equivalent are required before
release. The equivalent must record an immutable authority-change revision, two accountable approvals,
an observed no-bypass review path or declared administrator-override incident, and the resulting branch
commit identity. It is not an undocumented exception. If that evidence is unavailable, release remains
blocked.

A new signer proves possession with a versioned canonical non-secret challenge that names the Prikk
repository, full primary fingerprint, intended official-release role, immutable authority-change commit
or immutable public review revision, fresh nonce, issuance time, and expiry time. An excluded local
review-request file is not transaction authority. Implementation must pin the byte grammar and freshness
bounds so an old proof cannot authorize a later transaction. The review records the challenge,
transaction identity, full primary fingerprint, and verifier result without private material. Initial
bootstrap uses the break-glass procedure because no prior authorized signer exists.

Every governance transaction has a typed proof state. Bootstrap, addition, and replacement introduce a
fingerprint and therefore require `verified` proof. Removal-only and classification-only transactions
introduce no fingerprint and require `not-applicable` plus a reason. Missing/untyped proof is always
invalid, and `not-applicable` is invalid when the old/new authority comparison introduces a fingerprint.
Implementation derives introduced, removed, and unchanged effects from parsed normalized old/new primary-
fingerprint sets before validating the declared transaction type and proof state; operator labels are not
authority. Authority proof and later release-tag verification use distinct schema fields and fixtures
because they may involve different signers.

Tag creation must explicitly select an authorized full fingerprint. Verification must extract the full
primary signer fingerprint from verifier output, compare it exactly with the allowlist in the peeled
commit, and fail before atomic push on missing authority, ambiguous output, or mismatch. The evidence
snapshot records the fingerprint, authorization path, authority-file blob id, and observed verification
result. Tags created under this policy retain their own authority file through their peeled commits.
Tags through 0.17.7 predate this authority and must not be reported as passing its signer audit. Key
custody, scheduled rotation, expiry/revocation monitoring, attestations, SBOMs, and broader release-key
lifecycle remain DC-43 scope.

### Break-glass and dispute state machine

Loss recovery is triggered when all authorized keys are unavailable or unusable. Compromise containment
is triggered when any authorized key is suspected compromised. Dispute containment is triggered when
any active signer, authority transaction, tag, or release identity is materially disputed. Initial
bootstrap uses the same controls because no prior authorized signer exists. A healthy remaining signer
avoids replacement-key deadlock but does not waive containment, review, or the 72-hour hold. The required
sequence is:

1. Immediately hold new official tags, incomplete/future crate and archive distribution, release-page
   completion, and Pages-complete claims.
2. Open a durable public governance/incident record naming the trigger class, affected fingerprints or
   releases, proposed authority action or classification (including explicit no-authority-change), and
   non-secret evidence.
3. Obtain the two-distinct-natural-person maintainer/administrator and independent architect/security
   approvals required above.
4. If the transaction introduces a fingerprint through bootstrap, addition, or replacement, verify and
   record that signer's non-secret proof-of-possession challenge. A removal-only or classification-only
   transaction records typed `not-applicable` proof and its reason; proof state is never omitted.
5. Make an isolated history-preserving authority/governance commit. Do not force-push, retag, reuse a
   version, or combine unrelated release work. A compromised key may be removed after approvals. A
   classification-only incident may leave the authority file unchanged and records equal old/new blob
   ids explicitly.
6. Keep release publication blocked for at least 72 hours after the bootstrap/recovery/containment
   authority and evidence become public. Any-key compromise/dispute remains held until the affected key
   is removed or the disputed authority/release is classified.
7. Obtain architect/security re-review of the authority/governance commit, branch-governance evidence,
   typed verified or `not-applicable` proof state, containment/classification, and elapsed hold. Only an
   explicit ruling after both the minimum interval and required containment/classification lifts the hold.
8. Record the governance/incident reference, transaction type/action or classification, old/new
   authority-file blob ids (including equality), approvals, hold interval, proof state/reason, and any
   introduced signer fingerprints/verifier results in the first subsequent release evidence.

If no independent reviewer or repository administrator is available, official upstream release
authority remains unavailable; source development and downstream forks remain possible. Existing-
signer approval cannot be required for recovery because total key loss would otherwise be permanent.

After a published-authority dispute, freeze incomplete distribution and future releases and append a
public incident/dispute record and evidence snapshot. Emergency administrator quarantine may contain
exposure before classification, but immutable evidence must precede destructive host action where
technically possible, and quarantine alone never determines official status.

The same two-distinct-natural-person maintainer/administrator and independent architect/security review
classifies the tag against the authority blob in its peeled commit and that blob's governance record as
`valid-at-publication`, `never-authorized/hostile`, or still `disputed`. Only the first two terminal
classifications permit an explicit architect/security hold-lift ruling after the 72-hour minimum;
`disputed` keeps the hold active. A release valid at publication is never silently retagged or replaced;
it may be yanked where supported and superseded by a new version. A hostile tag that never satisfied
official authority is not an official release: administrators may quarantine or remove the hostile ref
while preserving forensic evidence, burning its version/name, publishing the incident, and never reusing
or retagging it.

### Open-source governance boundary

This authority governs only official upstream Prikk tags, official release-page assets, and official
package namespaces. It does not restrict source contributions, review participation, Apache-2.0 forks,
downstream builds, or downstream tags and packages under distinct identities/namespaces. Threshold or
mandatory multi-signature releases are deferred until project scale and tooling justify them.

### Crate-byte identity and completion

For every crate, release evidence records the staged `.crate` SHA-256, the registry-index checksum, the
SHA-256 of bytes fetched after registry visibility, and equality of all three values. A dependent is not
published until each predecessor is visible and its fetched bytes match the registry checksum and
reviewed staged package. A mismatch is `partial`, preserves the published crate identity, blocks all
dependents and `complete`, and requires incident evidence plus a superseding version rather than an
overwrite.

`complete` requires all of these outputs: the fixed source archive and checksum names are attached and
their bytes match recorded digests; every expected crate is registry-visible with equal staged,
registry, and fetched checksums; the release page is published rather than draft; Pages is deployed at
the peeled commit, or a reasoned pre-publication review explicitly records Pages as inapplicable. A
failed or delayed configured deployment remains `pending` or `partial` and cannot be waived afterward
to obtain `complete`. Source archives and `.crate` files are payload artifacts. Checksum files and
release evidence snapshots are integrity/status metadata and are not included in the self-referential
payload-digest rule, although their own names and observed bytes remain immutable once attached.

Evidence JSON uses an explicit schema version, rejects missing required fields and unknown fields, and
requires sequences to start at `001` and remain contiguous with no duplicates or gaps. Each snapshot
after `001` names the prior snapshot and its observed SHA-256; malformed or mismatched predecessor links
are invalid. Allowed overall transitions are absent/pending to `pending`, `partial`, `complete`, or
`superseded`; `partial` to `partial`, `complete`, or `superseded`; `complete` to `superseded`; and
`superseded` is terminal.

Snapshots carry a cumulative append-only attempt list. A later snapshot must retain every prior attempt
in order and add attachment, upload, registry, release-page, and Pages attempts since its predecessor,
including failed evidence-attachment attempts. If no snapshot can be attached, distribution remains
`pending` or at the last valid snapshot. A later `complete` snapshot may not omit an intervening failure.

### Gate and evidence boundary

The design must require, where applicable:

- clean worktree and exact commit/tag identity checks;
- `cargo fmt --all -- --check`;
- `cargo check --workspace --all-targets`;
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`;
- `cargo test --workspace` and `cargo build --workspace`;
- `mdbook build docs` and `git diff --check`;
- package contents/metadata checks and publish dry runs supported by the installed Cargo;
- normalized exact internal dependency checks and isolated-registry package builds;
- authorized signer fingerprint and authority-file identity checks;
- signer-authority approval, branch-governance, transaction-typed proof applicability/result, hold, and
  incident checks;
- staged/registry/fetched crate checksum equality;
- version, lockfile, changelog, roadmap, RFC lifecycle, tag, archive-root, checksum, evidence-snapshot
  sequence/attempt history, completion output set, and valid/forbidden release-state consistency.

The complete deterministic suite runs on the clean finalization commit before tagging. After local tag
creation, identity/archive checks prove that the signed tag peels to that tested commit. After atomic
push, remote identity checks prove that the published tag object and peeled commit are unchanged.

A policy list is not passing evidence. Each review or release record must say which commands were
actually observed, which were unavailable or inapplicable, and the host/filesystem limits. `cargo
audit`, `cargo deny`, crash-reboot testing, registry publication, GitHub release publication, and Pages
deployment must not be reported as passed unless they were executed and observed for that release.

### Documentation placement

The implementation must add `docs/src/reference/release-compatibility.md`, link it from the mdBook
summary and relevant format/recovery references, and add one concise README link. Detailed workflow,
gate, and lifecycle narration belongs in the reference page, not the README. `CHANGELOG.md` remains
released change history, `ROADMAP.md` remains the current schedule, and `rfcs/IMPLEMENTATION-STATUS.md`
remains a current-state snapshot rather than a second changelog.

## Non-goals

- No version bump, tag, package, publish, CI, executable-code, repository-object-schema, or CLI change.
- No compatibility promise, support window, LTS policy, migration tool, or 1.0 commitment.
- No private release-key storage, custody/backup design, scheduled rotation, expiry/revocation
  monitoring, hardware-key mandate, scalable quorum, SBOM, or provenance attestation; those remain
  DC-43 scope.
- No claim that a listed gate passed unless observed for the release under review.

## Dependencies and gates

DC-35 may proceed to scoped policy/documentation implementation but remains held for the single 0.18.0
corrective release. The final page must reflect DC-34's format and compatibility rulings, DC-40's
accepted format-1/format-2 transition, RFC-000's accepted-to-done boundary, and the project's unprefixed
signed-tag convention. `mdbook build docs`, link/status consistency, signer-governance fixtures,
normalized package/local-registry evidence, and a table-driven positive/forbidden release-state audit
are required implementation-review evidence.

## Acceptance criteria

The new reference is reviewed and navigable; the N3 contradictions are corrected; compatibility rules
cannot override identity authority; development build/source metadata, the three Git states, forbidden
mixed rows, and external distribution states are unambiguous; the full suite runs on the finalization
commit; atomic push has no non-atomic fallback; the tag signer is bound to repository-governed commit-
local fingerprint authority; multi-signer admission, bootstrap, total-loss recovery, 72-hour hold,
any-key compromise containment, two-person approval, dispute classification/lift, and upstream/community
boundaries are explicit; proof state is transaction-typed for introduction, removal-only, and
classification-only paths; exact Cargo requirements, package order, staged/registry/fetched crate
equality, immutable asset bytes, exact completion output, and append-only partial-publication evidence
are defined; no published asset requires later bookkeeping to become truthful; and all compatibility
and gate limitations remain explicit.
