# RFC 169 — publish the release key, and say exactly what it covers (item 3)

**Live 2026-10-07, and it is next.** RFC 169 is ACCEPTED by the owner.

## Task title and purpose

RFC 169 §3.1 and §5 item 3: a user can find the release key's fingerprint where they look, check a tag, and know exactly
what that check covers and what it does not. **No sentence may claim more than the mechanism gives.**

## Background and governing RFC

- **Read `rfcs/proposed/169-when-the-release-key-is-lost-or-compromised.md` whole,** above all §1 (the facts), §3.1, §3.3,
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

## Addendum 1 — 2026-10-07: what users see (RFC 169 §8, correction C1, from the rehearsal)

Add to items 1 and 2:
- **The check users run is `git verify-tag --raw <tag>`.** The **last field of the `VALIDSIG` line** must equal
  `25757DA6CBF7022C4E14CCAC1B3066B87DB99A34`. This holds whether the primary key or a subkey signed the tag, and
  `git tag -v` alone shows a subkey's id once one signs.
- **Say that *"This key is not certified with a trusted signature"* is expected** unless the user has certified the key
  (`gpg --lsign-key`). What matters is the fingerprint match.
- **The lost/compromised paragraph follows §8:**
  - a key that may be in someone else's hands is revoked, after which `git tag -v` fails on the old tags and the advisory
    lists the confirmed releases by commit id;
  - a key that is simply gone is retired without revocation, so old tags keep verifying.

## Addendum 2 — 2026-10-07: three text fixes (review `rfc169-publication-review-v1`)

1. **`install.md` and `SECURITY.md`:**
   - say the check runs in a clone (`git clone https://github.com/prikk-vcs/prikk && cd prikk`);
   - show `git verify-tag --raw <tag> 2>&1 | grep VALIDSIG`.
2. **The release-notes line leads with that check:** *"Release key <fp>: in a clone, `git verify-tag --raw <tag> 2>&1 | grep VALIDSIG` must end with this fingerprint (SECURITY.md, "Verifying a release")."* Update its test.
3. **`release-compatibility.md`:** the same `verify-tag --raw` check in place of `git tag -v X.Y.Z`.

**Budget:** 20 min. **Report:** `.git-exclude/review-request/rfc169-publication-report-v2.md`, with `scripts/gates.py`'s summary.

**Item 3 ACCEPTED 2026-10-07** (reviews `rfc169-publication-review-v1`, `-v2`; `2ba2c755`, `d9a60467`). The push follows the owner's reading of RFC 169 §8 (C1). **Owner actions still open:** items 1, 2 and 6 now, and item 4 before the first stable release (runbook `.git-exclude/runbooks/release-key-runbook.md`).

**2026-10-07: the owner approved C1** (RFC 169 §8). Item 3 is pushed with this record. **Item 3 is closed.**

**2026-10-07: RFC 169 parked by the owner** (acceptance withdrawn; back in `proposed/`). Item 3's factual docs stay; the lost/compromised paragraph in `SECURITY.md` is removed in the 0.49.0 release prep. Nothing else here is live.

**2026-10-07: moved here from `rfcs/handoffs/169-…/`.** The owner parked RFC 169 (it is back in `proposed/`), and a proposed RFC carries no handoff directory (RFC 120 §9.4a). This file records release-verification docs work that shipped in `d9a60467`, so it lives with the release policy's handoffs. Nothing in it is live.
