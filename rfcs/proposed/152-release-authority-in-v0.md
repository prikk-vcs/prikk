# RFC 152 — Release authority in v0: revising DC-35 for the project as it is

**Status.** **PROPOSED 2026-09-13 by the architect**, on the owner's ruling of the same day: *"DC-35 is
useless to me. The rules, too. However, it may be only in v0 development. At least, the person who made
it did not know the project reality. Old or useless rules should be revised or fixed to be improved."*
Awaits the owner's acceptance. Nothing here edits `release-signers.toml` — the owner's standing
constraint that the file must not be changed holds until the owner says otherwise.

## 1. What DC-35 requires, and what has actually happened

DC-35 (accepted 2026-07-15) requires that every release-signer transaction — bootstrap, addition,
removal — be an isolated public governance change approved by **two distinct natural persons**, with a
72-hour hold and a break-glass procedure; `release-signers.toml` is the commit-local authority file and
reads `authorized_primary_fingerprints = []`. `EXECUTION-ORDER.md` §3 calls the release lane *parked*
until that bootstrap; criterion 4 of the early-implementation board (`MILESTONES.md`) is *"the signer
bootstrap has never happened"* and is the one criterion still holding the badge.

Meanwhile the project has shipped **twenty-odd releases, 0.22.0 through 0.42.0**, every one the same
way: the owner authorizes the cut; the architect prepares (since 0.41.0, with the dev team's sweep), gates,
waits for CI, signs the tag with the repository's configured key, and pushes it; the Release workflow
builds and publishes assets; the owner authorizes crates.io publication per release; the architect
publishes and verifies. **One natural person holds every key and every word.** DC-35 described a project
with a maintainer quorum; this project has an owner.

## 2. The rule for v0, stated as what is true

1. **Release authority is the owner.** A cut is authorized by the owner's word; publication to crates.io
   by a second word, per release. Both are recorded in `ROADMAP.md`'s release table (already the practice).
2. **The architect executes**: readiness (with the dev team's sweep per the release-prep template), the
   gate set on the exact release commit, CI green, the signed annotated tag, the push, asset
   verification with the sweep's smoke script, publication in dependency order, index verification.
3. **The tag key is the repository's configured signing key** (`tag.gpgSign`, `user.signingkey`); every
   release tag is verified with `git tag -v` before it is pushed. That is the whole signer set in v0.
4. **`release-signers.toml` stays as it is and means what it says: no multi-signer policy exists in v0.**
   Its emptiness is a statement, not a fault waiting for a bootstrap. The tooling that reads it
   (`release_notes.rs`, the oracle self-test) already passes on the empty set — every release proves it.
5. **The two-person quorum, the 72-hour hold, the break-glass procedure, and criterion 4 are v1
   governance**: they apply on the day the project has two maintainers who each hold a release key, and
   not before. Until then they are not rules this project can follow, and a rule that cannot be followed
   is not protecting anything.

## 3. What this RFC changes when accepted

- `EXECUTION-ORDER.md` §3: the release lane is not parked; §2 is the lane, and it has been running since
  0.22.0. Criterion 4's text becomes *"v1 governance; not a v0 gate."*
- `MILESTONES.md`: criterion 4's row — **the owner edits this file or instructs the edit**; this RFC
  lists the change and does not make it.
- DC-35 is amended by reference (a dated section pointing here), not rewritten; its transaction design
  is kept for v1.
- **DC-43** (release security and distribution controls) is superseded in its current shape: its
  authority half rides on DC-35 and moves to v1 governance with it; its no-authority half (SHA-pinned
  actions, SBOM, a written dependency policy) **was refused by the owner 2026-09-13** and is not proposed
  again here. DC-43 moves to `rfcs/archive/` with that note when this RFC is accepted.
- `docs/src/reference/` release/compatibility text that promises a two-person signer set is corrected to
  §2's rule, so the documentation says what the project does.

## 4. What this RFC does not do

It does not weaken any integrity property of prikk itself: object signing, trust adoption, seal
authority and `verify` are untouched. It changes only who may say "release", from a quorum the project
does not have to the person who has said it every time.
