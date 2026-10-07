# Security Policy

Prikk is pre-1.0 experimental software. This file states what this project can and cannot promise
about security, and where to report a vulnerability privately.

## Reporting a vulnerability

Report privately through [GitHub Security
Advisories](https://github.com/prikk-vcs/prikk/security/advisories/new). **Do not open a public
issue** for a suspected vulnerability.

The interesting classes here are **identity, signature, publication, durability, and path/format
handling** — anything that could let a repository, ref, patch, or block appear authored, signed, or
maintainer-approved when it is not, or that could corrupt or misdirect a write. An ordinary bug (a
crash, a wrong result, a missing feature) is not a security report — use a public issue for those.

## Already known — please don't report these as findings

- The trust-on-first-use authorship boundary, and the absence of key rotation or revocation for
  AUTHOR keys — see [Trust and Threat Model § Core
  Caveats](./docs/src/reference/trust-threat-model.md#core-caveats).
- The platform durability gaps on Windows — see [Platform
  Support](./docs/src/reference/platform-support.md).

## What this project commits to

An accepted report will be acknowledged, and the fix will be made. **There is no CVE assignment
process and no committed response time** — this is pre-1.0 software with one maintainer, and
stating a timeline nobody has agreed to meet would be worse than stating none.

## Verifying a release

Release tags are signed with the project's one maintainer key. Its fingerprint is:

    25757DA6CBF7022C4E14CCAC1B3066B87DB99A34

Fetch the key, then check a tag from a clone of the repository:

    curl -fsSL https://github.com/nabbisen.gpg | gpg --import
    git clone https://github.com/prikk-vcs/prikk && cd prikk
    git verify-tag --raw 0.48.0 2>&1 | grep VALIDSIG

`git verify-tag --raw <version>` prints a `VALIDSIG` line, on stderr (hence `2>&1`). Its **last field** must equal the
fingerprint above. That is the check. The warning *"This key is not
certified with a trusted signature"* is expected unless you have certified the key yourself (`gpg --lsign-key`). What
matters is the fingerprint match.

**What each thing covers:**

- **The tag signature** covers the source at the tagged commit.
- **The release binaries** are built from that tag by the release workflow. They are not signed, so the tag does not
  cover them.
- **The `.sha256` beside each asset** is made in the same job as the asset. It detects a damaged download, not a
  substituted one: whoever can replace an asset can replace its checksum too.

**Tags are never moved or re-signed.**

There is
**no second signer**, **no support window** (only the latest release gets fixes) and **no stability
promise** for the object format, the CLI's JSON schemas or the library API before 1.0. The
release-signer allowlist (`release-signers.toml`) is empty because no multi-signer policy exists yet.
See [Release, Versioning, and Compatibility § How prikk
releases](./docs/src/reference/release-compatibility.md#how-prikk-releases). Whatever you obtain,
verify its content with `prikk verify`.
