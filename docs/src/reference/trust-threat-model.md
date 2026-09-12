# Trust and Threat Model

This page is the authoritative current-state reference for Prikk's trust and threat model. It
describes the current implementation and is grounded in the code, released RFCs, and implementation
status records listed in the anchor table at the foot of the page.

## Core Caveats

- Prikk is early implementation software and is not a production Git replacement.
- `.prikk/` is Prikk's native repository format and is not Git-compatible storage.
- Ref pointers are mutable, not roots of trust.
- Maintainer trust is repository-local with the current minimal `required = 1` policy.
- `verify` is not a global trust proof.
- MAINTAINER key revocation exists (`prikk trust maintainer remove`); there is no key rotation, hardware
  signing, remote trust, a distinct peer/sync trust framework, or stable migration policy yet, and no
  AUTHOR-identity revocation. Sync and tag adoption make no new trust decision of their own — every
  trust check either resolves against locally-held history or the receiver's own already-adopted keys;
  see [Trust Roots and Roles](#trust-roots-and-roles) below for the tag-adoption specifics.
  For AUTHOR keys specifically (DC-53 Stage 2): one `key_id` is permanently bound to the first public
  key ever recorded for it in a given repository; attempting to sign under the same `key_id` with a
  different key — whether from a genuine rotation attempt or an impersonation attempt — is refused
  identically, and is indistinguishable as the reason for the refusal.
- Durability and recovery claims are supported by current unit and integration tests, not by a
  completed crash-matrix or fuzzing campaign.
- Repository *mutation* is exercised by project gates on Linux, macOS, and Windows (DC-87 Stage 2).
  Windows' anchoring guarantee is weaker than Linux/macOS in one stated way — see
  [platform support](./platform-support.md) for the exact gap and which of the nine durability
  guarantees are held, weaker, or documented no-ops there. Read-only commands are CI-gated on macOS
  and Windows too — see [platform support](./platform-support.md).

Changes that alter trust, threat, verification, signature, key-management, durability,
platform-support, or production-readiness claims require architect review or accepted RFC/DC coverage.
The local persistence and crash-recovery boundary is covered by the
[durability and crash recovery](./durability-recovery.md) reference. The current `verify` / `doctor`
diagnostic catalog is covered by the
[integrity and recovery diagnostics](./integrity-recovery.md) reference. Current operator setup for
environment key input and repository-local maintainer trust is covered by the
[security and signing setup](../guide/security-setup.md) guide. Physical trust-store paths and other
`.prikk/` authority boundaries are covered by the
[repository layout and authority](./repository-layout.md) reference. Repository path validation and
worktree write-safety limits are covered by the
[path and worktree safety](./path-safety.md) reference.

## Trust Roots and Roles

Current signing uses role-bound Ed25519 signatures. The signature preimage binds the algorithm, object
type, object id, signer role, and key id. Signer roles include AUTHOR and MAINTAINER. Ed25519 signing
and strict verification live in `prikk-crypto`; trust stores, key persistence, rotation, revocation,
and policy are outside that crate.

AUTHOR signatures identify the key used by the authoring path for Patch envelopes. Production commit
and rollback-draft authoring use real Ed25519 AUTHOR signatures. **Since DC-53 (2026-08-18) Prikk does
maintain a repository-local AUTHOR key-material store and `verify` checks every reachable Patch's AUTHOR
signature against it.** Prikk still implements **no** AUTHOR revocation, rotation, expiration, or
identity policy.

**What AUTHOR key material proves, stated precisely (DC-53 Stage 2).** A repository records each
`key_id`'s public key the first time it observes a Patch signed under that name — trust-on-first-use,
and that first observation is **not** itself verified against anything. Every subsequent appearance of
the same `key_id` is checked against the key recorded at first contact, and one `key_id` is permanently
bound to one public key for the life of that repository. **What this proves is "the same `key_id` has
always signed under this name here" — not "this author's claimed identity is genuine."** A reader must
be able to tell *"prikk verified this author"* apart from *"prikk verified this author is the same one
as last time"*, because only the second is true.

When AUTHOR key material travels in a bundle (`prikk bundle export`/`import`, DC-53 Stage 2), the same
limit applies with one further step: a transported key is supplied by the sender. A signature that
verifies against a key which arrived in the same bundle proves only that the two are internally
consistent — an attacker who re-signs a Patch with their own key and ships that key in the bundle
produces a bundle that verifies perfectly. Import records transported material under the same
first-contact rule as local material; it performs no additional check of who actually holds the key.
Transport does not weaken the maintainer signature's own role in DC-78's exchange claim — a receiver
still relies on that signature for the decision to include imported patches at all; AUTHOR verification
adds continuity of authorship on top of it, and does not replace it.

MAINTAINER signatures identify publication objects. `seal` uses real role-bound Ed25519 MAINTAINER
signatures for Block, RefState, and RefUpdate envelopes and verifies the signer against the local
maintainer trust policy before publishing. The same gate — the local operator's own signer checked
against the local trust policy before any object or ref write — applies to every operation below
(`prikk tag create`/`prikk branch create`/`prikk branch close` since commit `053e442`):

<!-- rfc118-stage3-gated-operations:start -->
`seal`, `merge`, `sync build`, `sync seal`, `sync adopt-tag`, `prikk tag create`,
`prikk branch create`, `prikk branch close`.
<!-- rfc118-stage3-gated-operations:end -->

This list is derived, not transcribed: it names every `GatedOperation` variant `verify_signer_trusted`
accepts (`crates/prikk-store/src/trust.rs`), and a test binds it bidirectionally against this page
(RFC 118 stage 3) so it cannot silently drift. **That binding proves only that this page and the enum
agree on which operations gate — not that every operation which *ought* to gate does.**
`prikk tag create` itself published maintainer-signed objects for months before `053e442` added this
exact check, and no enumeration of gated operations can catch an operation that is absent from it. That
is a standing, unenumerable risk this list cannot close, and it is recorded as an open item, not implied
away by the list's own precision.

**Tag adoption is the receiver's own signed act (RFC 117 T4), never conjured from someone else's
assertion.** `sync build`/`accept` can move a Tag object into the received namespace, but arrival is
not adoption: **trust does not expand on receipt.** A received tag's own MAINTAINER signature is
checked and its outcome (`Sound` against an adopted key, or `Unverifiable` against one this repository
has not adopted) is reported by `sync tags` — but that outcome never gates anything. An `Unverifiable`
tag is stored and reported exactly like a `Sound` one. Nothing advances until an operator explicitly
runs `sync adopt-tag <name>`.

`sync adopt-tag` creates a **new, local** `Tag` object, signed under the receiver's own maintainer key —
not a copy of the sender's. The sender's tag and the receiver's tag are different objects sharing the
same global identity: a locally re-signed tag carries a different `author_key_id`, and therefore a
different signature and a different object id, even though both name the identical patch set (a tag
names a patch set, not a block, which is why a received tag's target has to be *resolved* against the
receiver's own local blocks rather than pointed to directly — blocks diverge across repositories, patch
sets do not). Adoption requires the receiver's own signer to be locally trusted — the same
trust-on-first-use maintainer policy described above, checked the same way `seal` checks it. **Adoption
does not verify who the sender is.** It does not check the received tag's own signature outcome at
all: whether the sender's tag was `Sound` or `Unverifiable` has no bearing on whether adoption succeeds,
because adoption asserts what the *receiver* now holds and signs under its own key, not anything about
who sent it.

**Adoption refuses rather than guesses.** It refuses if no received tag carries the requested name, if
more than one received tag claims that name, if the named patch set is not held locally yet (not enough
of this repository's history has been synced), or if the patch set resolves to more than one local
block. Each refusal is a person's decision not to act on an unresolved question — not a cryptographic
guarantee that the correct answer was found.

**Repositories are anonymous. Identity lives in signer keys and in patch ids — never in a
repository.** This is a settled property of the shipped design, not an open question or a deferred
non-goal: prikk has no peers, only artifacts. `crates/prikk-cli/src/sync.rs`
opens with *"No network. No socket. No new dependency"* — every sync subcommand reads and writes
files, and there is no session, no remote party, and nothing to authenticate as a repository in the
first place. Checkable, not merely asserted:

- `RecognitionClaimPayload` carries `block_id`, `patch_ids`, and `parent_block_ids` — content ids
  only, never an originator field.
- `SyncSummaryRefEntry` carries `ref_name`, `digest`, and `patch_count` — again, no originator field.
- Tags travel and are adopted under the receiver's own key, above, not the sender's.
- All trust is local (`trust maintainer add`), gated through `GatedOperation`.

**An artifact asserts nothing binding.** A claim never gates admission and never confers trust: the
receiver applies the claimed order and either it produces a valid state — which the receiver then
seals under their own key, `verify_signer_trusted` unchanged and still gating — or it does not. A
hostile or simply wrong claimed order cannot forge a state. So "what is a remote permitted to
assert?" has an answer: nothing that binds the receiver. The receiver is the sole authority over its
own store.

**What this forecloses.** There is no repository identifier to spoof, no peer to impersonate, and no
origin field a receiver could be fooled by, because none of the three exists. That is a security
property this design has, not a gap in it — the "global identity trust" and "remote trust" entries
under Threat Boundaries' non-goals list below describe capabilities not yet built, not this: there is
no repository identity for a future increment to eventually add trust *around*, because the design
never creates one to begin with.

## Key Input and Local Trust Store

Current key input is intentionally minimal. The CLI reads AUTHOR key material from
`PRIKK_AUTHOR_KEY_ID` and a seed **file** (`PRIKK_AUTHOR_SEED_FILE`, else the key directory's
`author.seed`), and MAINTAINER key material the same way. A seed never travels through the
environment. The seed values are caller-provided 32-byte
Ed25519 secret seeds encoded as 64 hex characters. **Prikk does not provide local secret storage**,
but it does provide key generation and public-key derivation: `prikk key generate` draws a fresh
seed from the OS CSPRNG, `prikk key public [--seed-file <path>]` derives a public key from a seed
already held, and `prikk setup` composes both with `init` and `trust maintainer add` into one
command (RFC 135) — see [First Run](../guide/first-run.md). For the current setup workflow and
seed-handling warnings, see the [security and signing setup](../guide/security-setup.md) guide.

The local maintainer trust store supports a set of repository-local adopted MAINTAINER keys, with
`required = 1` continuing to mean any one adopted key's signature suffices. `prikk trust maintainer add`
adds a new key id to the set, or idempotently confirms an already-adopted id's matching key; it refuses
to replace an adopted id's key with a different one. This refusal is a trust-on-first-use rule: the
first public key seen for a key id is the one trusted for that id, permanently, until an operator
removes it out-of-band. There is no remote trust distribution.

`prikk trust maintainer list` and `prikk trust maintainer check --key-id <ID>` (RFC 138) read that
same set without changing it: `list` enumerates every currently adopted key and `check` answers
whether one specific id is adopted, both with `--format json` for a machine-readable answer. Both
answer *"whose signatures does this repository accept on objects"* — **object trust, not ref
authority: adopting a key never lets it move a ref by itself.** `check` exits `0` whichever way the
question resolves, since a negative answer is a successful query, not an operational failure — `1`
and `2` are reserved for an unreadable policy and a missing or malformed `--key-id`, respectively.

## What Seal Checks

Seal requires `--allow-no-audit`, a valid local branch ref, a non-empty active WAL, valid active ref
metadata matching the requested ref, and no trailing partial WAL bytes. It verifies that the configured
MAINTAINER signer matches the repository-local trust policy before publication. It then persists Patch
objects, signs and writes the Block and RefState, durably appends the ref pointer as the commit point, appends
exactly one signed RefUpdate, confirms pointer/log agreement, and clears active state. Signer-backed
retry is also the only authority that may finish an exact interrupted publication.

Current seal does not run audit plugins, evaluate attestation policy, perform semantic merge, publish
multi-parent merge Blocks, or provide remote trust distribution.

## What Verify Checks

`prikk verify` is read-only. It checks persisted object placement and identity, envelope decoding,
Block references, ref pointer/log consistency, active WAL records, active WAL metadata health,
rollback-draft structure for active and sealed rollback-marked Patches, and publication trust for
Block, RefState, RefUpdate, and locally-published Tag envelopes against the repository-local
maintainer trust policy. A received, not-yet-adopted tag's own signature is deliberately not part of
this check — see [Trust Roots and Roles](#trust-roots-and-roles) above.

`verify` does not prove that a repository is globally trustworthy. **It does check every reachable
Patch's AUTHOR signature against recorded key material (DC-53), and fails when one does not verify or
when a `key_id`'s recorded material contradicts itself** — but that is continuity, not identity. It does
not enforce historical PKI semantics, AUTHOR revocation, rotation, expiration, threshold policy beyond
`required = 1`, remote policy, hosted identity, or complete crash-proof durability.

## Rollback-Draft Boundary

Rollback drafts are Patch objects whose payload purpose is `PatchPurpose::RollbackDraft`. Active
rollback-draft verification requires exactly one active WAL record, rejects trailing partial WAL bytes,
requires a rollback-draft Patch purpose, requires an AUTHOR Ed25519 signature, rejects the legacy
placeholder marker key id, requires 64-byte signature payloads, and compares the active payload with
the inverse Patch derived from the current ref.

This is structural and semantic validation for the supported rollback subset. It is not rollback
authorization and does not publish rollback refs. **Repository-wide AUTHOR verification is performed by
`verify`, not here** (DC-53); this path's own checks are unchanged.

## Threat Boundaries

Current protections target local repository corruption, malformed persisted data, wrong object
placement, ref pointer/log drift, active-WAL ownership drift, unsigned or untrusted publication
objects, and legacy rollback marker signatures. Diagnostics should avoid raw text spans, replacement
text, blob bytes, absolute host paths, `.prikk` private paths, signer secrets, key material, and
arbitrary object debug dumps.

**Not built yet, not refused** — this page's own Core Caveats already say so ("no key rotation,
hardware signing, remote trust, a distinct peer/sync trust framework, or stable migration policy
**yet**"): global identity trust, remote trust, hardware signing, key lifecycle management (with
one exception — rotating an AUTHOR key under its *own* `key_id` is refused, indistinguishably from
impersonation, per Core Caveats above; a new `key_id` is not restricted), stable repository-format
migration, and multi-maintainer thresholds (the trust store's `required` policy already supports a
threshold; only `required = 1` is deployed today).

**Genuinely unclear from this project's own record** — no statement here or elsewhere on this page
calls these either built-later or ruled out, so neither column would be honest: hosted forge
semantics, production audit policy, and plugin execution.

**What this page's own repository-identity settlement (above) refuses, by contrast, is
permanent** — see [Non-Goals](non-goals.md) for the confirmed refused set and what "refused" means
as distinct from everything on this list.

## Claim-to-Source Anchors

| Claim | Source anchors |
|---|---|
| Ed25519 is the only current signing and verification algorithm. | [`prikk-crypto`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-crypto/src/lib.rs), [`signature.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-object/src/signature.rs) |
| Signature preimages bind algorithm, object type, object id, signer role, and key id. | [`signature.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-object/src/signature.rs), [`author_signing.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/author_signing.rs), [`maintainer_signing.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/maintainer_signing.rs) |
| AUTHOR signing is real Ed25519 on Patch envelopes, not a placeholder. | [`author_signing.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/author_signing.rs), [`node_authoring.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/worktree_patch/node_authoring.rs), [DC-10](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-10-ROLLBACK-DRAFT-SIGNING.md) |
| AUTHOR *private* key material comes from environment variables and is never persisted by Prikk. **The public half is persisted** in the repository-local author-key container, recorded at authoring time (DC-53), because an Ed25519 signature cannot be verified without it. | [`main.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/main.rs), [`author_signing.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/author_signing.rs), [implementation status](https://github.com/prikk-vcs/prikk/blob/main/rfcs/IMPLEMENTATION-STATUS.md) |
| MAINTAINER publication signing is real Ed25519 and role-bound. | [`maintainer_signing.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/maintainer_signing.rs), [`seal.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/seal.rs), [DC-11](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-11-MAINTAINER-TRUST-STORE.md) |
| A received tag's MAINTAINER signature outcome is checked and reported but never gates; nothing advances until `sync adopt-tag` runs. | [`tag_travel.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/tag_travel.rs), RFC 117 T4 |
| `sync adopt-tag` creates a new, locally-signed `Tag` object distinct from the sender's, gated on the receiver's own locally-trusted signer, not on the sender's signature outcome. | [`tag_travel.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/tag_travel.rs), [`trust.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/trust.rs), RFC 117 T4 |
| Tag adoption refuses on a name miss, a name collision among received tags, an unheld patch set, or an ambiguous patch-set resolution — it never picks. | [`tag_travel.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/tag_travel.rs), [`patch_set_digest.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/patch_set_digest.rs) |
| Maintainer trust is repository-local, held as a set of adopted keys, with `required = 1` meaning any one adopted key's signature suffices. | [`trust.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/trust.rs), [`layout.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/layout.rs), [DC-11 FDD-04 handoff](https://github.com/prikk-vcs/prikk/blob/main/rfcs/handoffs/DC-11-maintainer-trust-store/fdd-04-update.md) |
| Seal validates the maintainer signer against local trust before publication. | [`seal.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/seal.rs), [`trust.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/trust.rs) |
| `prikk trust maintainer list`/`check` read the adopted-key set without changing it; `check` exits `0` for both a positive and a negative answer. | [`main.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/main.rs), [`output/trust.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/output/trust.rs), [`trust.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/trust.rs) (`load_maintainer_trust_policy_or_empty`) |
| Verify checks publication trust for Block, RefState, RefUpdate, and locally-published Tag envelopes; a received, unadopted tag is deliberately exempt. | [`verify.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/verify.rs), [`trust.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/trust.rs) |
| Verify enforces repository-wide AUTHOR verification (DC-53): every reachable Patch's AUTHOR signature is checked against recorded key material, one `key_id` binds to one public key, and material travels with a bundle export (introduced at `PBNDL002`, still carried unchanged at the current `PBNDL003`). It remains trust-on-first-use — continuity, not identity. | [`verify.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/verify.rs), [`rollback_verify.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/rollback_verify.rs), [implementation status](https://github.com/prikk-vcs/prikk/blob/main/rfcs/IMPLEMENTATION-STATUS.md) |
| Rollback-draft verification is structural and semantic for the supported subset only. | [`rollback_verify.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/rollback_verify.rs), [DC-14](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-14-ARBITRARY-SPAN-TEXT-INVERSE-ROLLBACK.md), [DC-14 FDD-04 handoff](https://github.com/prikk-vcs/prikk/blob/main/rfcs/handoffs/DC-14-arbitrary-span-text-inverse-rollback/fdd-04-update.md) |
| Active WAL metadata integrity is part of verification and doctor diagnostics. | [`verify.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/verify.rs), [`doctor.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/doctor.rs), [DC-15](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-15-ACTIVE-SESSION-INTEGRITY-HARDENING.md) |
| Durability and platform claims remain limited by current test evidence. | [DC-24 baseline recap](https://github.com/prikk-vcs/prikk/blob/main/rfcs/handoffs/DC-24-data-model-trust-threat-docs/baseline-recap.md), [DC-24](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-24-DATA-MODEL-TRUST-THREAT-DOCS.md) |

## Provenance

This reference consolidates released records through DC-23 and DC-24. It supersedes stale
v0.2.0-era notes that described MAINTAINER signing as deferred; the current released code signs
publication objects with real MAINTAINER Ed25519 signatures and verifies them against local trust.
DC-26 moved this current-state reference from `rfcs/fdds/` into the published book without changing
code, schema, trust, or CLI behavior.
