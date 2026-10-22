# Arch Linux (AUR) packages, updated automatically by the release workflow

**Written 2026-10-21. It is 0.54.0 step 3 Part 7, after Part 6.** The owner registered both packages and decided
(2026-10-21):
1. **in 0.54.0:** *"Yes."*;
2. **automated, with no approval gate:** *"I want it automated unless there is any serious risk or concern."*;
3. **the source package verifies the signed tag:** *"Yes."*;
4. **`prikk-bin` for x86_64 and aarch64:** *"Yes."*

**The architect found no risk that needs a human gate.** The packages point at the same tag and assets the release
already publishes. The one real risk is the AUR key, and this design keeps it narrow:
- the key is used in one job, holding only `ssh` and `git`;
- the job is reachable only from release tags;
- AUR's host key is pinned.

**The packages today** are placeholders (`pkgver=0.0.0`, dummy checksum):
- <https://aur.archlinux.org/packages/prikk>
- <https://aur.archlinux.org/packages/prikk-bin>

The design is `.git-exclude/scratch/aur-packaging-proposal-v1.md`, amended by decision 2.

## The working rule (unchanged)

As the step 3 handoff:
- one sitting, one report (`0.54.0-step-3-part-7-report.md`);
- R1 scopes;
- controls shown red;
- all 14 gates;
- **a workflow change is read on CI by the architect.**

## 1. Generated PKGBUILDs and `.SRCINFO` (templates, like `install.sh`)

New templates in `tools/release-policy/templates/`, emitted by `generate-installer` (or a sibling subcommand; say
which) for a given version, with checksums filled in from the release's own `.sha256` files.

**`prikk`, built from source:**
- `source=("git+https://github.com/prikk-vcs/prikk.git#tag=${pkgver}?signed")`, with
  `validpgpkeys=('25757DA6CBF7022C4E14CCAC1B3066B87DB99A34')` and `sha256sums=('SKIP')`, which is correct for a git
  source verified by signature;
- `makedepends=(git rust cargo)`;
- `prepare()`: `cargo fetch --locked`;
- `build()`: `cargo build --release --frozen -p prikk`;
- `package()`: `install -Dm755` the binary to `/usr/bin/prikk`, and `install -Dm644 LICENSE` to
  `/usr/share/licenses/prikk/LICENSE`;
- `license=('Apache-2.0')`, `arch=('x86_64' 'aarch64')`, `pkgrel=1` for each new version.

**`prikk-bin`:**
- `source_x86_64` and `source_aarch64` point at `prikk-<target>.tar.gz` from the release;
- `sha256sums_x86_64` and `sha256sums_aarch64` come from the published `.sha256` files;
- `provides=('prikk')`, `conflicts=('prikk')`;
- `package()` installs the binary and `LICENSE` as above;
- its `pkgdesc` or a comment says the checksum proves the file matches the release page, not who published it.

**A unit test** pins the generated text for a fixed version and checksums, so a template change is a reviewed diff.

## 2. The release workflow: build in one job, push in another

**Job `aur-build`** (needs the publication job; **no secrets**):
- runs in an `archlinux:base` container, pinned by digest;
- generates both PKGBUILDs for the tag;
- as a non-root user, runs `makepkg --printsrcinfo > .SRCINFO` for each;
- runs `makepkg -s` (or `--nobuild` plus `--verifysource`, then a build) for **both** packages.
- For `prikk` this imports the maintainer public key from the repository (state where it is kept) and **must fail if
  the tag signature does not verify.**
- uploads the two package directories (PKGBUILD and `.SRCINFO` only) as a workflow artifact.

**Job `aur-push`** (needs `aur-build`; `environment: aur`; **no container, no third-party action**):
- downloads the artifact;
- writes `secrets.AUR_SSH_PRIVATE_KEY` to a file readable only by the runner user;
- uses a **pinned `known_hosts` line for `aur.archlinux.org`,** taken from the fingerprints AUR publishes on its web
  site, cited in the workflow comment, with `StrictHostKeyChecking=yes`;
- clones `ssh://aur@aur.archlinux.org/<pkg>.git`, copies PKGBUILD and `.SRCINFO`, commits as *"prikk <version>"*, and
  pushes, for each package;
- **if the content is unchanged** (a rerun), it pushes nothing and succeeds;
- removes the key file at the end, whatever the outcome.

**Permissions:** `contents: read` for both jobs, and nothing else.

**A failure in either job never touches the GitHub release.** The jobs run after publication. A rerun is safe.

## 3. A dry run before the first real release

Add a `workflow_dispatch` input to run **only `aur-build`** for an existing tag, never `aur-push`. Run it for `0.53.0`
before 0.54.0 is cut: both packages must build, and the source package must verify `0.53.0`'s signed tag.
- **The architect reads that run.**
- **A control:** the same dry run against a tag name that does not exist, or with the key removed from the job, must
  fail in `makepkg`'s signature check. Show it red.

