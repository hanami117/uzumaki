# Installs, checks and uninstalls Uzumaki with the installer build.ps1 made. For a CI runner or a throwaway
# Windows account: it installs for the current user and writes (then removes) marker files in the real
# %APPDATA%\Uzumaki and %LOCALAPPDATA%\Uzumaki, to prove that an uninstall keeps the user's data.
# Never run it on a PC where Uzumaki is in use.
#
#   pwsh packaging\windows\smoke.ps1 -Setup target\windows-release\Uzumaki-0.2.3-windows-x64-setup.exe -Version 0.2.3
param(
    [Parameter(Mandatory = $true)][string]$Setup,
    [Parameter(Mandatory = $true)][string]$Version
)
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$setupPath = (Resolve-Path $Setup).Path
$dir = Join-Path ([System.IO.Path]::GetTempPath()) "Uzumaki Smoke $PID"   # a space in the path, on purpose
$log = Join-Path ([System.IO.Path]::GetTempPath()) "uzumaki-smoke-$PID"
New-Item -ItemType Directory -Force $log | Out-Null
$startMenu = Join-Path ([Environment]::GetFolderPath("Programs")) "Uzumaki.lnk"
$desktop = Join-Path ([Environment]::GetFolderPath("DesktopDirectory")) "Uzumaki.lnk"
$data = @((Join-Path $env:APPDATA "Uzumaki"), (Join-Path $env:LOCALAPPDATA "Uzumaki"))
$ok = $false
$uninstallKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{8669C4B9-FEBE-4B60-A8B1-13C227E304C6}_is1"

