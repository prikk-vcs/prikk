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

Every release tag is signed by the project's one maintainer key and verified before it is pushed. The
checksum beside each downloaded asset proves the file matches what the release page published, and its
build-info names the commit and tag it was built from; neither proves *who* published it — the signed
tag does. There is
**no second signer**, **no support window** (only the latest release gets fixes) and **no stability
promise** for the object format, the CLI's JSON schemas or the library API before 1.0. The
release-signer allowlist (`release-signers.toml`) is empty because no multi-signer policy exists yet.
See [Release, Versioning, and Compatibility § How prikk
releases](./docs/src/reference/release-compatibility.md#how-prikk-releases). Whatever you obtain,
verify its content with `prikk verify`.
