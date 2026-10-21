# Windows installer/uninstaller handoff v1 Part 6a, item 5: the CI test for install.ps1/
# uninstall.ps1, run on the Windows runner. Not scanned by `reference-check`/`boundary-check`'s
# strict shell grammar (`.ps1` is outside `scannable_reference_file`'s `md|yml|yaml|sh` set,
# `tools/release-policy/src/reference.rs`), the same reason `scripts/gates.py` is a `.py` file
# rather than a `.sh` one (see that script's own top-of-file comment): this test needs real
# branching (scenarios, red/green controls, loops over PATH entries), which the project's own
# closed, line-based command grammar cannot express.
#
# Written to run correctly under both Windows PowerShell 5.1 and PowerShell 7 -- no ternary (?:),
# no null-coalescing (??), no pipeline-chain operators (&&/||), all PowerShell 7-only syntax. The
# CI job invokes this script twice, once under each engine (`powershell.exe` and `pwsh`), so the
# whole cycle below -- happy path and all three controls -- runs for real under both.
#
# No PowerShell interpreter was available in the development environment this script was written
# in (no `pwsh`/`powershell` on the Linux machine used for this round -- checked, not assumed), so
# this script has only been reviewed by hand against install.ps1/uninstall.ps1's own logic; its
# first real execution is this CI job, which the architect reads per the handoff's own working
# rule ("a workflow change is read on CI by the architect").

param(
  [Parameter(Mandatory = $true)]
  [string]$DistDir
)

$ErrorActionPreference = 'Stop'
$script:FailureCount = 0

function Write-Section([string]$Title) {
  Write-Host ""
  Write-Host "==== $Title ====" -ForegroundColor Cyan
}

function Assert-True([bool]$Condition, [string]$Message) {
  if ($Condition) {
    Write-Host "  PASS: $Message" -ForegroundColor Green
  } else {
    Write-Host "  FAIL: $Message" -ForegroundColor Red
    $script:FailureCount = $script:FailureCount + 1
  }
}

# A minimal static file server, started in its own background job (its own process, in Windows
# PowerShell -- a live .NET object such as HttpListener cannot cross that boundary, so the
# listener is created *inside* the job's own scriptblock, not passed in). Binding to
# "http://localhost:<port>/" (not a wildcard host) does not require administrator rights or a URL
# ACL reservation on Windows -- a documented HttpListener exception, not something this script
# measured here.
function Start-StaticFileServer([string]$Directory, [int]$Port) {
  $job = Start-Job -ScriptBlock {
    param($Directory, $Port)
    $listener = New-Object System.Net.HttpListener
    $listener.Prefixes.Add("http://localhost:$Port/")
    $listener.Start()
    while ($listener.IsListening) {
      try {
        $context = $listener.GetContext()
      } catch {
        break
      }
      $requestPath = $context.Request.Url.AbsolutePath.TrimStart('/')
      $filePath = Join-Path $Directory $requestPath
      if (Test-Path -Path $filePath -PathType Leaf) {
        $bytes = [System.IO.File]::ReadAllBytes($filePath)
        $context.Response.ContentLength64 = $bytes.Length
        $context.Response.OutputStream.Write($bytes, 0, $bytes.Length)
      } else {
        $context.Response.StatusCode = 404
      }
      $context.Response.OutputStream.Close()
    }
  } -ArgumentList $Directory, $Port
  Start-Sleep -Milliseconds 750
  return $job
}

function Stop-StaticFileServer($Job) {
  Stop-Job -Job $Job -ErrorAction SilentlyContinue | Out-Null
  Remove-Job -Job $Job -Force -ErrorAction SilentlyContinue | Out-Null
}

# Proves a Windows PATH change took effect for a genuinely new process, not merely the already-
# running script's own process (which would not see it either way, since neither in-process
# inheritance nor this project's test harness has a real logoff/logon cycle available in CI).
# Windows itself builds a new session's PATH by concatenating the Machine and User values from the
# registry; this reconstructs that exact concatenation explicitly, rather than trusting whatever
# environment a child process happens to inherit from its parent, which would prove nothing new.
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

