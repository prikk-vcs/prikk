# 0.54.1: the Windows installer refuses x64 machines under Windows PowerShell 5.1

**Written 2026-10-24. Live, and next.** It is a patch release. Per `release-compatibility.md`, a release that changes no
runtime source (installer, metadata, docs) is a patch; precedents 0.27.1 and 0.31.1. RFC 152: *"a mistake ships as
the next patch release"*. The 0.54.0 tag and assets are not touched.

**The owner decided (2026-10-24):**
1. **0.54.1 soon:** the fix, plus a release-workflow job that runs the real one-line install from the release page.
2. **On ARM64 Windows, install the x64 build** and say, in one line, that it runs under Windows' x64 emulation.

## What happened

**The owner ran the documented line on a real x64 Windows 11 machine,** in Windows PowerShell 5.1.26100:

```
> irm https://github.com/prikk-vcs/prikk/releases/latest/download/install.ps1 | iex
prikk-install: running under PowerShell 5.1.26100.9711
prikk-install: download prikk-x86_64-pc-windows-msvc.zip from the release page by hand,
prikk-install: or run 'cargo install prikk'
prikk-install: no prebuilt binary for Windows/ (only x86_64 ships one)
```

**`$env:PROCESSOR_ARCHITECTURE` is `AMD64` on that machine.**

**The cause, from `tools/release-policy/templates/install.ps1.txt` (lines 185–196):**
- The check is `[System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq [...Architecture]::X64`, in a
  `try`, with `$env:PROCESSOR_ARCHITECTURE` as the `catch` fallback.
- On this machine the fallback never ran. With `AMD64` it would have installed. So the .NET expression **raised no
  error and yielded nothing**: the refusal printed `Windows/` with an empty architecture. In Windows PowerShell, reading
  a member that does not resolve gives `$null` silently, and `$null -eq X64` is false.
- **GitHub's `windows-latest` runner answers that expression properly.** That is why the CI job passed 13 runs under 5.1,
  and why only a real machine found it.

**The lesson for the design:** a check whose failure is silent must not be what decides a refusal.

## 1. The architecture check, rewritten

- **Decide from Windows' own variables, with no .NET call:**
  - `PROCESSOR_ARCHITEW6432` when it is set: a 32-bit process on 64-bit Windows sees the OS's architecture there;
  - otherwise `PROCESSOR_ARCHITECTURE`.

  Values: `AMD64`, `ARM64`, `x86`.
- **`AMD64`:** install the x64 build, as today.
- **`ARM64`:** install the x64 build, and print one line before the download:
  *`prikk-install: this is ARM64 Windows; installing the x64 build, which runs under Windows' x64 emulation`* (or your
  words).
- **Anything else** (`x86`, empty, unknown): refuse, **naming the value** (`unknown` if empty), and point at the zip and
  `cargo install prikk`. The message is built from the same variable the decision used. Never re-evaluate a second
  expression for it.
- **Replace the template's comment above the check** with the reason for the new rule. Remove the .NET claims,
  including "confirmed from .NET's own documentation, not measured here".
- **`uninstall.ps1`** has no architecture check, so nothing changes there.

## 2. Tests (in `scripts/windows-installer-test.ps1`, both engines)

The child process inherits the parent's environment, so set the variables before each launch:

| `PROCESSOR_ARCHITECTURE` | `PROCESSOR_ARCHITEW6432` | expected |
|---|---|---|
| `AMD64` | unset | installs; no emulation line |
| `x86` | `AMD64` | installs; no emulation line |
| `ARM64` | unset | installs; the emulation line is printed |
| `x86` | unset | refuses, naming `x86`; nothing installed |
| empty | unset | refuses, naming `unknown`; nothing installed |

**Red first:** with the current template, the `ARM64` row and the `x86`-only row must fail. On the runner, .NET says X64,
so the old code installs where the row expects otherwise. Show those reds in your report, then green.

## 3. A release-workflow job: install from the real release page

**In `.github/workflows/release.yml`, a job `windows-install-from-release`:**
- `needs: publish`; `permissions: contents: read`; no secrets.
- **A matrix of runners and engines:**
  - runners `windows-latest` and **`windows-11-arm`**. Check that label is available to this public repository. If it
    is not, stop and report;
  - engines Windows PowerShell 5.1 (`powershell`) and PowerShell 7 (`pwsh`). Check `pwsh` exists on the ARM image.
- **Each cell runs the documented line exactly:**
  `irm https://github.com/prikk-vcs/prikk/releases/latest/download/install.ps1 | iex`.
  Then, from a **new** process built with the registry PATH (as the existing CI test does):
  - `prikk --version` must print **this tag's version**. That catches "latest" pointing anywhere else;
  - on ARM64 the emulation line must appear in the install output;
  - then run `uninstall.ps1` from the release the same way, and check `prikk` is gone.
- **A failure here never touches the published release.** It runs after `publish`, like the AUR jobs.
- **A `workflow_dispatch` entry,** like `aur-build.yml`'s, runs the same cells against an existing tag. Its first use is
  0.54.0. Expect `windows-latest` to install, because the runner's .NET answers. Expect the ARM cell to refuse with an
  empty architecture: that is 0.54.0's bug, shown on CI.
- **`command_scan`:** `irm … | iex` is opaque execution by design. Pin the exact line in `procedure.rs`, as the AUR
  jobs' `su -c` strings are pinned, and say so in the report.

## 4. Text

- **`CHANGELOG.md`:** a new `## Unreleased` with `### Fixed`. The entry says:
  - the Windows installer refused x64 machines under Windows PowerShell 5.1 where .NET's architecture query answered
    nothing;
  - it now decides from Windows' own processor variables;
  - on ARM64 it installs the x64 build, which runs under emulation.

  Name the owner's report as "found on a real Windows 11 machine". No process words.
- **The install guide:** commit on **`hold/0.54.0-windows-docs`**, not `main`. One sentence in the Windows section:
  ARM64 Windows gets the x64 build, which runs under Windows' x64 emulation.

## 5. The release commit (in the same sitting, after 1–4 are green)

- the version becomes **0.54.1** in `Cargo.toml` (all eight entries) and the lock;
- the CHANGELOG heading becomes `## 0.54.1 — <today>`;
- **the 14 gates on the final commit.** No runtime source changes, so P2/P3-style measurement and the matrix are not
  needed. Say "no file under `crates/` changed", and show it with `git diff --stat 0.54.0..HEAD -- crates`.

## The working rule

- One sitting, one report (`0.54.1-windows-installer-report.md`); R1 scopes; controls shown red; all 14 gates.
- **A workflow change is read on CI by the architect.** The new job's dispatch run against 0.54.0 is the architect's
  step, after your push.
- Commit on `main`, except the guide sentence, which goes on the held branch.

## Then (the architect, and the owner)

1. The architect reviews, pushes, and dispatches the new job against 0.54.0. Expect: x64 installs, and the ARM cell
   shows the bug.
2. The architect runs the gates on the tip and reads CI.
3. The owner's word on the tag `0.54.1`, and separately on crates.io.
4. **The 0.54.1 release run** builds, publishes, runs `aur-build`/`aur-push` (if the AUR key is fixed by then), and runs
   the new install job on x64 and ARM64. The architect reads every job.
5. **The held docs are cherry-picked by hash,** with the new ARM64 sentence, once `install.ps1` works on the release
   page and the AUR pages show the release.
6. The owner tries the one-line install again on the same machine.
