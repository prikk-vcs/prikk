# One object, several signers — handoff v1

**Live 2026-09-16.** RFC 156 (`rfcs/accepted/156-one-object-several-signers.md`), accepted with its §7.3 bound and the
architect's reading of rule 1 (in its Status). **0.45.0 item 0**, before the key-id collision fix. Read the RFC first;
this handoff orders the work and names what the RFC leaves to implementation.

**Two stop points.** Stage 0 is measurement only and ends in a report. Stages 1–5 follow only on the architect's word.

## Stage 0 — the compatibility gate (measure, then stop)

RFC 156 §5: **does a 0.44.0 binary still open, read and verify a repository holding a superseding record?** The answer
decides whether this ships as a minor release or becomes a format change that needs RFC 155's carry-forward first.

1. **Build the state without building the feature.** A test-support helper, gated like the existing ones, appends a
   second record for an existing id — its envelope carrying the stored signatures plus one more valid signature, in
   canonical order — to the container and the index, bypassing the write decision. Two cases:
   - a **Patch** with two valid AUTHOR signatures, by two keys both recorded in the author-key index;
   - a **Block** with two valid MAINTAINER signatures, by two adopted keys.
2. **Run the released 0.44.0 binary** (the GitHub asset, checksum-verified — not a local build) against each repository:
   `verify`, `verify --format json`, `log --format json`, `show <id> --format json`, `status`, `doctor`,
   `bundle export`, then `bundle import` of that bundle into a fresh 0.44.0 repository.
3. **Also run the same set with the current tree's binary**, so the report shows the two side by side.
4. **Report:** per command, exit code and the relevant output. Plus one ruling-relevant fact per case: **which
   signature 0.44.0 reports as the author or signer**, and whether 0.44.0's container scan treats two records for one
   id as a finding.
   Report: `.git-exclude/review-request/one-object-several-signers-stage0-report-v1.md`. **Stop there.**

## Stage 1 — `verify` checks every signature of every role

Independent of everything after it, and worth landing even if Stage 0 changes the schedule. Ten sites select one
signature by role, and each must be read and decided:

| site | selects |
|---|---|
| `author/author_signing.rs:76` (`require_author_key_id`) | first AUTHOR |
| `author/author_key_index.rs:468`, `:496` (`verify_author_signature_against_material`) | first AUTHOR |
| `bundle.rs:461`, `:736` | MAINTAINER filter; first AUTHOR |
| `patch_exchange/accept.rs:227` | first by role |
| `patch_exchange/artifact.rs:195` | first by role |
| `recognition_claim.rs:151` | first by role |
| `rollback/verify.rs:222` | first AUTHOR |
| `tag_travel.rs:75` | first MAINTAINER |

For each, say whether it is a **verification** site (must check every signature of the role, and fail on any invalid
one), an **attribution** site (must report every signer: Stage 4), or a **selection** site where one signature is
genuinely enough (say why). **`verify` fails on an envelope any of whose signatures is invalid.**

## Stage 2 — the union write rule

The store stops refusing a second envelope for an id; it merges.

1. **The decision, in one place.** Replace the "same id, different bytes → `Integrity`" branch of
   `foundation/index.rs::decide_write_outcome` with a merge decision:
   - `AlreadyPresent` when the incoming signatures are a subset of the stored ones;
   - `Merge(union)` when they add any;
   - a refusal when the **payload** differs under the same id (that is corruption, and stays `Integrity`).

   `ObjectWriteSession::check_write` (0.44.0) keeps sharing it, so a merge is decided before any write.
2. **What may enter the union** (RFC 156 §4 rules 1–3):
   - every incoming signature is verified first, and an invalid one refuses the whole import or exchange, with nothing
     written;
   - a MAINTAINER signature enters only if an adopted key made it; otherwise it is dropped and reported;
   - an AUTHOR signature enters if it verifies against its key id's material under trust-on-first-use, with the
     binding rules unchanged.
