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
# whole cycle below -- happy path and all controls -- runs for real under both.
#
# No PowerShell interpreter was available in the development environment this script was written
# in (no `pwsh`/`powershell` on the Linux machine used for this round -- checked, not assumed), so
# this script has only been reviewed by hand against install.ps1/uninstall.ps1's own logic; its
# first real execution is this CI job, which the architect reads per the handoff's own working
# rule ("a workflow change is read on CI by the architect").
#
# Part 6a2 (review 0.54.0-step-3-part-6a-review-v1, condition C3): every installer invocation below
# launches `$enginePath`, not a hardcoded `powershell` -- `(Get-Process -Id $PID).Path` is the
# current process's own executable path, `powershell.exe` when this test script itself runs under
# Windows PowerShell 5.1 and `pwsh.exe` when it runs under PowerShell 7 (the review's own suggested
# mechanism). Before this fix, every installer run here was launched as literal `powershell`
# regardless of which engine ran this test script, so the CI job's own "PowerShell 7" pass never
# actually exercised install.ps1/uninstall.ps1 under PowerShell 7 at all, only under 5.1, twice.
# `$LASTEXITCODE` keeps working unchanged either way: it is set by invoking an external executable,
# which `& $enginePath -File ...` still is regardless of which engine `$enginePath` resolves to.

param(
  [Parameter(Mandatory = $true)]
  [string]$DistDir
)

$ErrorActionPreference = 'Stop'
$script:FailureCount = 0
$enginePath = (Get-Process -Id $PID).Path

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

