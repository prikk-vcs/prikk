# 0.54.1: verifies the real, published install.ps1/uninstall.ps1 behaved correctly when run via
# the documented one-line `irm ... | iex` form against the actual GitHub release page, in
# `.github/workflows/windows-install-from-release.yml` -- not a local fixture server the way
# `scripts/windows-installer-test.ps1` already proves the generated scripts' own logic.
#
# A `.ps1` file, like `windows-installer-test.ps1`, for the same reason: outside
# `scannable_reference_file`'s `md|yml|yaml|sh` set (`tools/release-policy/src/reference.rs`), so
# the real checking logic here is never subject to this project's own closed, line-based command
# grammar. That workflow's own `run:` steps stay simple, exact-match strings instead
# (`tools/release-policy/src/command_scan/procedure.rs`'s `windows_install_from_release_powershell`/
# `_pwsh`).
#
# Written to run correctly under both Windows PowerShell 5.1 and PowerShell 7 -- no ternary (?:),
# no null-coalescing (??), no pipeline-chain operators (&&/||).
#
# No PowerShell interpreter was available in the development environment this script was written
# in (no `pwsh`/`powershell` on the Linux machine used for this round -- checked, not assumed), so
# this script has only been reviewed by hand against install.ps1/uninstall.ps1's own logic and
# against documented PowerShell/.NET semantics; its first real execution is this CI job, which the
# architect reads per the handoff's own working rule ("a workflow change is read on CI by the
# architect").

param(
  [Parameter(Mandatory = $true)]
  [ValidateSet("PostInstall", "PostUninstall")]
  [string]$Mode
)

$ErrorActionPreference = 'Stop'
$script:FailureCount = 0

function Assert-True([bool]$Condition, [string]$Message) {
  if ($Condition) {
    Write-Host "  PASS: $Message" -ForegroundColor Green
  } else {
    Write-Host "  FAIL: $Message" -ForegroundColor Red
    $script:FailureCount = $script:FailureCount + 1
  }
}

# The same technique scripts/windows-installer-test.ps1's own Invoke-WithReconstructedPath already
# uses: a genuinely new process, with its PATH built directly from the Machine+User registry
# values, rather than whatever this already-running process happens to have inherited. That is the
# only way to prove the one-line install's own PATH change took effect for a new process, the way
# a real user's next terminal would see it -- reused verbatim here, not reinvented, per the
# handoff's own instruction.
function Invoke-WithReconstructedPath([string[]]$ArgumentList) {
  $machinePath = [Environment]::GetEnvironmentVariable('Path', 'Machine')
  $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  $combinedPath = "$machinePath;$userPath"
  $psi = New-Object System.Diagnostics.ProcessStartInfo
  $psi.FileName = "cmd.exe"
  $psi.Arguments = "/c " + ($ArgumentList -join " ")
  $psi.UseShellExecute = $false
  $psi.RedirectStandardOutput = $true
  $psi.RedirectStandardError = $true
  $psi.EnvironmentVariables["Path"] = $combinedPath
  $process = [System.Diagnostics.Process]::Start($psi)
  $stdout = $process.StandardOutput.ReadToEnd()
  $stderr = $process.StandardError.ReadToEnd()
  $process.WaitForExit()
  return @{
    ExitCode = $process.ExitCode
    StdOut   = $stdout
    StdErr   = $stderr
  }
}

if ($Mode -eq "PostInstall") {
  # Set by the calling workflow step's own `env:` block (not by this script's own command line --
  # a bare `$env:VAR` reference inside a `run:` string would split on PowerShell's own `:`, so the
  # workflow instead reads these from the process environment, which needs no such reference at
  # all in its own scanned command text).
  $expectedTag = $env:EXPECTED_TAG
  if (-not $expectedTag) {
    throw "EXPECTED_TAG was not set -- the calling workflow step must set it"
  }
  $expectEmulationLine = $env:EXPECT_EMULATION_LINE -eq "true"

  $versionResult = Invoke-WithReconstructedPath -ArgumentList @("prikk.exe", "--version")
  Assert-True ($versionResult.ExitCode -eq 0) "'prikk --version' succeeds from a reconstructed-PATH new process"
  Assert-True ($versionResult.StdOut.Contains($expectedTag)) "'prikk --version' ($($versionResult.StdOut.Trim())) prints this call's own expected tag ($expectedTag) -- catches 'latest' pointing anywhere else"

  # Written by the install step itself (Start-Transcript/Stop-Transcript around the exact
  # documented one-line form) -- this script never runs the installer; it only reads what that
  # step already captured.
  $transcriptPath = "install-output.log"
  if (-not (Test-Path -Path $transcriptPath -PathType Leaf)) {
    throw "install-output.log not found -- the install step must write it before this script runs"
  }
  $transcriptContent = Get-Content -Path $transcriptPath -Raw
  $hasEmulationLine = $transcriptContent.Contains("x64 emulation")
  if ($expectEmulationLine) {
    Assert-True $hasEmulationLine "the ARM64 emulation line appeared in the install's own output"
  } else {
    Assert-True (-not $hasEmulationLine) "no emulation line appeared in the install's own output (this runner is not ARM64)"
  }
} else {
  # install.ps1's own default install directory (no PRIKK_INSTALL_DIR override anywhere in this
  # job -- the documented one-line form uses its defaults throughout, deliberately).
  $installDir = Join-Path $env:LOCALAPPDATA "Programs\prikk"
  $binaryPath = Join-Path $installDir "prikk.exe"
  Assert-True (-not (Test-Path -Path $binaryPath -PathType Leaf)) "prikk.exe is gone after uninstall.ps1 ran via the one-line form"
}

if ($script:FailureCount -gt 0) {
  Write-Host "$($script:FailureCount) assertion(s) failed" -ForegroundColor Red
  exit 1
} else {
  Write-Host "All assertions passed" -ForegroundColor Green
  exit 0
}