function Remove-UserPathEntryIfPresent([string]$Entry) {
  $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  if ($userPath) {
    $entries = $userPath -split ';' | Where-Object { $_ -ne '' -and $_ -ne $Entry }
    [Environment]::SetEnvironmentVariable('Path', ($entries -join ';'), 'User')
  }
}

function Test-UserPathContains([string]$Entry) {
  $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  if (-not $userPath) {
    return $false
  }
  $entries = $userPath -split ';' | Where-Object { $_ -ne '' }
  return ($entries -contains $Entry)
}

$distDirResolved = (Resolve-Path $DistDir).Path
$assetName = "prikk-x86_64-pc-windows-msvc.zip"
$assetPath = Join-Path $distDirResolved $assetName
$checksumPath = Join-Path $distDirResolved "$assetName.sha256"
$installScriptPath = Join-Path $distDirResolved "install.ps1"
$uninstallScriptPath = Join-Path $distDirResolved "uninstall.ps1"

if (-not (Test-Path -Path $assetPath -PathType Leaf)) {
  throw "fixture zip not found at $assetPath -- the packaging step must run before this script"
}
if (-not (Test-Path -Path $installScriptPath -PathType Leaf)) {
  throw "install.ps1 not found at $installScriptPath -- generate-installer must run before this script"
}

$scratchRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("prikk-installer-test-" + [System.IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Force -Path $scratchRoot | Out-Null

# ---------------------------------------------------------------------------
# Scenario A: the happy path -- install, confirm from a new process, uninstall.
# ---------------------------------------------------------------------------
Write-Section "Scenario A: install, verify from a new process, uninstall"

$installDirA = Join-Path $scratchRoot "install-a"
$env:PRIKK_INSTALL_DIR = $installDirA
$env:PRIKK_INSTALL_BASE_URL = "http://localhost:18080"
$serverA = Start-StaticFileServer -Directory $distDirResolved -Port 18080
try {
  & powershell -NoProfile -ExecutionPolicy Bypass -File $installScriptPath
  Assert-True ($LASTEXITCODE -eq 0) "install.ps1 exited 0"
  Assert-True (Test-Path -Path (Join-Path $installDirA "prikk.exe") -PathType Leaf) "prikk.exe is present after install"
  Assert-True (Test-Path -Path (Join-Path $installDirA "path-added-by-install.marker") -PathType Leaf) "the PATH marker file was written"
  Assert-True (Test-UserPathContains $installDirA) "the install directory is on the user PATH"

  $versionResult = Invoke-WithReconstructedPath -ArgumentList @("prikk.exe", "--version")
  Assert-True ($versionResult.ExitCode -eq 0) "'prikk --version' succeeds from a reconstructed-PATH new process"
  Assert-True ($versionResult.StdOut.Trim().Length -gt 0) "'prikk --version' printed something"

  & powershell -NoProfile -ExecutionPolicy Bypass -File $uninstallScriptPath
  Assert-True ($LASTEXITCODE -eq 0) "uninstall.ps1 exited 0"
  Assert-True (-not (Test-Path -Path (Join-Path $installDirA "prikk.exe"))) "prikk.exe is gone after uninstall"
  Assert-True (-not (Test-Path -Path (Join-Path $installDirA "path-added-by-install.marker"))) "the marker file is gone after uninstall"
  Assert-True (-not (Test-Path -Path $installDirA)) "the install folder itself is gone (it was left empty)"
  Assert-True (-not (Test-UserPathContains $installDirA)) "the install directory is no longer on the user PATH"
} finally {
  Stop-StaticFileServer $serverA
  Remove-Item Env:\PRIKK_INSTALL_DIR -ErrorAction SilentlyContinue
  Remove-Item Env:\PRIKK_INSTALL_BASE_URL -ErrorAction SilentlyContinue
  Remove-UserPathEntryIfPresent $installDirA
}

# ---------------------------------------------------------------------------
# Control 1: a corrupted zip must install nothing. Shown red first: a perturbed install.ps1 with
# the checksum comparison forced to "always matches" is run against the same corrupted zip, and
# must be seen to install anyway -- proving the control can fail -- before the real,
# unperturbed install.ps1 is run against it and must refuse.
# ---------------------------------------------------------------------------
Write-Section "Control 1: corrupted zip (red before green)"

$control1Dir = Join-Path $scratchRoot "control1-server"
New-Item -ItemType Directory -Force -Path $control1Dir | Out-Null
Copy-Item -Path $checksumPath -Destination (Join-Path $control1Dir "$assetName.sha256")
$corruptBytes = [System.IO.File]::ReadAllBytes($assetPath)
# Flip one byte roughly in the middle of the archive -- enough to break both the hash and the zip
# central directory, without needing the file to still be a well-formed (but wrong) zip.
$flipIndex = [int]($corruptBytes.Length / 2)
$corruptBytes[$flipIndex] = $corruptBytes[$flipIndex] -bxor 0xFF
[System.IO.File]::WriteAllBytes((Join-Path $control1Dir $assetName), $corruptBytes)

$installScriptContent = Get-Content -Path $installScriptPath -Raw
$perturbedMarkerText = 'if ($actualHash -ne $expectedHash) {'
$perturbedReplacementText = 'if ($false) {'
if (-not $installScriptContent.Contains($perturbedMarkerText)) {
  throw "control 1's perturbation text was not found in install.ps1 -- the template changed shape; update this script"
}
$perturbedInstallContent = $installScriptContent.Replace($perturbedMarkerText, $perturbedReplacementText)
$perturbedInstallPath = Join-Path $scratchRoot "install-perturbed-control1.ps1"
Set-Content -Path $perturbedInstallPath -Value $perturbedInstallContent

$control1DirRed = Join-Path $scratchRoot "install-control1-red"
$env:PRIKK_INSTALL_DIR = $control1DirRed
$env:PRIKK_INSTALL_BASE_URL = "http://localhost:18081"
$serverControl1 = Start-StaticFileServer -Directory $control1Dir -Port 18081
try {
  & powershell -NoProfile -ExecutionPolicy Bypass -File $perturbedInstallPath
  Assert-True (Test-Path -Path (Join-Path $control1DirRed "prikk.exe") -PathType Leaf) "RED: the perturbed install.ps1 (checksum check disabled) installs the corrupted zip's contents anyway -- the control can fail"
} finally {
  Remove-UserPathEntryIfPresent $control1DirRed
}

$control1DirGreen = Join-Path $scratchRoot "install-control1-green"
$env:PRIKK_INSTALL_DIR = $control1DirGreen
try {
  & powershell -NoProfile -ExecutionPolicy Bypass -File $installScriptPath
  Assert-True ($LASTEXITCODE -ne 0) "GREEN: the real install.ps1 exits non-zero against the corrupted zip"
  Assert-True (-not (Test-Path -Path (Join-Path $control1DirGreen "prikk.exe"))) "GREEN: the real install.ps1 installs nothing from the corrupted zip"
} finally {
  Stop-StaticFileServer $serverControl1
  Remove-Item Env:\PRIKK_INSTALL_DIR -ErrorAction SilentlyContinue
  Remove-Item Env:\PRIKK_INSTALL_BASE_URL -ErrorAction SilentlyContinue
  Remove-UserPathEntryIfPresent $control1DirGreen
}

# ---------------------------------------------------------------------------
# Controls 2 and 3 together: a PATH entry this installer did not add (no marker file next to it)
# must be left alone by uninstall.ps1. Shown red first: a perturbed uninstall.ps1 with the
# marker-presence check forced to "always true" is run against this entry, and must be seen to
# remove it anyway -- proving the control can fail -- before the real, unperturbed uninstall.ps1
# is run against an identical setup and must leave it in place.
# ---------------------------------------------------------------------------
Write-Section "Controls 2+3: a pre-existing, unmarked PATH entry (red before green)"

$uninstallScriptContent = Get-Content -Path $uninstallScriptPath -Raw
$perturbedUninstallMarkerText = 'if (Test-Path -Path $markerPath -PathType Leaf) {'
$perturbedUninstallReplacementText = 'if ($true) {'
if (-not $uninstallScriptContent.Contains($perturbedUninstallMarkerText)) {
  throw "controls 2+3's perturbation text was not found in uninstall.ps1 -- the template changed shape; update this script"
}
$perturbedUninstallContent = $uninstallScriptContent.Replace($perturbedUninstallMarkerText, $perturbedUninstallReplacementText)
# The real script's own marker-removal line assumes the marker exists (reached only when the
# marker-presence check passed); forcing that check to always-true means this line now runs even
# when there is no marker file on disk, so it must not abort the perturbation's own red
# demonstration with an unrelated "file not found" error.
$perturbedUninstallContent = $perturbedUninstallContent.Replace(
  'Remove-Item -Path $markerPath -Force',
  'Remove-Item -Path $markerPath -Force -ErrorAction SilentlyContinue'
)
$perturbedUninstallPath = Join-Path $scratchRoot "uninstall-perturbed-control23.ps1"
Set-Content -Path $perturbedUninstallPath -Value $perturbedUninstallContent

$control23Dir = Join-Path $scratchRoot "control23-dir"
New-Item -ItemType Directory -Force -Path $control23Dir | Out-Null
# No marker file is ever written in $control23Dir -- this simulates a user who already had this
# directory on their PATH for a reason unrelated to prikk, never installed by install.ps1.

$env:PRIKK_INSTALL_DIR = $control23Dir
$existingUserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$newUserPath = if ($existingUserPath) { "$existingUserPath;$control23Dir" } else { $control23Dir }
[Environment]::SetEnvironmentVariable('Path', $newUserPath, 'User')
try {
  Assert-True (Test-UserPathContains $control23Dir) "setup: the unrelated entry is on the user PATH before either uninstall runs"

  & powershell -NoProfile -ExecutionPolicy Bypass -File $perturbedUninstallPath
  Assert-True (-not (Test-UserPathContains $control23Dir)) "RED: the perturbed uninstall.ps1 (marker check forced true) wrongly removes the unrelated PATH entry -- the control can fail"

  # Restore the entry (the red run above removed it) before exercising the real script.
  $existingUserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  $restoredUserPath = if ($existingUserPath) { "$existingUserPath;$control23Dir" } else { $control23Dir }
  [Environment]::SetEnvironmentVariable('Path', $restoredUserPath, 'User')
  Assert-True (Test-UserPathContains $control23Dir) "setup: the unrelated entry is restored before the real uninstall.ps1 runs"

  & powershell -NoProfile -ExecutionPolicy Bypass -File $uninstallScriptPath
  Assert-True (Test-UserPathContains $control23Dir) "GREEN: the real uninstall.ps1 leaves the unrelated, unmarked PATH entry in place"
} finally {
  Remove-Item Env:\PRIKK_INSTALL_DIR -ErrorAction SilentlyContinue
  Remove-UserPathEntryIfPresent $control23Dir
  Remove-Item -Path $control23Dir -Recurse -Force -ErrorAction SilentlyContinue
}

Remove-Item -Path $scratchRoot -Recurse -Force -ErrorAction SilentlyContinue

Write-Section "Summary"
if ($script:FailureCount -gt 0) {
  Write-Host "$($script:FailureCount) assertion(s) failed" -ForegroundColor Red
  exit 1
} else {
  Write-Host "All assertions passed" -ForegroundColor Green
  exit 0
}
