# Runs the staged Uzumaki for real on this machine, hidden, as a test copy: scratch folders, a scratch
# sign-in profile, muted, its own control pipe. Signed out, so it plays nothing; it checks that the
# program starts with no window, answers `uzu` over the named pipe, starts the playback engine
# (castLabs Electron) and reaches Apple's web player, then quits and takes the engine with it.
# For a CI runner: never run it where Uzumaki is in use (it is safe there too, but pointless).
#
#   pwsh packaging\windows\app-smoke.ps1 -Dir target\windows-dist\Uzumaki
param(
    [Parameter(Mandatory = $true)][string]$Dir
)
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$dir = (Resolve-Path $Dir).Path
$scratch = Join-Path ([System.IO.Path]::GetTempPath()) "uzumaki-app-smoke-$PID"
New-Item -ItemType Directory -Force $scratch | Out-Null
$env:UZUMAKI_TEST_INSTANCE = "1"
$env:UZUMAKI_MUTE = "1"
$env:UZUMAKI_PROFILE_DIR = Join-Path $scratch "profile"
$env:UZUMAKI_CTL_SOCKET = "\\.\pipe\uzumaki-app-smoke-$PID"
$env:XDG_CONFIG_HOME = Join-Path $scratch "config"
$env:XDG_CACHE_HOME = Join-Path $scratch "cache"
$env:XDG_STATE_HOME = Join-Path $scratch "state"
$env:XDG_DATA_HOME = Join-Path $scratch "data"
$uzu = Join-Path $dir "uzu.exe"
$ok = $false

function Check([bool]$ok, [string]$what) {
    if ($ok) { Write-Host "ok    $what" } else { throw "FAILED: $what" }
}
function Engines { @(Get-CimInstance Win32_Process -Filter "Name = 'electron.exe'" | Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith($dir, [System.StringComparison]::OrdinalIgnoreCase) }) }

$app = $null
try {
    $app = Start-Process -FilePath (Join-Path $dir "uzumaki.exe") -ArgumentList "--hidden" -PassThru `
        -RedirectStandardOutput (Join-Path $scratch "out.txt") -RedirectStandardError (Join-Path $scratch "err.txt")
    # The pipe appears (looked up by name: nothing connects to it, and `uzu` would start a second copy
    # if it found none), then the engine connects (the first start also downloads the Widevine component).
    $pipe = "uzumaki-app-smoke-$PID"
    $there = $false
    foreach ($i in 1..60) {
        if ($app.HasExited) { throw "uzumaki.exe ended by itself (exit code $($app.ExitCode))" }
        if ([System.IO.Directory]::GetFiles("\\.\pipe\") | Where-Object { $_ -like "*\$pipe" }) { $there = $true; break }
        Start-Sleep -Milliseconds 500
    }
    Check $there "the app started with no window and listens on its named pipe"
    $said = ""
    foreach ($i in 1..90) {
        if ($app.HasExited) { throw "uzumaki.exe ended by itself (exit code $($app.ExitCode))" }
        $said = & $uzu status 2>&1 | Out-String
        if ($LASTEXITCODE -eq 0 -and $said -notmatch "connecting to Apple Music") { break }
        Start-Sleep -Seconds 2
    }
    Write-Host ($said.TrimEnd() -split "`n" | ForEach-Object { "      $_" } | Out-String).TrimEnd()
    Check ($LASTEXITCODE -eq 0) "uzu reaches the running app over the named pipe"
    Check ($said -notmatch "connecting to Apple Music") "the playback engine started and found Apple's web player"
    Check ($said -match "not signed in") "signed out, as a scratch profile is"
    Check ((Engines).Count -gt 0) "castLabs Electron runs from the staged folder ($((Engines).Count) processes)"
    # Chromium fills its profile folder as it goes (some files only when it quits): anything in it will do.
    Check (@(Get-ChildItem $env:UZUMAKI_PROFILE_DIR -Force -ErrorAction SilentlyContinue).Count -gt 0) "the engine's profile is the scratch one"
    Check (Test-Path (Join-Path $scratch "state\uzumaki\Logs\bridge.log")) "the app keeps its files in the scratch folders (the engine's log)"
    # A second copy must not start: this one is a test copy (no single-instance check), so only `uzu` is tried.
    $said = & $uzu search "bach cello suite" 2>&1 | Out-String
    Write-Host ($said.Trim() -split "`n" | Select-Object -First 4 | ForEach-Object { "      $_" } | Out-String).TrimEnd()
    Check ($LASTEXITCODE -eq 0 -and $said -match "\S") "a catalog search comes back through the app"
    $said = & $uzu quit 2>&1 | Out-String
    Check ($LASTEXITCODE -eq 0 -and $said -match "quit") "uzu quit: $($said.Trim())"
    foreach ($i in 1..40) { if ($app.HasExited) { break }; Start-Sleep -Milliseconds 500 }
    Check ($app.HasExited) "the app ended"
    foreach ($i in 1..40) { if ((Engines).Count -eq 0) { break }; Start-Sleep -Milliseconds 500 }
    Check ((Engines).Count -eq 0) "the playback engine ended with it"
    Write-Host "The app on this machine: all checks passed."
    $ok = $true
}
finally {
    if ($app -and -not $app.HasExited) { Stop-Process -Id $app.Id -Force -ErrorAction SilentlyContinue }
    Engines | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    foreach ($f in "err.txt", "out.txt") {
        $p = Join-Path $scratch $f
        if (Test-Path $p) { Write-Host "--- $f (last lines)"; Get-Content $p -Tail 30 }
    }
    $bridgeLog = Join-Path $scratch "state\uzumaki\Logs\bridge.log"
    if (Test-Path $bridgeLog) { Write-Host "--- bridge.log (last lines)"; Get-Content $bridgeLog -Tail 30 }
    Start-Sleep -Seconds 1
    Remove-Item -Recurse -Force $scratch -ErrorAction SilentlyContinue
}
if ($ok) { exit 0 } else { exit 1 }
