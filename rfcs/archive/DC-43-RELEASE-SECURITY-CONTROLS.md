# RFC (proposed) - DC-43 Release Security and Distribution Controls

**Status.** **ARCHIVED 2026-09-13 — superseded by RFC 152.** Its authority half rode on DC-35 (archived with it); its no-authority half was refused by the owner 2026-09-13. Earlier: Proposed; security/architect design review required. **Re-reviewed 2026-09-13:** the signer bootstrap (two natural persons, DC-35) still has not happened and is the owner's; but the design goals split — `SECURITY.md` exists and `cargo audit` gates every push already; **SHA-pinned actions, an SBOM per release asset, and a written dependency/advisory policy need no authority and can start as small tooling**; key custody, rotation and quorum stay behind the bootstrap. **The owner refused the no-authority part 2026-09-13** and ruled that DC-35's rules are to be revised for the project's reality — RFC 152. DC-43 is held here until RFC 152 is accepted, then archived as superseded.

**Status update, 2026-08-27 (evidenced, not a ruling — the schedule position below is stale; the
public-preview prerequisite is unaffected and stands as originally written).** DC-42, the cited
predecessor, does not exist as live work: it was superseded 2026-07-29 into DC-56, DC-57, and
DC-58 (`rfcs/archive/DC-42-PERFORMANCE-MAINTAINABILITY-GATES.md`), and all three have since been
implemented — DC-56 at `8748f00`, DC-57 at `caa2fc2` (accepted 2026-08-02), DC-58 at `6f53da3`
(accepted 2026-07-31; `rfcs/EXECUTION-ORDER.md` lines 121, 127, 189). **There is no longer a
predecessor for this RFC to sit behind.** The record does not support naming a specific successor
in DC-42's place either — nothing establishes that DC-43 should now wait on any one of the three,
or on anything else. The schedule position is unknown, not merely outdated, and should be read
that way rather than corrected to a guess.

**Target milestone.** M2 - required before reconsidering public-preview readiness.
**Schedule position.** ~~Third remaining post-M1 increment, after DC-42.~~ **Stale — see the status
update above.** Completion remains a prerequisite for any public-preview reconsideration; this
program order is not implementation authority.
**Tracks.** Architect review N7.
**Touches.** Vulnerability reporting, dependency policy, SBOM/provenance, release attestations, release-
key lifecycle, registry-owner lifecycle, CI platform policy, and public release documentation.

## Design goals

- Add a tracked `SECURITY.md` with supported-version and private vulnerability-reporting guidance that
  does not expose or request secrets in public issues.
- Select and configure a dependency/advisory policy with explicit allowed licenses, duplicate/version
  handling, and failure ownership.
- Generate an SBOM for release artifacts and record artifact digests without changing an already
  published asset.
- Define provenance/attestation generation and verification for source archives and published crates.
- Define mature release-key custody/backup, scheduled rotation, expiration/revocation monitoring,
  hardware-backed-key policy, scalable maintainer quorum, and registry-owner lifecycle controls without
  weakening DC-35's minimum non-deadlocking bootstrap/recovery authority.
- Make required release gates, optional evidence jobs, and failure handling visible in tracked policy.
- Preserve the project's explicit experimental/no-production warning until an independent review says
  otherwise.

Tool and hosted-service choices require design review before workflow edits. Actions must be pinned to
reviewed immutable revisions according to existing CI policy. Publishing credentials remain external
secrets and must never be copied into RFCs, logs, fixtures, or review packages.

## Non-goals

- No guarantee of vulnerability-free dependencies, paid support SLA, bug bounty, automatic emergency
  release, or production-readiness claim.
- No key-management redesign for repository AUTHOR/MAINTAINER signatures.
- No removal of DC-35's M1 multi-signer capability, two-person authority review, break-glass recovery,
  or official-upstream/community boundary; DC-43 may strengthen those controls.
- No mutation or replacement of released tags, crates, archives, checksums, or attestations.

## Acceptance criteria

Security reporting is usable, dependency policy runs reproducibly, release artifacts have reviewable
SBOM/digest/provenance evidence, release-key and registry-owner lifecycle controls are recoverable and
reviewed, workflow permissions are least-privilege, and independent review finds no unsupported
readiness claim.