function Check([bool]$ok, [string]$what) {
    if ($ok) { Write-Host "ok    $what" } else { throw "FAILED: $what" }
}
function UserPath { [string](Get-ItemProperty -Path "HKCU:\Environment" -Name Path -ErrorAction SilentlyContinue | ForEach-Object { $_.Path }) }
function OnPath { (UserPath) -split ';' | Where-Object { $_ -ieq $dir } | Measure-Object | ForEach-Object { $_.Count -gt 0 } }
function Run([string]$exe, [string[]]$arguments, [string]$what) {
    $p = Start-Process -FilePath $exe -ArgumentList $arguments -Wait -PassThru
    Check ($p.ExitCode -eq 0) "$what (exit code $($p.ExitCode))"
}
function Install([string]$tasks, [string]$name) {
    Run $setupPath @("/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/DIR=`"$dir`"", "/TASKS=`"$tasks`"", "/LOG=`"$log\$name.log`"") "install ($name)"
}
function Uninstall([string[]]$extra, [string]$name) {
    $un = Join-Path $dir "unins000.exe"
    Check (Test-Path $un) "the uninstaller is there"
    Run $un (@("/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/LOG=`"$log\$name.log`"") + $extra) "uninstall ($name)"
    # The uninstaller finishes from a copy of itself: give it a moment to delete the folder.
    foreach ($i in 1..60) { if (-not (Test-Path $dir)) { break }; Start-Sleep -Milliseconds 500 }
}

try {
    Check (-not (Test-Path $uninstallKey)) "Uzumaki is not installed yet on this machine"
    $pathBefore = UserPath

    # --- 1. a plain install: no admin, the PATH task on (the default), no desktop shortcut ------------
    Install "addtopath" "install-1"
    foreach ($f in "uzumaki.exe", "uzu.exe", "unins000.exe", "README-WINDOWS.txt", "LICENSE.txt", "THIRD_PARTY_NOTICES.txt",
                   "bridge\main.js", "bridge\preload.js", "bridge\signin.js", "bridge\package.json", "bridge\uzumaki.ico",
                   "bridge\node_modules\electron\dist\electron.exe", "bridge\node_modules\electron\dist\electron.exe.sig",
                   "bridge\node_modules\electron\dist\resources.pak", "bridge\node_modules\electron\dist\locales\en-US.pak",
                   "bridge\node_modules\electron\dist\LICENSES.chromium.html") {
        Check (Test-Path (Join-Path $dir $f)) "$f is installed"
    }
    $said = (& (Join-Path $dir "uzu.exe") --version | Out-String).Trim()
    Write-Host "      $said"
    Check ($said -match [regex]::Escape($Version)) "uzu --version says $Version"
    $said = (& (Join-Path $dir "uzumaki.exe") --version | Out-String).Trim()
    Check ($said -match [regex]::Escape($Version)) "uzumaki --version says $Version ($said)"
    Check (Test-Path $startMenu) "the Start menu shortcut is there"
    Check (-not (Test-Path $desktop)) "no desktop shortcut unless asked"
    Check (OnPath) "the folder is on the user's PATH"
    $un = Get-ItemProperty $uninstallKey
    Check ($un.DisplayName -eq "Uzumaki") "listed in Installed apps as '$($un.DisplayName)'"
    Check ($un.DisplayVersion -eq $Version) "listed with version $($un.DisplayVersion)"
    Check (-not (Test-Path "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{8669C4B9-FEBE-4B60-A8B1-13C227E304C6}_is1")) "nothing was written for all users"
    # uzu from the PATH, as a new terminal would find it.
    $env:Path = "$env:Path;$dir"
    Check ((& uzu --version | Out-String) -match [regex]::Escape($Version)) "uzu runs from the PATH"
    # With no app running `uzu quit` must say so and start nothing (the installer itself relies on it).
    $said = & (Join-Path $dir "uzu.exe") quit 2>&1 | Out-String
    Check ($LASTEXITCODE -eq 0 -and $said -match "isn't running") "uzu quit without the app: $($said.Trim())"

    # The user's data and what the app writes for the system (stand-ins: the app itself never ran here).
    foreach ($d in $data) { New-Item -ItemType Directory -Force $d | Out-Null; Set-Content (Join-Path $d "smoke-marker.txt") "keep me" }
    $run = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
    New-Item -Path $run -Force | Out-Null
    Set-ItemProperty -Path $run -Name "Uzumaki" -Value "`"$dir\uzumaki.exe`" --hidden"
    $appId = "HKCU:\Software\Classes\AppUserModelId\io.github.hanami117.uzumaki"
    New-Item -Path $appId -Force | Out-Null
    Set-ItemProperty -Path $appId -Name "DisplayName" -Value "Uzumaki"

    # --- 2. an update over it: same folder without /DIR, the desktop shortcut this time, PATH once ----
    # A file of an older engine must not survive the update.
    Set-Content (Join-Path $dir "bridge\node_modules\electron\dist\old-engine-file.txt") "stale"
    Run $setupPath @("/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/TASKS=`"addtopath,desktopicon`"", "/LOG=`"$log\install-2.log`"") "install again (an update)"
    Check (Test-Path (Join-Path $dir "uzumaki.exe")) "the update went to the same folder"
    Check (Test-Path (Join-Path $dir "bridge\node_modules\electron\dist\electron.exe")) "the engine is there after the update"
    Check (-not (Test-Path (Join-Path $dir "bridge\node_modules\electron\dist\old-engine-file.txt"))) "the update replaced the engine whole"
    Check (Test-Path $desktop) "the desktop shortcut is there when asked"
    Check (((UserPath) -split ';' | Where-Object { $_ -ieq $dir } | Measure-Object).Count -eq 1) "the folder is on the PATH once"

    # --- 3. uninstall: the programs go, the user's data stays ----------------------------------------
    Uninstall @() "uninstall-1"
    Check (-not (Test-Path $dir)) "the install folder is gone"
    Check (-not (Test-Path $startMenu)) "the Start menu shortcut is gone"
    Check (-not (Test-Path $desktop)) "the desktop shortcut is gone"
    Check (-not (OnPath)) "the PATH entry is gone"
    $items = { param($p) @($p -split ';' | Where-Object { $_ -ne '' }) -join ';' }
    Check ((& $items (UserPath)) -eq (& $items $pathBefore)) "the user's PATH has what it had before"
    Check (-not (Test-Path $uninstallKey)) "no longer listed in Installed apps"
    Check ($null -eq (Get-ItemProperty -Path $run -Name "Uzumaki" -ErrorAction SilentlyContinue)) "the start-at-login entry is gone"
    Check (-not (Test-Path $appId)) "the app id's registration is gone"
    foreach ($d in $data) { Check (Test-Path (Join-Path $d "smoke-marker.txt")) "kept: $d" }

    # --- 4. install without the PATH task, uninstall with /REMOVEDATA: the data goes too --------------
    Install "" "install-3"
    Check (-not (OnPath)) "no PATH entry when the task is off"
    Uninstall @("/REMOVEDATA") "uninstall-2"
    Check (-not (Test-Path $dir)) "the install folder is gone again"
    foreach ($d in $data) { Check (-not (Test-Path $d)) "deleted when asked: $d" }
    Write-Host "Install and uninstall: all checks passed."
    $ok = $true
}
finally {
    Write-Host "--- installer logs: $log"
    Get-ChildItem $log -ErrorAction SilentlyContinue | ForEach-Object { Write-Host "--- $($_.Name) (last lines)"; Get-Content $_.FullName -Tail 15 }
    foreach ($d in $data) { Remove-Item (Join-Path $d "smoke-marker.txt") -ErrorAction SilentlyContinue }
}
if ($ok) { exit 0 } else { exit 1 }