# Part 6a2, condition C2: the raw (unexpanded) registry read/write this test uses to verify
# install.ps1/uninstall.ps1 themselves do the same -- see either template's own comment for the
# citations (RegistryKey.GetValue's default-expansion behavior, RegistryValueOptions.
# DoNotExpandEnvironmentNames, RegistryValueKind, Environment.SetEnvironmentVariable's own
# signature). This test-local copy exists because the templates' own Get-RawUserPathEntry/
# Set-RawUserPathEntry functions are private to each generated script's own `& { }` scope (Part
# 6a2, condition C1) and cannot be called from this outer test process.
function Get-RawUserPathForTest {
  $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $false)
  if ($null -eq $key) {
    return @{ Value = ""; Kind = [Microsoft.Win32.RegistryValueKind]::String; Existed = $false }
  }
  try {
    $exists = $false
    foreach ($name in $key.GetValueNames()) {
      if ($name -eq 'Path') { $exists = $true; break }
    }
    if (-not $exists) {
      return @{ Value = ""; Kind = [Microsoft.Win32.RegistryValueKind]::String; Existed = $false }
    }
    $kind = $key.GetValueKind('Path')
    $raw = $key.GetValue('Path', "", [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    return @{ Value = [string]$raw; Kind = $kind; Existed = $true }
  } finally {
    $key.Close()
  }
}

function Set-RawUserPathForTest([string]$Value, [Microsoft.Win32.RegistryValueKind]$Kind) {
  $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
  try {
    $key.SetValue('Path', $Value, $Kind)
  } finally {
    $key.Close()
  }
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
  & $enginePath -NoProfile -ExecutionPolicy Bypass -File $installScriptPath
  Assert-True ($LASTEXITCODE -eq 0) "install.ps1 exited 0"
  Assert-True (Test-Path -Path (Join-Path $installDirA "prikk.exe") -PathType Leaf) "prikk.exe is present after install"
  Assert-True (Test-Path -Path (Join-Path $installDirA "path-added-by-install.marker") -PathType Leaf) "the PATH marker file was written"
  Assert-True (Test-UserPathContains $installDirA) "the install directory is on the user PATH"

  $versionResult = Invoke-WithReconstructedPath -ArgumentList @("prikk.exe", "--version")
  Assert-True ($versionResult.ExitCode -eq 0) "'prikk --version' succeeds from a reconstructed-PATH new process"
  Assert-True ($versionResult.StdOut.Trim().Length -gt 0) "'prikk --version' printed something"

  & $enginePath -NoProfile -ExecutionPolicy Bypass -File $uninstallScriptPath
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
  & $enginePath -NoProfile -ExecutionPolicy Bypass -File $perturbedInstallPath
  Assert-True (Test-Path -Path (Join-Path $control1DirRed "prikk.exe") -PathType Leaf) "RED: the perturbed install.ps1 (checksum check disabled) installs the corrupted zip's contents anyway -- the control can fail"
} finally {
  Remove-UserPathEntryIfPresent $control1DirRed
}

$control1DirGreen = Join-Path $scratchRoot "install-control1-green"
$env:PRIKK_INSTALL_DIR = $control1DirGreen
try {
  & $enginePath -NoProfile -ExecutionPolicy Bypass -File $installScriptPath
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

  & $enginePath -NoProfile -ExecutionPolicy Bypass -File $perturbedUninstallPath
  Assert-True (-not (Test-UserPathContains $control23Dir)) "RED: the perturbed uninstall.ps1 (marker check forced true) wrongly removes the unrelated PATH entry -- the control can fail"

  # Restore the entry (the red run above removed it) before exercising the real script.
  $existingUserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  $restoredUserPath = if ($existingUserPath) { "$existingUserPath;$control23Dir" } else { $control23Dir }
  [Environment]::SetEnvironmentVariable('Path', $restoredUserPath, 'User')
  Assert-True (Test-UserPathContains $control23Dir) "setup: the unrelated entry is restored before the real uninstall.ps1 runs"

  & $enginePath -NoProfile -ExecutionPolicy Bypass -File $uninstallScriptPath
  Assert-True (Test-UserPathContains $control23Dir) "GREEN: the real uninstall.ps1 leaves the unrelated, unmarked PATH entry in place"
} finally {
  Remove-Item Env:\PRIKK_INSTALL_DIR -ErrorAction SilentlyContinue
  Remove-UserPathEntryIfPresent $control23Dir
  Remove-Item -Path $control23Dir -Recurse -Force -ErrorAction SilentlyContinue
}

# ---------------------------------------------------------------------------
# Control 4 (C1): the one-line `irm | iex` form runs in the caller's own session, with nothing
# protecting it, unless install.ps1/uninstall.ps1 wrap their own bodies in a script block and fail
# with `throw`, never `exit` (review 0.54.0-step-3-part-6a-review-v1). This control evaluates the
# served install.ps1 TEXT with Invoke-Expression inside a genuinely separate child PowerShell
# PROCESS (not merely a nested scriptblock in this test script's own process) -- the point is to
# prove the *session* a real `irm | iex` invocation runs in stays clean, which a `-File`
# invocation (used everywhere else in this test) cannot prove, since `-File` already gets its own
# new process and script scope regardless of install.ps1's own fix.
# ---------------------------------------------------------------------------
Write-Section "Control 4: the one-line form, in a child session (C1)"

$childScriptsDir = Join-Path $scratchRoot "control4-child-scripts"
New-Item -ItemType Directory -Force -Path $childScriptsDir | Out-Null

# --- Control 4a: the happy path, evaluated via iex in a child session --------------------------
$installDir4a = Join-Path $scratchRoot "install-control4a"
$env:PRIKK_INSTALL_DIR = $installDir4a
$env:PRIKK_INSTALL_BASE_URL = "http://localhost:18082"
$server4a = Start-StaticFileServer -Directory $distDirResolved -Port 18082
try {
  # `Test-Path Variable:\<name>` (the Variable: PSDrive, about_Scopes's own table: "Variable:
  # Variables defined in the current scope") is used to check that $repo/$version/$installDir do
  # not exist in this child session after the one-line install -- not `$null -eq $repo`, which
  # reads $true even for a variable that was never created at all (an undefined variable simply
  # reads as $null in PowerShell, by default, with no error), so it cannot distinguish "never
  # created" from "created and set to $null"; `Test-Path Variable:\repo` can.
  $child4aTemplate = @'
$before = $ErrorActionPreference
$scriptText = Get-Content -Path '__INSTALL_SCRIPT_PATH__' -Raw
Invoke-Expression $scriptText
$after = $ErrorActionPreference
Write-Output "PRIKK_BEFORE=$before"
Write-Output "PRIKK_AFTER=$after"
Write-Output "PRIKK_REPO_DEFINED=$(Test-Path Variable:\repo)"
Write-Output "PRIKK_VERSION_DEFINED=$(Test-Path Variable:\version)"
Write-Output "PRIKK_INSTALLDIR_DEFINED=$(Test-Path Variable:\installDir)"
'@
  $child4aContent = $child4aTemplate.Replace('__INSTALL_SCRIPT_PATH__', $installScriptPath)
  $child4aPath = Join-Path $childScriptsDir "child-4a.ps1"
  Set-Content -Path $child4aPath -Value $child4aContent

  $output4a = & $enginePath -NoProfile -File $child4aPath
  $exitCode4a = $LASTEXITCODE
  Assert-True ($exitCode4a -eq 0) "4a: the child session's own process exited 0 after the one-line install via iex"
  Assert-True (Test-Path -Path (Join-Path $installDir4a "prikk.exe") -PathType Leaf) "4a: prikk.exe is present after the one-line install via iex"

  $before4a = ($output4a | Where-Object { $_ -like "PRIKK_BEFORE=*" }) -replace '^PRIKK_BEFORE=', ''
  $after4a = ($output4a | Where-Object { $_ -like "PRIKK_AFTER=*" }) -replace '^PRIKK_AFTER=', ''
  Assert-True (($before4a.Length -gt 0) -and ($before4a -eq $after4a)) "4a: the child session's own ErrorActionPreference ($before4a) is unchanged after the one-line install via iex"

  $repoDefined4a = ($output4a | Where-Object { $_ -like "PRIKK_REPO_DEFINED=*" }) -replace '^PRIKK_REPO_DEFINED=', ''
  $versionDefined4a = ($output4a | Where-Object { $_ -like "PRIKK_VERSION_DEFINED=*" }) -replace '^PRIKK_VERSION_DEFINED=', ''
  $installDirDefined4a = ($output4a | Where-Object { $_ -like "PRIKK_INSTALLDIR_DEFINED=*" }) -replace '^PRIKK_INSTALLDIR_DEFINED=', ''
  Assert-True ($repoDefined4a -eq "False") "4a: `$repo does not exist in the child session after the one-line install via iex"
  Assert-True ($versionDefined4a -eq "False") "4a: `$version does not exist in the child session after the one-line install via iex"
  Assert-True ($installDirDefined4a -eq "False") "4a: `$installDir does not exist in the child session after the one-line install via iex"
} finally {
  Stop-StaticFileServer $server4a
  Remove-Item Env:\PRIKK_INSTALL_DIR -ErrorAction SilentlyContinue
  Remove-Item Env:\PRIKK_INSTALL_BASE_URL -ErrorAction SilentlyContinue
  Remove-UserPathEntryIfPresent $installDir4a
}

# --- Control 4b: the corrupted-zip case, in a child session -- the session must survive ---------
# Reuses Control 1's own corrupted-zip fixture directory ($control1Dir) on a fresh port (the
# server that originally served it was already stopped in Control 1's own `finally` block).
# The child script wraps Invoke-Expression in its own try/catch: `throw` (the fix) is a normal
# .NET exception and is caught by it, after which the child process keeps running and prints the
# sentinel below; the old `exit 1` behavior is not a catchable error at all -- about_Try_Catch_
# Finally states plainly "A finally block also runs if an exit keyword stops the script from
# within a catch block", meaning exit stops the script (here, the whole child process, since nothing
# wraps the top level of a `-File` script) even when already inside a catch, i.e. it is not
# something a catch block intercepts -- so if install.ps1 still called `exit`, the child process
# would terminate at that statement and the sentinel line below would never print. This is what
# makes this control genuinely able to fail against the old behavior, not merely "the script no
# longer contains the word exit".
$installDir4b = Join-Path $scratchRoot "install-control4b"
$env:PRIKK_INSTALL_DIR = $installDir4b
$env:PRIKK_INSTALL_BASE_URL = "http://localhost:18083"
$server4b = Start-StaticFileServer -Directory $control1Dir -Port 18083
try {
  $child4bTemplate = @'
try {
  $scriptText = Get-Content -Path '__INSTALL_SCRIPT_PATH__' -Raw
  Invoke-Expression $scriptText
} catch {
  Write-Output "PRIKK_CAUGHT=true"
}
Write-Output "PRIKK_SENTINEL=reached-after-iex"
'@
  $child4bContent = $child4bTemplate.Replace('__INSTALL_SCRIPT_PATH__', $installScriptPath)
  $child4bPath = Join-Path $childScriptsDir "child-4b.ps1"
  Set-Content -Path $child4bPath -Value $child4bContent

  $output4b = & $enginePath -NoProfile -File $child4bPath
  Assert-True (-not (Test-Path -Path (Join-Path $installDir4b "prikk.exe"))) "4b: the one-line install via iex against the corrupted zip installs nothing"

  $caught4b = $output4b -contains "PRIKK_CAUGHT=true"
  Assert-True $caught4b "4b: the failed install's own throw was caught by the child session's try/catch (a catchable error, not a process-killing exit)"

  $sentinel4b = $output4b -contains "PRIKK_SENTINEL=reached-after-iex"
  Assert-True $sentinel4b "GREEN: the child session prints a sentinel after the failed one-line install -- the session survived; the old 'exit 1' behavior would have ended the whole child process before this line, uncatchable by the surrounding try/catch"
} finally {
  Stop-StaticFileServer $server4b
  Remove-Item Env:\PRIKK_INSTALL_DIR -ErrorAction SilentlyContinue
  Remove-Item Env:\PRIKK_INSTALL_BASE_URL -ErrorAction SilentlyContinue
  Remove-UserPathEntryIfPresent $installDir4b
}

# ---------------------------------------------------------------------------
# Control 5 (C2): writing the user PATH must not rewrite entries the installer did not add, and
# must not silently convert the Path value's own registry kind from REG_EXPAND_SZ to REG_SZ.
# Shown red first: install.ps1's own raw-registry calls are perturbed back to the old
# [Environment]::Get/SetEnvironmentVariable pattern (the exact two calls the review identifies as
# the cause) and must be seen to expand/downgrade a seeded, unexpanded %USERPROFILE%\prikk-keep
# entry -- proving the control can fail -- before the real, unperturbed install.ps1 and
# uninstall.ps1 are run against the same setup and must leave it untouched end to end.
#
# The CI runner's own real HKCU\Environment\Path value and kind are saved first and restored in
# `finally` regardless of pass/fail. This control replaces that value with a small, deterministic
# one of its own for the scenario's own duration, rather than appending to whatever is already
# there: a stray leading/trailing/doubled ';' already present in the real runner's own Path
# (something this test does not control and cannot inspect in advance) could otherwise make even a
# *correct* split/filter/rejoin round-trip fail a literal byte-for-byte comparison for reasons
# having nothing to do with this control's own subject.
# ---------------------------------------------------------------------------
Write-Section "Control 5: raw PATH registry preservation, unexpanded ExpandString entries (C2, red before green)"

$originalRawPath = Get-RawUserPathForTest
$keepEntry = '%USERPROFILE%\prikk-keep'
$installDir5 = Join-Path $scratchRoot "install-control5"

try {
  $seededValue = $keepEntry
  Set-RawUserPathForTest -Value $seededValue -Kind ([Microsoft.Win32.RegistryValueKind]::ExpandString)
  $afterSeedRaw = Get-RawUserPathForTest
  Assert-True ($afterSeedRaw.Value -ceq $seededValue) "setup: the seeded Path value reads back unexpanded, as written"
  Assert-True ($afterSeedRaw.Kind -eq [Microsoft.Win32.RegistryValueKind]::ExpandString) "setup: the seeded Path value's own registry kind is ExpandString"

  # --- RED: install.ps1, perturbed back to the old API calls ------------------------------------
  $rawReadMarker = '$rawPath = Get-RawUserPathEntry'
  if (-not $installScriptContent.Contains($rawReadMarker)) {
    throw "control 5's read perturbation text was not found in install.ps1 -- the template changed shape; update this script"
  }
  $installWriteMarker = 'Set-RawUserPathEntry -Value $newPath -Kind $rawPath.Kind'
  if (-not $installScriptContent.Contains($installWriteMarker)) {
    throw "control 5's install write perturbation text was not found in install.ps1 -- the template changed shape; update this script"
  }
  $rawReadPerturbed = '$rawPath = @{ Value = [Environment]::GetEnvironmentVariable(''Path'', ''User''); Kind = [Microsoft.Win32.RegistryValueKind]::String }'
  $installWritePerturbed = '[Environment]::SetEnvironmentVariable(''Path'', $newPath, ''User'')'
  $perturbedInstall5Content = $installScriptContent.Replace($rawReadMarker, $rawReadPerturbed).Replace($installWriteMarker, $installWritePerturbed)
  $perturbedInstall5Path = Join-Path $scratchRoot "install-perturbed-control5.ps1"
  Set-Content -Path $perturbedInstall5Path -Value $perturbedInstall5Content

  $env:PRIKK_INSTALL_DIR = $installDir5
  $env:PRIKK_INSTALL_BASE_URL = "http://localhost:18084"
  $server5 = Start-StaticFileServer -Directory $distDirResolved -Port 18084
  try {
    & $enginePath -NoProfile -ExecutionPolicy Bypass -File $perturbedInstall5Path
    $redRaw = Get-RawUserPathForTest
    $redCorrupted = ($redRaw.Value -cne $seededValue) -or ($redRaw.Kind -ne [Microsoft.Win32.RegistryValueKind]::ExpandString)
    Assert-True $redCorrupted "RED: the perturbed install.ps1 (old Get/SetEnvironmentVariable calls) expands the seeded entry and/or downgrades its registry kind -- the control can fail"
  } finally {
    Stop-StaticFileServer $server5
    Remove-Item Env:\PRIKK_INSTALL_BASE_URL -ErrorAction SilentlyContinue
    Remove-Item -Path $installDir5 -Recurse -Force -ErrorAction SilentlyContinue
  }

  # Restore the clean seed before exercising the real scripts.
  Set-RawUserPathForTest -Value $seededValue -Kind ([Microsoft.Win32.RegistryValueKind]::ExpandString)
  $afterRestoreRaw = Get-RawUserPathForTest
  Assert-True ($afterRestoreRaw.Value -ceq $seededValue) "setup: the seeded Path value is restored before the real install.ps1 runs"

  # --- GREEN: the real, unperturbed install.ps1 -------------------------------------------------
  $env:PRIKK_INSTALL_BASE_URL = "http://localhost:18085"
  $server5b = Start-StaticFileServer -Directory $distDirResolved -Port 18085
  try {
    & $enginePath -NoProfile -ExecutionPolicy Bypass -File $installScriptPath
    Assert-True ($LASTEXITCODE -eq 0) "GREEN: the real install.ps1 exits 0 with a seeded ExpandString Path entry present"
    $afterInstallRaw = Get-RawUserPathForTest
    $afterInstallEntries = $afterInstallRaw.Value -split ';' | Where-Object { $_ -ne '' }
    Assert-True ($afterInstallEntries -ccontains $keepEntry) "GREEN: the seeded entry is still present, unexpanded, after the real install.ps1 runs"
    Assert-True ($afterInstallRaw.Kind -eq [Microsoft.Win32.RegistryValueKind]::ExpandString) "GREEN: the Path value's own registry kind is still ExpandString after the real install.ps1 runs"
    Assert-True ($afterInstallEntries -contains $installDir5) "GREEN: the real install.ps1 still added its own entry to the Path"
  } finally {
    Stop-StaticFileServer $server5b
    Remove-Item Env:\PRIKK_INSTALL_BASE_URL -ErrorAction SilentlyContinue
  }

  # --- GREEN: the real, unperturbed uninstall.ps1 -- byte-for-byte round trip -------------------
  & $enginePath -NoProfile -ExecutionPolicy Bypass -File $uninstallScriptPath
  Assert-True ($LASTEXITCODE -eq 0) "GREEN: the real uninstall.ps1 exits 0"
  $afterUninstallRaw = Get-RawUserPathForTest
  Assert-True ($afterUninstallRaw.Value -ceq $seededValue) "GREEN: the raw Path value after uninstall.ps1 is byte-for-byte identical to the seeded value (only this installer's own entry was removed; the seeded entry was completely untouched)"
  Assert-True ($afterUninstallRaw.Kind -eq [Microsoft.Win32.RegistryValueKind]::ExpandString) "GREEN: the Path value's own registry kind is still ExpandString after the real uninstall.ps1 runs"
} finally {
  Remove-Item Env:\PRIKK_INSTALL_DIR -ErrorAction SilentlyContinue
  Remove-Item Env:\PRIKK_INSTALL_BASE_URL -ErrorAction SilentlyContinue
  Remove-Item -Path $installDir5 -Recurse -Force -ErrorAction SilentlyContinue
  # Restore the CI runner's own real Path value and kind regardless of pass/fail.
  if ($originalRawPath.Existed) {
    Set-RawUserPathForTest -Value $originalRawPath.Value -Kind $originalRawPath.Kind
  } else {
    $envKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
    if ($null -ne $envKey) {
      try { $envKey.DeleteValue('Path', $false) } finally { $envKey.Close() }
    }
  }
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