---

## Appendix — the implementation handoff, prepared in advance and held (refiled 2026-09-13)

This text lived at `rfcs/handoffs/DC-43-release-security-controls/implementation-handoff-v1.md` from before RFC-000's lifecycle rule; a handoff directory means an accepted RFC, and this one is not accepted. RFC 120 §9.4a's gate (2026-09-13) found it as its only real-tree finding. The content is kept here, unchanged below, until the RFC is accepted and a live handoff is issued from it.

### DC-43 Release Security and Distribution Controls - Implementation Handoff

**Prepared in advance.** Implementation may **not** begin until `rfcs/proposed/DC-43-…` moves to
`rfcs/accepted/` through design review **and** its security review, and it should consume the settled
post-DC-52 tooling gate rather than extend the retained Python engine.
**Authored by** the architect (function-designer role). Implementation review remains independent.
**Size:** medium. Design work can proceed without credentials; anything touching signing keys or registry
credentials cannot.
**Touches:** repository security metadata, release artifact controls, CI release workflow. No product
code.

## Why this exists

Architect review N7: the repository publishes crates and documentation but has no tracked `SECURITY.md`,
no SBOM or release-attestation workflow, no dependency policy configuration, and no documented
vulnerability-reporting path. The README's experimental warning is appropriate and should stay — but a
project asking users to trust signed, verifiable history needs a route for someone to report a problem in
it.

## Scope

1. **Vulnerability reporting.** A tracked `SECURITY.md` with a reporting channel, expected response
   posture, and explicit scope (what is in and out given the experimental status).
2. **Dependency policy.** Configuration making the current implicit posture explicit — the workspace has
   a deliberately minimal production set (`ed25519-dalek`, `getrandom`, `rustix`) and 169 locked packages.
   `cargo audit` already runs; this makes the *policy* reviewable, not just the scan.
3. **SBOM and provenance.** Generated per release artifact, verifiable offline.
4. **Distribution verification.** How a consumer verifies what they fetched matches what was published.

## Sequencing constraints

- **Consume, do not extend.** DC-45's consolidation made the Rust command authoritative; DC-52 retires the
  Python path. DC-43 must build on the settled gate. Extending the retained Python engine would re-create
  the dual-ownership debt DC-45 spent eleven rounds removing.
- **Interacts with DC-51.** If DC-51 has landed, the dependency-placement gate is the enforcement point
  for part of the dependency policy — reference it rather than duplicating it.
- **Release-lane boundary.** DC-43 defines and builds controls. It does **not** perform a release, request
  a fingerprint, bootstrap a signer, or activate the release lane. Artifact-signing controls may be
  designed and tested without ever exercising a real signer.

## Traps

- **Do not** let SBOM/provenance work drift into signing-key handling. Signer governance is DC-35's, the
  allowlist is empty and fail-closed, and touching it is a release-lane action.
- **Do not** add a CI job without the accompanying classifier amendment — `.github/workflows/ci.yml` is a
  governed procedure file, and any new `run:` command must match an accepted production or
  `boundary-check`/`reference-check` fail closed. This is the DC-46 pattern.
- **Do not** upgrade the public posture as a side effect. Adding a `SECURITY.md` does not make the project
  production-ready, and the README's experimental warning stays.
- SBOM tooling may add dependencies. If it does, the DC-41 stage-3/4 discipline applies: dev-only
  placement, MSRV re-verified on the integrated workspace, `Cargo.lock` re-freeze recorded, advisory
  surface reported.

## Definition of done

- `SECURITY.md` tracked, with reporting channel and honest scope.
- Dependency policy configuration present and enforced or explicitly advisory (state which).
- SBOM generated per release artifact and verifiable offline.
- Distribution verification documented end-to-end from a consumer's position.
- Any new CI command carries its classifier amendment in the same increment.
- No signer, release-lane, tag, or publication action taken.
- Full gate set green (`rfcs/EXECUTION-ORDER.md` §6.8).

## Submit with

Diff; evidence note covering each of the four scope items and their verification method; any dependency
addition with placement, MSRV, and lockfile evidence; gate output; explicit statement that no signer or
release-lane state changed and no public readiness claim was upgraded.
