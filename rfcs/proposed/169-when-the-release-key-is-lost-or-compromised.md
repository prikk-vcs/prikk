# RFC 169 — When the release key is lost or compromised: a written procedure (D7)

**Status.** **PROPOSED 2026-10-06 by the architect** (0.49.0 step 6: *"D6, D5, D7"*, from external review 014).
- **A procedure, not code.** It changes no product behaviour and does not touch `release-signers.toml`.
- **The owner holds the release key, so the decisions in §5 are the owner's.** Once they are decided, the procedure lands
  in `SECURITY.md` and the docs, reviewed by the dev team before push (RFC 152 §7).

## 1. What is missing (facts, read 2026-10-06)

- **014, D7:** *"One key signs everything, its fingerprint is printed only in `MILESTONES.md`, and the documents that
  covered revocation are archived as not applicable to v0. One signer is a defensible v0 choice. No written plan for its
  failure is not."* Graded "0.49.0, before any stable claim".
- **How releases are signed today** (RFC 152 §51-53): an annotated tag, GPG-signed with the owner's key, verified with
  `git tag -v`. **A tag is never moved or re-signed.**
- **The fingerprint** (RSA `25757DA6CBF7022C4E14CCAC1B3066B87DB99A34`) appears only in `MILESTONES.md`.
  - `SECURITY.md:31-42` says releases are "signed by the project's one maintainer key", but names no fingerprint.
  - No user-facing doc tells a user how to verify a tag.
- **`release.yml`** checks CI status, not the tag's signature.
- **`release-signers.toml`** holds `authorized_primary_fingerprints = []`. **It is not changed by this RFC.**
- **There is no rotation, revocation or compromise text for the release key.** The archived DC-35 had a full
  break-glass machine, and DC-43 deferred custody and rotation, both ruled not applicable to v0 (RFC 152).
- **crates.io publishing** is a separate credential (RFC 141, Trusted Publishing), with its own compromise path.

## 2. Goals

- A user can **verify** a release: the fingerprint is published where users look, with the command.
- **Lost key** (the owner can no longer sign): what happens to existing releases (nothing; their signatures still
  verify), how a new key is introduced, and how users learn of it.
- **Compromised key** (someone else may sign): how users are told, which signatures stop being trustworthy, how the
  key is revoked, and how a new key is introduced.
- **No moving tags, ever** (RFC 152). The procedure never re-signs history.

## 3. The proposed procedure

1. **Publish and verify:**
   - `SECURITY.md` and `docs/src/guide/install.md` name the fingerprint and the command (`git tag -v <tag>`), and say
     the tag's signature is the root of trust for a release;
   - the release notes repeat the fingerprint.
2. **Preparation, while the key is healthy (owner):**
   - an offline revocation certificate for the key;
   - an offline backup of the key, or a decision not to keep one (§5);
   - a note of where both live, kept outside the repository.
3. **Lost key:**
   - existing releases stay valid;
   - the owner creates a new key, publishes its fingerprint in `SECURITY.md` and a GitHub Security Advisory, and the
     next release is signed with it;
   - **the old key is revoked with the revocation certificate,** so it cannot be misused if found later;
   - the CHANGELOG entry for the next release states the change.
4. **Compromised key:**
   - **within hours:** publish the revocation certificate to the keyservers the fingerprint is listed on; publish a
     GitHub Security Advisory naming the key and the time window believed affected; pause releases;
   - **assess:** every tag in the window is checked against the release workflow's records (the CI run, the commit,
     the asset checksums). A tag that matches its CI run and pushed commit stays valid, and one that does not is named
     in the advisory as untrusted. **No tag is moved or re-signed;**
   - **resume** with a new key, as in item 3;
   - the crates.io token is handled separately (RFC 141).
5. **Rehearsal:** once, before the first stable release, the owner walks through items 2–4 against a throwaway key, and
   records that it was done (not the key material).

## 4. What this RFC does not do

- It does not add a second signer, a quorum, or any change to `release-signers.toml` (RFC 152: v0 is one signer).
- It does not add tag-signature verification to `release.yml`. **That is a candidate for the owner** (§5, item 4), not
  part of this procedure.

## 5. Decisions for the owner

1. **A revocation certificate, created now and kept offline:** yes or no. *Recommended: yes.* Without it, a lost key
   cannot be revoked at all.
2. **A backup of the key, kept offline:** yes or no. A backup prevents loss but adds a copy that could leak. *Your
   choice; the procedure works either way, and says which one applies.*
3. **Where the fingerprint is published:** `SECURITY.md` and `install.md` (*recommended*), plus a keyserver of your
   choice.
4. **Whether `release.yml` should verify the tag's signature before publishing:** a separate small change, not part of
   this RFC. *Recommended for after 0.49.0.*
5. **The rehearsal (§3 item 5) before the first stable release:** yes or no. *Recommended: yes.*

## 6. Security considerations

- **Publishing the fingerprint helps** users detect a forged tag. It reveals nothing secret.
- **Every action that needs the private key** (the revocation certificate, signing) stays with the owner. No agent
  handles key material.
- **Residual:** one signer means a compromise is a single point of failure until revoked. Stated, as RFC 152 accepts for
  v0.
