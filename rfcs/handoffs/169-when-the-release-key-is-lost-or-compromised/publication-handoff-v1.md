# RFC 169 — publish the release key, and say exactly what it covers (item 3)

**Live 2026-10-07, and it is next.** RFC 169 is ACCEPTED by the owner.

## Task title and purpose

RFC 169 §3.1 and §5 item 3: a user can find the release key's fingerprint where they look, check a tag, and know exactly
what that check covers and what it does not. **No sentence may claim more than the mechanism gives.**

## Background and governing RFC

- **Read `rfcs/accepted/169-when-the-release-key-is-lost-or-compromised.md` whole,** above all §1 (the facts), §3.1, §3.3,
  §3.4 and §7.
- **The key:** RSA `25757DA6CBF7022C4E14CCAC1B3066B87DB99A34`. Users fetch it from `https://github.com/nabbisen.gpg`.
- **The facts the docs must match:**
  - release tags are GPG-signed;
  - **release binaries are not signed;**
  - each `.sha256` file is made in the same job as its asset and uploaded beside it (`release.yml:86-213`). So it detects
    a damaged download, not a substituted one.
- **Today's `SECURITY.md` overclaims.** `SECURITY.md:31-36` says the checksum *"proves the file matches what the release
  page published"*, and that the signed tag proves *"who published it"*. But the tag does not cover the binaries.

## Change scope

1. **`SECURITY.md`, the "Verifying a release" section:**
   - name the fingerprint, how to fetch the key, and `git tag -v <version>`;
   - **say what each thing covers:**
     - the tag signature covers the source at the tagged commit;
     - the binaries are built from that tag by the release workflow, and are not signed;
     - a checksum detects a damaged download, not a substituted one;
   - **a short "If the release key is lost or compromised" paragraph, from §3.3–§3.4, in user terms:**
     - announcements come as a GitHub Security Advisory, and a `SECURITY.md` commit naming any new key;
     - after a compromise, old tags may no longer verify in strict tools, and the advisory lists every confirmed release
       by commit id;
     - tags are never moved or re-signed.
2. **`docs/src/guide/install.md`:** a short "Verify the tag" section next to "Verify the checksum", with the same
   fingerprint, the same command and the same coverage sentence. Correct "Verify the checksum" if it overclaims.
3. **The release-notes generator** (`tools/release-policy`, `release-notes`): every release's notes end with one line
   naming the fingerprint and `git tag -v <tag>`.
   - **The fingerprint is one constant in the tool,** and a test pins it against the docs. The constant, `SECURITY.md` and
     `install.md` agree, or the test fails.

## Explicit non-change scope

- No change to `release.yml` (the binary chain is §5 item 5, a later RFC).
- No change to `release-signers.toml`.
- No change to `MILESTONES.md`.
- **No key material anywhere.**

## Prohibited shortcuts

- **"Root of trust", "proves who published", or any wording wider than the mechanism.**
- **A fingerprint typed more than once without the pinning test.**

## Compatibility and security constraints

- Docs only, plus one generator line. The release workflow's commands are unchanged (`command_scan` still classifies
  them).
- **The fingerprint must be copied exactly. Compare it against `gpg --list-keys` output of the public key, not by eye.**

## Known risks

- A second, stale fingerprint left somewhere in the docs. **Grep for 40-hex strings under `docs/` and in `SECURITY.md`,
  and report every hit.**

## Required evidence and review request

- **The diffs,** and the generator's output for a sample tag.
- **The pinning test, and its control:** change one hex digit in the constant, and the test goes red.
- **The grep** for 40-hex strings.
- **`scripts/gates.py`'s summary.**
- **Report:** `.git-exclude/review-request/rfc169-publication-report-v1.md`, with `date` at the start and end.

| unit | what | budget (stop at ×2) |
|---|---|---:|
| P1 | items 1–3 | 60 min |
