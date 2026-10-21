# The Windows installer, and a landing page that tells each platform the truth

**Written 2026-10-20. It is 0.54.0 step 3 Part 6, after Part 5.** One item, the interim landing-page correction, is
**not** here: it runs now, with Part 1b (see the step 3 handoff's progress note).

**Why this exists:**
- **The universal installer** (`universal-installer-handoff-v1.md`, 2026-08-31) deferred Windows to *"its own
  increment"*. That increment was considered but never planned.
- **The landing page** (RFC 137 increment 4, 2026-09-05) has since said *"Linux, macOS, and Windows"* under an
  `install.sh` command that refuses on Windows and on Intel Macs.
- **stikk's letter 022** found the gap. **Our owner ruled that installing prikk is prikk's responsibility.** Peripheral
  tools check that prikk is installed and point to our installer.

**The owner's decisions (2026-10-20):**
1. **A Windows installer in 0.54.0:** *"Yes."*
2. **On the landing page, only the visitor's platform is visible by default:** *"tabs are used and only one for the
   user platform is visible by default."*
3. **The false sentence is corrected now:** *"Yes."*

## The working rule (unchanged)

The step 3 handoff's rule applies:
- one sitting per part, one final report each;
- `date` pasted;
- R1 scopes;
- controls shown red;
- all 14 gates;
- the architect pushes on your log;
- **a workflow change is read on CI by the architect.**

**Two sittings:**
- **6a:** the scripts, the generator, release and CI. Report `0.54.0-step-3-part-6a-report.md`.
- **6b:** the landing page and the documentation. Report `0.54.0-step-3-part-6b-report.md`.

## Part 6a — `install.ps1` and `uninstall.ps1` (about 90 min)

1. **Generated, not hand-maintained,** exactly as `install.sh`:
   - new templates beside `tools/release-policy/templates/install.sh.txt`;
   - emitted by `generate-installer`;
   - published by `release.yml` with their own `.sha256`, next to `install.sh` (`release.yml:198-202`).
2. **Behaviour: mirror `install.sh`, platform for platform.** Read the template, and state in the report every place
   the two differ, and why.
   - **The target:** `x86_64-pc-windows-msvc` only. Any other architecture, ARM64 Windows included, refuses with the
     manual route, as `install.sh` refuses Intel Macs.
   - **Overrides:** the same, as environment variables: `PRIKK_INSTALL_VERSION`, `PRIKK_INSTALL_DIR` and
     `PRIKK_INSTALL_BASE_URL`, the last of these for tests.
   - **Verification:** download the zip and its `.sha256` from the release and verify with `Get-FileHash`.
     - The `.sha256` line shape was matched to `sha256sum` on purpose (RFC 107 Stage 2), so compare the hex exactly.
     - **On a mismatch, install nothing.**
     - Print the same statement `install.sh` prints (template line ~182): the checksum proves the file matches what the
       release page published; it does not prove who published it.
   - **The location:** `%LOCALAPPDATA%\Programs\prikk\prikk.exe` by default. This is the folder named to stikk in
     letter 025.
   - **PATH:** the user `Path` only (`[Environment]::SetEnvironmentVariable(…, 'User')`), never the machine's. It is
     added once, only if absent, and the change is recorded in a marker file in the install folder, so uninstall
     knows it added the entry. `install.sh` does not prompt; **follow the same rule,** and say so in the report. If
     you think both should ask first, say so as a question, not a change.
   - **An existing install:** behave as `install.sh` does, and state what that is.
3. **`uninstall.ps1`:**
   - removes `prikk.exe`, the marker, and the folder if it is then empty;
   - removes the PATH entry **only if the marker says the installer added it;**
   - removes nothing it did not add.
4. **It runs where newcomers are:** **Windows PowerShell 5.1** (preinstalled), and PowerShell 7.
   - The one-line form: `irm https://github.com/prikk-vcs/prikk/releases/latest/download/install.ps1 | iex`.
   - **Say from measurement** whether 5.1 needs TLS 1.2 set explicitly.
   - **Say how execution policy affects** the one-line form, and the downloaded-file form.
5. **CI test, on the Windows runner** (a workflow change, so the architect reads the job):
   - build the zip and `.sha256` exactly as `release.yml` does, into a local folder;
   - run `install.ps1` with `PRIKK_INSTALL_BASE_URL` pointing at it;
   - `prikk --version` from the user PATH in a new process;
   - `uninstall.ps1`; then the binary, folder, marker and PATH entry are gone.
   - **Controls shown red:**
     - a corrupted zip installs nothing;
     - a pre-existing user PATH entry is left in place by uninstall;
     - with the marker logic removed, uninstall removes a PATH entry it did not add, and the test catches it.
6. **The policy files:** `release-policy`'s `check` and `reference-check` must know the new assets and templates. Any
   expected-asset list (16 assets today) gains `install.ps1`, `uninstall.ps1` and their `.sha256`.

## Part 6b — the landing page and the documentation (about 60 min)

1. **The landing page's install section becomes tabs** (the owner's decision):
   - **Two tabs:** **macOS / Linux** (the `install.sh` line) and **Windows** (the `install.ps1` line). Each has its own
     copy button.
   - **Only the visitor's platform is visible by default.** Detect it with `navigator.userAgentData.platform` where
     available, and `navigator.userAgent` otherwise. An unknown platform selects the first tab.
   - **Accessible:** the WAI-ARIA tabs pattern (`tablist`, `tab`, `tabpanel`, `aria-selected`, arrow keys), and a
     visible focus.
   - **Without JavaScript,** both commands show, each under its platform's label, so nobody is stuck. The tabs are a
     progressive enhancement.
   - **Each tab states its limits truthfully:**
     - macOS / Linux: Linux x86_64 and aarch64, **macOS on Apple Silicon only**;
     - Windows: x86_64.

     The install guide covers the rest.
   - **Follow RFC 137's own rules for the page:** its size, its no-external-script constraint, and any landing-page
     test or gate. Read `rfcs/done/137-project-entrance-landing-page.md`, and state which rules apply.
