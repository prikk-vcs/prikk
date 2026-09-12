# Security and Signing Setup

This guide describes the current operator setup for Prikk signing and repository-local maintainer
trust. For the full security model, see the [trust and threat model](../reference/trust-threat-model.md).
For verification diagnostics after setup, see
[integrity and recovery diagnostics](../reference/integrity-recovery.md). For the physical trust-store
paths, see [repository layout and authority](../reference/repository-layout.md).

## Core Caveats

- Prikk is early implementation software and is not a production Git replacement.
- Key input is a seed **file** per role — a path you name, or the role's file in your key
  directory — and intentionally minimal. `prikk key status` reports what is in effect.
- Seeds are secret key material. Prikk does not store, encrypt, rotate, revoke, expire, or back up
  private keys — `prikk key generate --out` writes one, once, to a path you name, and never reads it
  back or manages it afterward (see [First Run](first-run.md)).
- `prikk key generate` draws a fresh seed from the OS CSPRNG; `prikk key public` derives a
  public key from a seed you already hold; `prikk setup` composes both roles plus
  `trust maintainer add` into one command. External tooling is no longer required, though nothing
  stops you from bringing your own matched seed/public-key pair instead.
- Maintainer trust is repository-local, held as a set of adopted MAINTAINER keys with `required = 1`
  (any one adopted key's signature suffices), and enforces trust-on-first-use per key id.
- AUTHOR signatures are real Ed25519 signatures, and there is **no AUTHOR trust policy**: no list of
  adopted author keys, and nothing to adopt one into. A patch signed by an author key this repository
  has never seen commits, seals, and passes `verify` — verified by doing it, not inferred.
- What *is* enforced for AUTHOR keys is **one key id, one public key**. Committing under a key id the
  repository has already recorded, with different key material, is refused:

  ```
  error: integrity error: author key_id author already has a different recorded public key (…); one
  key_id binds to one public key -- this looks like a key-rotation attempt, which is not supported and
  is indistinguishable from impersonation
  ```

  That is an impersonation guard, not a trust decision: it stops one identity being silently reused by
  a different key, and says nothing about whether that identity is *trusted*. `verify` reports
  `unverifiable author signatures: 0` when every author signature checks out against the material
  recorded for its key id.
- MAINTAINER key revocation exists (`prikk trust maintainer remove`); there is no key rotation, hardware
  signing, remote trust, sync trust, hosted identity, multi-maintainer threshold policy, or stable
  migration policy yet.

## Current Signing Roles

Prikk currently uses role-bound Ed25519 signatures.

AUTHOR signing is used for Patch envelopes produced by commit and rollback-draft authoring paths. The
AUTHOR signature identifies the key used by the authoring path, but it is not checked against a
repository-wide AUTHOR trust store.

MAINTAINER signing is used for publication objects. Seal signs Block, RefState, and RefUpdate
envelopes with the configured MAINTAINER signer and verifies that signer against the repository-local
maintainer trust policy before publishing.

The signature preimage binds the signature algorithm, object type, object id, signer role, and key id.

## Current Key Inputs

The CLI reads each role's **seed from a file**, in exactly two places and no third:

1. `PRIKK_AUTHOR_SEED_FILE` / `PRIKK_MAINTAINER_SEED_FILE`, if set — a path;
2. otherwise `<key directory>/author.seed` / `<key directory>/maintainer.seed`.

The key directory is `$XDG_CONFIG_HOME/prikk` (else `$HOME/.config/prikk`) on Unix and
`%APPDATA%\prikk` on Windows. `PRIKK_AUTHOR_KEY_ID` / `PRIKK_MAINTAINER_KEY_ID` name the key id
recorded in signatures and default to `author` / `maintainer`.

**A seed never travels through the environment.** `PRIKK_AUTHOR_SEED` and `PRIKK_MAINTAINER_SEED`
carried one until prikk 0.40; they are now detected and **refused**, not ignored, for one release. An
environment variable is readable by every child process, survives in shell profiles long after the
key changed, and lands in process listings — none of which is true of a file the operator places.

Each seed file holds a 32-byte Ed25519 secret seed as 64 hex characters. A missing file, a file
readable by group or others (Unix), an empty key id, wrong-length seed hex, and non-hex bytes all fail
closed before signing.

`prikk key public --role maintainer` derives the matching public key directly — see
[First Run](first-run.md). Nothing computes it automatically; deriving it is a separate, explicit
step.

## Can I Sign Here?

`prikk key status` answers that question without signing anything, and without printing a seed:

```text
prikk key status [path] [--role author|maintainer] [--format json]
```

It reports, per role, which of the two inputs above is in effect (`source`), the file it resolves to,
whether that file is usable and if not why (`reason` — missing, an override that names a file which is
not there, readable by group or others, or undecodable), the key id, and how that key id relates to
what this repository has already recorded (`binding`): `unrecorded`, `matches`, `mismatch`, or — for
MAINTAINER — `not-adopted`.

It answers the same question `commit` and `seal` answer when they refuse, from the same computation, so
a `usable: true` here is a prediction of what the signing path will do rather than a second opinion.
**Not-ready is an answer, not a failure**: every state above exits `0`. A non-zero exit is reserved for
a repository that cannot be read at all.

```sh
prikk key status --role author --format json
```

`--format json` emits a `key-status-v1` document and is the form front-ends should read; it carries
public material only.

## Maintainer Trust Store Setup

The current commands for repository-local MAINTAINER trust are:

```text
prikk trust maintainer add --key-id ID --public-key HEX
prikk trust maintainer remove --key-id ID
```

`ID` must match the MAINTAINER key id used by `PRIKK_MAINTAINER_KEY_ID`. `HEX` must be the lowercase
64-hex-character Ed25519 public key that matches the MAINTAINER seed file.

`add` writes the trusted public key and adds it to the repository's adopted-key set, with `required = 1`
continuing to mean any one adopted key's signature suffices. Adopting a key id already in the set with
the same public key succeeds idempotently; adopting it again with a different public key is refused.
This refusal is Prikk's trust-on-first-use enforcement: the first public key seen for a key id is the
one trusted for that id, permanently, even after removal — `remove` takes a key id out of the adopted
set, but re-adding the same id later with a *different* public key is still refused. There is still no
remote trust distribution.

## Minimal Local Workflow

The shortest path is one command — see [First Run](first-run.md) for the full walkthrough and output:

```sh
prikk setup ./sample-repo
```

`setup` prints the exports to run next, having already run `init`, generated both an AUTHOR and a
MAINTAINER seed, and registered the MAINTAINER key. If you would rather bring your own matched
seed/public-key pair instead of a generated one, or understand each step separately, the same result
composed by hand:

```sh
prikk init ./sample-repo

export PRIKK_AUTHOR_KEY_ID="author-key-id"
export PRIKK_AUTHOR_SEED_FILE="$AUTHOR_SEED_FILE"
export PRIKK_MAINTAINER_KEY_ID="maintainer-key-id"
export PRIKK_MAINTAINER_SEED_FILE="$MAINTAINER_SEED_FILE"

(cd ./sample-repo && prikk trust maintainer add \
  --key-id "$PRIKK_MAINTAINER_KEY_ID" \
  --public-key "$MAINTAINER_PUBLIC_KEY_64_HEX")

echo "hello prikk" > ./sample-repo/readme.txt
(cd ./sample-repo && prikk commit -m "genesis")
(cd ./sample-repo && prikk seal --allow-no-audit)
(cd ./sample-repo && prikk verify)
```

`AUTHOR_SEED_FILE` and `MAINTAINER_SEED_FILE` are paths to mode-`0600` seed files you supply, and
`MAINTAINER_PUBLIC_KEY_64_HEX` a value you supply — `prikk key generate --out <path>` and
`prikk key public --seed-file <path>` produce them if you do not already have your own. Leave both
`_FILE` variables unset to use the key directory, which is what `prikk setup` writes.

The MAINTAINER seed and public key above must be matched private/public halves of one Ed25519 keypair.
If they do not match, seal fails because the configured signer is not trusted by the repository-local
policy.

## Seed Handling Warnings

Any seed or key values published in Prikk's README, quick start, docs, tests, review packages, or issue
comments are public examples. They are compromised by publication and must never be used for real
signing.

Do not commit real seeds, paste them into issues, store them in shell history, print them in CI logs,
or put them in release artifacts. Prikk does not currently provide a secret-storage boundary; the
operator owns secret generation, storage, backup, rotation, and destruction outside Prikk.

## Failure and Diagnostic Hints

A missing AUTHOR seed file prevents commands that need AUTHOR signing from creating signed Patch
envelopes; the refusal names the path it looked for and the command that creates it.

A missing MAINTAINER seed file prevents seal from creating signed publication objects, the same way.

A seed file that group or others can read is refused before signing, naming the mode and the `chmod`
that fixes it.

Malformed seed hex is rejected before signing. Empty key ids and unsafe key ids are rejected by shared
signature validation.

An untrusted MAINTAINER signer prevents seal from publishing. A repository with publication objects
that do not verify against the local trust policy reports publication-trust issues through `verify` and
`doctor`.

The current CLI wording is human diagnostic output, not a stable machine-readable key-management
contract.

## Deferred Work

Key generation and public-key derivation shipped — see [First Run](first-run.md). Still deferred: a
general configuration command and every durable, non-secret policy setting (no beneficiary yet — a
first real adopter is the named trigger); a credential-helper boundary (refused deliberately, git/ssh-style, in
favor of the write-once-where-you-name-it model `key generate --out` already uses); local secret
storage beyond that single named write, keychain integration, passphrase handling, key rotation, key
expiration, compromise recovery, hardware signing, multi-maintainer thresholds, repository-wide
AUTHOR trust policy (including AUTHOR-identity revocation — only MAINTAINER key revocation is
supported), remote trust, hosted identity, JSON key-management output, stable trust-policy migration,
stable repository-format migration, and
production readiness.

## Claim-to-Source Anchors

| Claim | Source anchors |
|---|---|
| AUTHOR and MAINTAINER production signing use real Ed25519 signatures. | [`author_signing.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/author_signing.rs), [`maintainer_signing.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/maintainer_signing.rs), [DC-10](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-10-ROLLBACK-DRAFT-SIGNING.md), [DC-11](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-11-MAINTAINER-TRUST-STORE.md) |
| Signature preimages bind algorithm, object type, object id, signer role, and key id. | [`signature.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-object/src/signature.rs), [`author_signing.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/author_signing.rs), [`maintainer_signing.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/maintainer_signing.rs) |
| The CLI reads AUTHOR and MAINTAINER key material from environment variables and expects 64-hex secret seeds. | [`main.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/main.rs), [`author_signing.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/author_signing.rs), [`maintainer_signing.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/maintainer_signing.rs) |
| Prikk exposes `trust maintainer add`, key generation (`key generate`), and public-key derivation (`key public`) — since RFC 135. | [`key.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/key.rs), [`commands.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/commands.rs), [DC-30](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-30-KEY-MANAGEMENT-SIGNING-SETUP-GUIDE.md) |
| The maintainer trust store is repository-local and fixed to one MAINTAINER key with `required = 1`. | [`trust.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/trust.rs), [DC-11](https://github.com/prikk-vcs/prikk/blob/main/rfcs/done/DC-11-MAINTAINER-TRUST-STORE.md), [trust and threat model](../reference/trust-threat-model.md) |
| Seal verifies the configured MAINTAINER signer against local trust before publication. | [`seal.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-cli/src/seal.rs), [`trust.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/trust.rs) |
| Verify checks publication trust for Block, RefState, and RefUpdate objects against local MAINTAINER trust. | [`verify.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/verify.rs), [`trust.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/trust.rs), [integrity and recovery diagnostics](../reference/integrity-recovery.md) |
| Current AUTHOR signatures are not checked against a repository-wide AUTHOR trust policy. | [`verify.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/verify.rs), [`rollback_verify.rs`](https://github.com/prikk-vcs/prikk/blob/main/crates/prikk-store/src/rollback_verify.rs), [trust and threat model](../reference/trust-threat-model.md) |

## Provenance

This guide implements DC-30. It is documentation-only and does not change signing, trust, CLI, object
schema, repository format, verification, seal, or repository behavior.
