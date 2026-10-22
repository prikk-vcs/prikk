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

**The first CI run of `windows-installer` passed** (job `119539225487`): 36 checks, both controls red then green. Part
6a2's conditions stand as written (review addendum).

## Progress, 2026-10-22 (later): Part 6a2 read on CI; Part 6a3 before 6b

**Part 6a2 is pushed** (`9c51c1d3`). On CI, C1 (the `iex` scoping) and C3 (the engine) are proven on Windows. **C2's
new PATH control found a real defect** (review `0.54.0-step-3-part-6a2-review-v1`).
- **The bug:** with exactly one entry in the user PATH, `$pathEntries` is a string, not a one-item array, so `+`
  concatenates. The installer writes one merged entry.
- **Who it hits:** any user whose user PATH holds one entry.

## Part 6a3 — the PATH list bug and the controls (about 45 min, report `0.54.0-step-3-part-6a3-report.md`)

The review's C1–C5, in full there. In short:
1. **C1:** `@( … )` around every pipeline result used as a list, in both scripts. Control 5 passes in both passes.
2. **C2:** two more Control 5 shapes: the user `Path` value absent, and two entries (one `%VAR%`, one literal).
3. **C3:** on a Control 5 failure, print the raw values and kinds before and after each step.
4. **C4:** Control 4's two red runs: the wrapper removed, and an `exit 1` before the checksum throw.
5. **C5:** write the marker before the PATH registry write.

**Then Part 6b** (with its items above, and the macOS timeout).

## Progress, 2026-10-22 (later still): Part 6a3 cut off by the job's limit; Part 6a4

**Part 6a3 is pushed** (`56117f52`). Its CI run reached 38 `PASS` and 0 `FAIL` before the 20-minute limit cut it off
during Control 5 (review `0.54.0-step-3-part-6a3-review-v1`).
- **Control 4's red runs are proven.**
- **The time goes to the test's own server teardown:** `Stop-Job` cannot interrupt `HttpListener.GetContext()`, so it
  waits about 2 minutes per server.

## Part 6a4 — the fixture server stops in seconds (about 30 min, report `0.54.0-step-3-part-6a4-report.md`)

1. **C1:** the fixture server stops within seconds (for example, a `GET /__stop` the loop recognises, then
   `Wait-Job -Timeout 10`). Log each scenario's elapsed seconds.
2. **C2:** the job timeout follows the measured two-pass runtime, with about 2× margin.
3. **C3:** the run completes both passes: Control 5's three shapes green, and the second pass logs
   `running under PowerShell 7.x`. The architect reads that run.

**Then Part 6b.**

## Progress, 2026-10-22 (late morning): Part 6a accepted; Part 6b is live

**Part 6a4 is accepted, and with it Part 6a** (review `0.54.0-step-3-part-6a4-review-v1`): the installer test passed on
CI, with 124 checks, both engines (13 runs each under 5.1 and 7.6) and about a minute per pass.

**Next: Part 6b** (above), carrying:
- **in `install.md`:** the PATH statement (added without a prompt; removed by uninstall only if install added it) and
  execution policy for a saved `install.ps1`;
- `install.sh`'s Windows refusal pointing to `install.ps1`;
- the macOS suite timeout, 30 → 45 minutes.

## Progress, 2026-10-22 (afternoon): Part 6b accepted; Part 6 complete; two docs commits held for the release

**Part 6b is accepted** (review `0.54.0-step-3-part-6b-review-v1`). **Part 6 is complete.**
- **The landing page's tabs, tested by the architect in headless Chromium** with the platform emulated: Windows →
  Windows tab; Linux, macOS and unknown → macOS/Linux; ArrowRight, Home and End all correct; without JavaScript, both
  commands show.
- **Held for the release:** `docs.yml` publishes on push, and `install.ps1` is in no release yet (it returns 404 today).
  The landing page (`c87da024`) and the install guide (`9d34f257`) therefore wait on the local branch
  `hold/0.54.0-windows-docs`. **Release prep cherry-picks both onto `main` just before the tag,** and tightens the
  fallback detection to `/\bWin(dows|32|64)\b/`.
- **Pushed now:** `install.sh`'s refusal, the macOS timeout and the CHANGELOG entry (`49873b5a`, `79362e6b`,
  `b9570af5`).