3. **One superseding record**, the canonical union, appended to the container with its index entry; nothing is
   rewritten. The last-entry-wins lookup then reads the union.
4. **Concurrency.** Two writers merging into the same id must not lose each other's signatures. Decide and append under
   one lock scope that re-reads the stored record before appending, and show that holds (§Controls 6). If the existing
   leaf object-store lock cannot give that without breaking the documented lock order, **stop and report** the order.
5. **Every writer goes through it** — local writers and imports alike — so the collaboration case merges in both orders
   (seal then import, import then seal).
6. **Refusals before writes**: 0.44.0's rule holds, so a merge refused for any reason leaves the repository byte for byte
   as it was.

## Stage 3 — the bound (RFC 156 §7.3)

- **Counted:** every AUTHOR signature that entered a stored object through `bundle import` or `sync accept`.
- **Never counted, never refused:** signatures written by local writers (the Status's reading of rule 1), and
  MAINTAINER signatures by adopted keys.
- **Limit 4 per object, in total across all imports**, as one named, documented constant. At the limit the next counted
  signature refuses the import or exchange, naming the object and that its set is full, with nothing written.
- **Where the count lives:** it must be derivable from the stored object and repository state, never a second store of
  truth. Say how you derive "arrived through an import". **If it cannot be derived without new persisted state, stop
  and report** before adding any.

## Stage 4 — attribution

- `show` and `log`, prose and JSON, report **every** AUTHOR signer (and every MAINTAINER signer where they report one).
- JSON gains `author_key_ids` (and the maintainer equivalent where applicable), additive within each report's schema
  version. **`author_key_id` keeps its current meaning** — the first in canonical order — and its doc says so.

## Stage 5 — docs and CHANGELOG

- `docs/src/guide/sync.md`: the documented refusal (*"importing the sender's bundle of that ref refuses with `existing
  container record for <id> differs from candidate`"*) becomes the merge.
- `docs/src/reference/trust-threat-model.md`: several signers per object, the bound, the residual.
- `docs/src/guide/backup-restore.md`, and every page that says one envelope per id.
- **CHANGELOG:**
  - `### Changed`: one object may carry several signers;
  - `### Fixed`: `verify` checks every signature;
  - `### Added`: `author_key_ids`;
  - naming the limitation GHSA-px5q-233r-6hq5 described.

  **The advisory itself is updated by the architect and the owner at release**, not in this round.

## Controls (each must be able to fail)

1. **Order independence:** one object under two valid signatures, imported in both orders, gives byte-identical stored
   envelopes. Perturb: keep the first.
2. **An honest copy after a foreign one** is stored, and both signers are reported.
3. **An invalid incoming signature** refuses with nothing written (every `.prikk` file compared).
4. **A non-adopted MAINTAINER signature** is not stored and is reported.
5. **`verify` fails on an envelope whose second AUTHOR signature is invalid.** Perturb: check the first only.
6. **Concurrency:** two merges into one id, interleaved by a seam, keep both writers' signatures. Perturb: append
   without re-reading.
7. **The collaboration case, both orders:** receiver seals then imports the sender's bundle, and imports then seals —
   merged, `verify` clean, both maintainer signatures reported.
8. **The bound:** four counted signatures accepted; the fifth refuses, named, nothing written; a local writer's signature
   still merges into a full set; an adopted maintainer's signature still merges; keys recorded by an earlier import
   still count (RFC 156 §7.2's exemption must not return).
9. **A payload mismatch under one id** still refuses as `Integrity`.
10. **Stage 0's gate**, kept as a test where it can be (a fixture written by the current tree, read by the oldest
    supported reader available to the suite); otherwise stated in the report as measured-only.

## Discipline and report

- The full gate set on the final commit, both cross-target clippies included.
- `cargo +1.85.0 check` in the pre-commit run.
- The staged list before every commit.
- Never push, tag or publish.

**Reports:** Stage 0 as above. Stages 1–5:
`.git-exclude/review-request/one-object-several-signers-report-v1.md`.