2. **`docs/src/guide/install.md`:**
   - a Windows section matching the Linux/macOS one (one line, what it verifies and claims, location, PATH,
     uninstalling, overrides);
   - remove *"A PowerShell equivalent is a separate, later increment"*;
   - one sentence: `prikk.exe` is not code-signed, and Windows may warn or block it. That is stikk's Smart App Control
     note, stated as a fact about signing, not as a measurement.
3. **The CHANGELOG `### Added`:** the Windows installer and uninstaller.
4. **Smoke:** none for Windows (the script is POSIX). Say so; CI's test is the coverage.

## Then

- The architect reviews each part, pushes, and reads the release-workflow change when 0.54.0 is cut. **The first real
  `install.ps1` is the one the 0.54.0 release publishes.** The architect verifies its checksum against the release
  and reads the Windows CI test that ran the same template.
- **stikk's letter 026** (draft) tells stikk that prikk ships its own Windows installer in 0.54.0.

## Progress, 2026-10-22: Part 6a pushed for CI; Part 6a2 before 6b

**Part 6a is pushed** (`2e3675ad`) so its `windows-installer` job runs on a real Windows runner. **It is not yet
accepted** (review `0.54.0-step-3-part-6a-review-v1`).

## Part 6a2 — the installer on a real user's machine (about 60 min, report `0.54.0-step-3-part-6a2-report.md`)

The review's C1–C3, in full there. In short:
1. **C1:**
   - wrap each script's body in one script block and fail with `throw`, never `exit`, so the one-line `irm | iex` form
     leaves nothing in the user's session;
   - test that form in a child session: `$ErrorActionPreference` unchanged, no script variables left, and the session
     still alive after a failed install.
2. **C2:**
   - read and write `HKCU\Environment`'s `Path` raw (no expansion), keeping its value kind, and touch only our entry;
   - announce the change;
   - test that a seeded `%USERPROFILE%\prikk-keep` `ExpandString` entry survives install and uninstall byte for byte.
3. **C3:** run the installer under the same engine the test runs under, and log `$PSVersionTable.PSVersion` from inside
   each run.

**Rulings for Part 6b:**
- **`install.md` says** the installer adds its folder to the user PATH, without a prompt, and that `uninstall` removes
  it only if install added it.
- **`install.md` covers** execution policy for a saved `install.ps1`.
- **`install.sh`'s Windows refusal** points to `install.ps1`.

**Also into Part 6b:** raise the macOS suite's timeout from 30 to 45 minutes. A degraded macOS runner needed about 36
(run `39676219682`: compile 15 m 46 s, against 7 normally).
