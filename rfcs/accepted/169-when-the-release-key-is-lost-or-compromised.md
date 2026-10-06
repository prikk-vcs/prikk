# RFC 169 — When the release key is lost or compromised: a written procedure, and what a signature covers (D7)

**Status.** **ACCEPTED 2026-10-07 by the owner** (*"Accepted."*).
- **History:** proposed 2026-10-06 by the architect (0.49.0 step 6, *"D6, D5, D7"*, from external review 014); revised
  the same day after the architect's review against the owner's philosophy (§7).
- **The architect's reading of that acceptance:** §3 as written, and §5's seven recommendations, carried out as the owner
  delegated on 2026-10-06 (*"All except 6. can be processed without me (including 7.)?"*, answered in this session):

  | item | what | who | when |
  |---|---|---|---|
  | 3 | publish the fingerprint and what it covers | the dev team, then the architect's review | 0.49.0: `rfcs/handoffs/169-when-the-release-key-is-lost-or-compromised/publication-handoff-v1.md` |
  | 7 | the rehearsal | the architect, with a throwaway key in an isolated keyring | now; it produces a tested runbook for items 1, 2 and 4 |
  | 1, 2, 4 | the two revocation certificates, the encrypted backup, the signing subkey with the primary offline | **the owner only** (the private key and its passphrase) | 1 and 2 now; 4 before the first stable release |
  | 6 | the GitHub tag ruleset | **the owner only** (a repository setting) | now |
  | 5 | the binary chain (tag-signature check, attestations) | a separate RFC | after 0.49.0 |

  - **Item 7's reading:** the owner's own walk-through is replaced by the tested runbook. Carrying out items 1, 2 and 4 from
    it is the owner's practice of §3.2.
- **A procedure and a few owner actions; no product code.** It does not touch `release-signers.toml`.
- **The owner holds the key, so every action that needs the private key is the owner's.** No agent handles key material.
- **Once accepted,** the procedure lands in `SECURITY.md` and the docs, reviewed by the dev team before push (RFC 152
  §7).

## 1. Facts (read 2026-10-06)

- **014, D7:** *"One key signs everything, its fingerprint is printed only in `MILESTONES.md` … No written plan for its
  failure is not [defensible]."* Graded "0.49.0, before any stable claim".
- **How a release is signed** (RFC 152): an annotated tag, GPG-signed by the owner, never moved or re-signed. The 0.48.0
  tag reads `prikk 0.48.0` plus a link to the Release page.
- **The key** (`gpg --list-keys`, public part only):
  - `rsa4096/1B3066B87DB99A34`, fingerprint `25757DA6CBF7022C4E14CCAC1B3066B87DB99A34`, created 2021-08-04;
  - **the primary key itself signs and certifies (`[SC]`). There is no signing subkey and no expiry.** One encryption
    subkey;
  - **it is the owner's personal GitHub key,** not a key used only for prikk.
- **What users actually check today:**
  - **Release binaries are not signed.** `release.yml` makes each `.sha256` file in the same job, and uploads it next to
    its asset (`release.yml:86-213`).
  - **So a checksum detects a damaged download, not a substituted one.** Whoever can replace an asset can replace its
    checksum too.
  - **`git tag -v` covers the source at the tagged commit,** not the binaries built from it.
  - **crates.io publishes through Trusted Publishing** (RFC 141), a separate credential.
- **The fingerprint appears only in `MILESTONES.md`.**
  - `SECURITY.md:31-42` mentions *"the project's one maintainer key"* without naming it.
  - No user doc says how to verify anything.
- **`release.yml` checks CI status, not the tag's signature.** Nothing on GitHub stops a release tag being moved or
  deleted, except the owner's discipline.
- **There is no rotation, revocation or compromise text.** DC-35 and DC-43 were archived as not applicable to v0 (RFC
  152).

## 2. Goals

- **A user can tell what a signature covers, and check it.** No sentence claims more than the mechanism gives.
- **A lost key** costs no existing release its validity.
- **A compromised key** is contained quickly. Users are told which releases remain trustworthy and how to check.
- **No moving tags, ever,** enforced rather than only promised.

## 3. The procedure

### 3.1 Verify (users)

- **`SECURITY.md` and `docs/src/guide/install.md` say, plainly:**
  - **Release tags are signed** with key `25757DA6CBF7022C4E14CCAC1B3066B87DB99A34`. Fetch it from
    `https://github.com/nabbisen.gpg`, then check a tag with `git tag -v <version>`. The fingerprint is repeated in every
    release's notes.
  - **What that covers:** the source at the tagged commit.
  - **Release binaries are built from that tag by the release workflow.** Their `.sha256` files detect a damaged
    download, not a substituted one. *(If §5 item 5 is accepted, this line changes to the attestation check.)*

### 3.2 Prepare (owner, while the key is healthy)