## 4. The owner's setup (the owner does this; no agent touches the key)

The report lists these steps for the owner, exactly:
1. Create a dedicated key: `ssh-keygen -t ed25519 -f aur-prikk -C "prikk release workflow (AUR)" -N ""`.
2. On <https://aur.archlinux.org/account/>, add the public half (`aur-prikk.pub`) under *SSH Public Key*.
3. In the GitHub repository, go to *Settings → Environments → New environment* `aur`.
   - **Deployment branches and tags:** *selected*, tag pattern `[0-9]*.[0-9]*.[0-9]*`.
   - Add the secret `AUR_SSH_PRIVATE_KEY` with the private half's content.
4. Delete the local private key file afterwards, or keep it offline. A lost key is replaced by repeating 1–3.

## 5. Text

- **`docs/src/guide/install.md`, an Arch Linux section:**
  - `paru -S prikk-bin` (or `yay`), the published binary;
  - `paru -S prikk`, built from the signed source tag;
  - what each verifies.
- **The landing page's Linux tab** (Part 6) may name the AUR in one short line. Say whether it fits RFC 137's rules.
- **The CHANGELOG `### Added`:** prikk is published to the AUR by the release workflow.
- **The release-cut procedure** gains one line: the architect reads the `aur-build` and `aur-push` jobs.

## Then

- The architect reviews and pushes, reads the dry run, and reads both AUR jobs in the 0.54.0 release run.
- **The architect then confirms the AUR pages show 0.54.0.**

## Progress, 2026-10-22: Part 7a reviewed; `aur-build` accepted, `aur-push` needs Part 7a2

**Review:** `.git-exclude/reviewed/0.54.0-step-3-part-7a-review-v1.md`. Pushed at `37b430a2`.
- **The dry run for `0.53.0` is green on CI** (run `39715772469`: the signed tag `Passed`, and both packages built).
- **Its control is red** (run `39715780143`, with the key import skipped: `FAILED (unknown public key …)`).

**Next: Part 7a2,** about 45 minutes. It has five conditions, C1–C5, all in the review:
- **C1:** `.SRCINFO` is missing from the artifact, because the upload skips hidden files.
- **C2:** the ssh key and `known_hosts` are relative paths, which `git -C` moves.
- **C3:** commits have no author on the runner. **The owner decided the identity on 2026-10-22:**
  `prikk release workflow <19984221+nabbisen@users.noreply.github.com>`.
- **C4:** the key file is created with mode 600 from the start.
- **C5:** a local control for `aur-push`'s step sequence, shown red and then green.

Report: `0.54.0-step-3-part-7a2-report.md`.

**Then Part 7b.** Its `install.md` section and its landing-page line are committed on `hold/0.54.0-windows-docs`, not
`main`: the AUR packages are placeholders until the 0.54.0 release runs `aur-push`. Its CHANGELOG entry goes on
`main`. The Arch section tells users to import the release key before `paru -S prikk`, because the key is on no public
keyserver.

## Progress, 2026-10-22: Part 7a accepted; Part 7b is next

**Review:** `.git-exclude/reviewed/0.54.0-step-3-part-7a2-review-v1.md`. Pushed at `a818ca85`; CI 17/17.
- **The dry run, re-run** (run `39722106943`), uploads all four files. Both checksums match 0.53.0's release page.
- **`aur-push`'s committed step text was run by the architect against local repositories:**
  - green, and a rerun that commits nothing;
  - both controls red: with no commit identity, and with relative key paths.

**Next: Part 7b, §5's text,** with the review-7a notes:
- the `install.md` Arch section and the landing line go on `hold/0.54.0-windows-docs`;
- `install.md` says to import the release key before `paru -S prikk`;
- the CHANGELOG entry goes on `main`.

Report: `0.54.0-step-3-part-7b-report.md`.

## Progress, 2026-10-22: Part 7b reviewed; Part 7b2 (text only) is next

**Review:** `.git-exclude/reviewed/0.54.0-step-3-part-7b-review-v1.md`. Not yet accepted. Three text fixes, about 20
minutes:
- **T1:** the CHANGELOG entry claims that a compromised AUR push cannot make `paru -S prikk` build unsigned source.
  That is false: an AUR push controls the PKGBUILD. Also, it is two PKGBUILDs per release, not one.
- **T2:** `install.md` should tell the user to read the PKGBUILD (`paru -G prikk`), checking `validpgpkeys` and
  `?signed`, instead of repeating the fingerprint check. `makepkg` already enforces that check.
- **T3:** keep to measured behaviour: the key is on no keyserver, and `makepkg` stops with "unknown public key".

**Where the fixes go:**
- the CHANGELOG fix is a new commit on `main`, on top of `9bac1077`; do not amend it;
- the `install.md` fix goes on `hold/0.54.0-windows-docs`.

`9bac1077` and this note are held locally until 7b2, and pushed together. Report: `0.54.0-step-3-part-7b2-report.md`.