1. **Two revocation certificates, kept offline,** in a different place from any backup:
   - **"key is no longer used"** (a soft revocation): for a lost key. Signatures made before it stay valid in tools that
     distinguish the reasons (OpenPGP's soft/hard rule);
   - **"key has been compromised"** (a hard revocation): for a stolen key.
   - **GnuPG 2.1 and later already made one at key creation** (`openpgp-revocs.d/`), with *"no reason specified"*. Strict
     tools treat that as hard: every old release tag would stop verifying. **So it is the compromise certificate only.**
2. **An encrypted offline backup of the secret key**, so that a lost laptop is not a lost key.
3. **A signing subkey for daily use, with the primary key moved offline** (certify only).
   - **The effect:** a compromise of the everyday machine burns the subkey, not the identity. The fingerprint users
     check stays the same.
   - **What users see:** `git tag -v` shows *"using RSA key <subkey>"*, still under the same primary fingerprint.
   - **After this,** the public key is re-uploaded to GitHub.
4. **A GitHub ruleset on release tags** (`*.*.*`): only the owner may create them, and nobody may update or delete them.
   This makes *"a tag is never moved"* a mechanism, not a promise.

### 3.3 A lost key (the owner can no longer sign)

- **Existing releases stay valid. Nothing is moved or re-signed.**
- **Revoke with the soft certificate,** so the key cannot be misused if it turns up later.
- **A new key is introduced through the project's GitHub account:** a `SECURITY.md` commit naming its fingerprint, and a
  GitHub Security Advisory. **The account is the second trust anchor, and it is protected separately** (2FA, passkeys).
- **If only the subkey was lost** (§3.2 item 3), the primary issues a new subkey. The fingerprint is unchanged, and
  users need do nothing.
- **The next release's CHANGELOG states the change.**

### 3.4 A compromised key (someone else may sign)

1. **Within hours:**
   - publish the hard revocation to the places the key is published (GitHub, plus the keyserver if §5 item 3 names one);
   - publish a GitHub Security Advisory naming the key and the window believed affected;
   - pause releases.
2. **Assess:**
   - **Every release tag in the official repository is compared with its release-workflow run:** the run's commit, and
     its assets' checksums.
   - **A forged tag outside the official repository is not a release.** The advisory says so.
3. **Tell users what still holds.**
   - **After a hard revocation,** strict tools stop verifying old tags. So the advisory lists every release confirmed by
     its workflow run, **with its commit id,** so that a user can check a checkout by commit id.
   - **No tag is moved or re-signed.**
4. **Resume** with a new key, as in §3.3. **If only the subkey was compromised:** revoke the subkey, issue a new one,
   keep the fingerprint, and still publish the advisory.

### 3.5 Rehearsal

Once, before the first stable release, the owner walks through §3.2–§3.4 with a throwaway key, and records that it was
done (not the key material).

## 4. What this RFC does not do

- **No second signer, quorum, or `release-signers.toml` change** (RFC 152: v0 has one signer).
- **No new project-only key.** Replacing the personal key would change the fingerprint users check, which is a
  transition with its own confusion. The subkey (§3.2 item 3) gives most of the isolation without one.
- **No release-workflow change in this RFC.** §5 item 5 is a separate small change, if accepted.

## 5. Decisions for the owner

| # | decision | recommendation | why |
|---|---|---|---|
| 1 | the two revocation certificates, offline (§3.2 item 1) | **yes, now** | without the soft one, a lost key either stays unrevoked or is revoked hard, which breaks every old tag in strict tools |
| 2 | an encrypted offline backup (§3.2 item 2) | **yes, now** | a lost machine should not force a key change; a revocation certificate alone cannot sign |
| 3 | where the fingerprint is published (§3.1) | **`SECURITY.md`, `install.md`, each release's notes, and GitHub's key URL;** keys.openpgp.org optional | the places a user looks, with the command next to the fingerprint |
| 4 | a signing subkey, with the primary offline (§3.2 item 3) | **yes, before the first stable release** (not blocking 0.49.0) | a compromised machine then costs a subkey, not the identity users check |
| 5 | the binary chain, a separate small change after 0.49.0: `release.yml` verifies the tag's signature against the pinned fingerprint before building, and attaches GitHub artifact attestations to every asset (`gh attestation verify`) | **yes, after 0.49.0** | today nothing ties a downloaded binary to the key; this ties it to a signed tag and to the workflow run that built it |
| 6 | the tag ruleset (§3.2 item 4) | **yes, now** (a GitHub setting, the owner's to change) | "never moved" becomes enforced |
| 7 | the rehearsal (§3.5) | **yes, before the first stable release** | a procedure never walked through fails at its first real use |

**Considered and not recommended:** an expiry date on the key. Each time the owner forgets to extend it, every user sees
*"This key has expired"* on every old tag. With items 1 and 2 in place, it adds little.

## 6. Security considerations

- **Publishing the fingerprint reveals nothing secret,** and lets users detect a forged tag.
- **Residual:**
  - one signer is a single point of failure until revocation. RFC 152 accepts that for v0;
  - **binary authenticity rests on the GitHub account and workflow** until §5 item 5 lands. §3.1 says so to users.
- **The personal key is used outside prikk too.** Its exposure there is exposure here. §5 item 4 narrows that.

## 7. The owner-philosophy review (2026-10-06)

| finding | before | after |
|---|---|---|
| **A claim wider than the mechanism** | "the tag's signature is the root of trust for a release", though the binaries are unsigned and checksums are made beside them | §3.1 says what the tag covers and what a checksum does not; item 5 closes the gap |
| **A revocation that would break every old tag** | "revoke with the revocation certificate"; GnuPG's automatic certificate is *no reason specified*, which strict tools read as hard | a soft certificate for loss, a hard one for compromise |
| **One key for identity and daily signing** | the primary signs; any compromise burns the fingerprint | a signing subkey, with the primary offline (item 4) |
| **"Never moved" only promised** | discipline | a GitHub tag ruleset (item 6) |
| **Users unable to verify after a hard revocation** | not addressed | the advisory lists confirmed releases by commit id |
| **A backup left as "your choice"** | no recommendation | recommended, with the reason |
