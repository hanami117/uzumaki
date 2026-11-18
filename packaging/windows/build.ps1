# Packs Uzumaki for Windows from an existing release build and a staged bridge: the installer, the
# zip and their checksums.
#
#   cargo build --release --locked -p uzumaki -p uzumaki-ctl
#   (bridge: npm ci, node node_modules/electron/install.js, the VMP signature: see vmp\README.md)
#   pwsh packaging\windows\build.ps1 -Version 0.2.3 -Build 1
#
# Out (in -OutDir, default target\windows-release), <release> = <version>, or <version>-<build> from build 2 on:
#   Uzumaki-<release>-windows-x64-setup.exe (+ .sha256)   the installer (packaging\windows\uzumaki.iss)
#   Uzumaki-<release>-windows-x64.zip (+ .sha256)         the same files without an installer
# and -StageDir (default target\windows-dist\Uzumaki): the files that go into both:
#   uzumaki.exe  uzu.exe  README-WINDOWS.txt  LICENSE.txt  THIRD_PARTY_NOTICES.txt
#   bridge\  main.js preload.js signin.js airplay.js package.json uzumaki.ico
#            node_modules\electron\dist\  (castLabs Electron: electron.exe + electron.exe.sig, unchanged)
#
# Nothing is Authenticode-signed. The hook (SignPath or signtool) is Sign-Files below. Electron's
# VMP signature must already be in place: this script only checks that electron.exe.sig is there
# and never touches electron.exe (any change to it would break the signature).
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [int]$Build = 1,
    [string]$BinDir = "target\release",
    [string]$BridgeDir = "bridge",
    [string]$StageDir = "target\windows-dist\Uzumaki",
    [string]$OutDir = "target\windows-release"
)
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$root = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
Set-Location $root
if ($Version -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$') { throw "bad version: $Version" }
if ($Build -lt 1) { throw "bad build: $Build" }
$release = if ($Build -eq 1) { $Version } else { "$Version-$Build" }
# Four numbers for the installer's own version details: the version without a pre-release tag, then the build.
$fileVersion = "$(($Version -split '-')[0]).$Build"

# SIGNING HOOK. Today: nothing. The programs would be signed here, before they are packed, and the
# installer afterwards (uzumaki.iss says how Inno signs it and its uninstaller).
function Sign-Files([string[]]$Files) {
    foreach ($f in $Files) { Write-Host "not signed: $f" }
}

function Write-Sha256([string]$File) {
    $name = Split-Path $File -Leaf
    $hash = (Get-FileHash -Algorithm SHA256 $File).Hash.ToLowerInvariant()
    # The sha256sum format: hash, two spaces, name, one line feed.
    [System.IO.File]::WriteAllText("$File.sha256", "$hash  $name`n", (New-Object System.Text.ASCIIEncoding))
    Write-Host "$hash  $name"
}

# --- the files -----------------------------------------------------------------------------------
foreach ($d in $StageDir, $OutDir) {
    if (Test-Path $d) { Remove-Item -Recurse -Force $d }
    New-Item -ItemType Directory -Force $d | Out-Null
}
$stage = (Resolve-Path $StageDir).Path
$out = (Resolve-Path $OutDir).Path
Copy-Item (Join-Path $BinDir "uzumaki.exe"), (Join-Path $BinDir "uzu.exe"), "packaging\windows\README-WINDOWS.txt" $stage
Copy-Item "LICENSE" (Join-Path $stage "LICENSE.txt")
Copy-Item "THIRD_PARTY_NOTICES.md" (Join-Path $stage "THIRD_PARTY_NOTICES.txt")
Sign-Files (Join-Path $stage "uzumaki.exe"), (Join-Path $stage "uzu.exe")

# The playback engine: the bridge's scripts and castLabs Electron's `dist` folder, nothing else of
# node_modules (the npm package's own files are only needed to download it).
$bridge = Join-Path $stage "bridge"
New-Item -ItemType Directory -Force $bridge | Out-Null
foreach ($f in "main.js", "preload.js", "signin.js", "airplay.js", "package.json") { Copy-Item (Join-Path $BridgeDir $f) $bridge }
Copy-Item "packaging\windows\uzumaki.ico" $bridge
$distFrom = Join-Path $BridgeDir "node_modules\electron\dist"
$distTo = Join-Path $bridge "node_modules\electron\dist"
if (-not (Test-Path (Join-Path $distFrom "electron.exe"))) { throw "no castLabs Electron at $distFrom (npm ci, then node node_modules/electron/install.js, in $BridgeDir)" }
if (-not (Test-Path (Join-Path $distFrom "electron.exe.sig"))) { throw "electron.exe has no VMP signature (electron.exe.sig): see packaging\windows\vmp\README.md" }
New-Item -ItemType Directory -Force (Split-Path $distTo) | Out-Null
Copy-Item -Recurse $distFrom $distTo
# Chromium's own interface texts: the hidden player shows none but Apple's page. English stays
# (Chromium's fallback); the rest is 40 MB nobody reads.
Get-ChildItem (Join-Path $distTo "locales") -Filter *.pak | Where-Object { $_.Name -ne "en-US.pak" } | Remove-Item -Force
# The copy is byte for byte the signed program.
foreach ($f in "electron.exe", "electron.exe.sig") {
    if ((Get-FileHash (Join-Path $distFrom $f)).Hash -ne (Get-FileHash (Join-Path $distTo $f)).Hash) { throw "$f changed while it was staged" }
}

# The programs carry the version they were built from: never pack a stale build under a new name.
$said = (& (Join-Path $stage "uzu.exe") --version | Out-String).Trim()
if ($said -notmatch [regex]::Escape("$Version (build $Build)")) { throw "uzu.exe says '$said', expected $Version (build $Build)" }

# --- the installer -------------------------------------------------------------------------------
$iscc = (Get-Command iscc.exe -ErrorAction SilentlyContinue | Select-Object -First 1 | ForEach-Object { $_.Source })
if (-not $iscc) {
    $iscc = @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe", "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe") |
        Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $iscc) { throw "Inno Setup 6 (ISCC.exe) not found: install it from https://jrsoftware.org/isdl.php" }
& $iscc /Qp "/DAppVersion=$Version" "/DFileVersion=$fileVersion" "/DRelease=$release" "/DSourceDir=$stage" "/DOutputDir=$out" "packaging\windows\uzumaki.iss"
if ($LASTEXITCODE -ne 0) { throw "iscc failed ($LASTEXITCODE)" }
$setup = Join-Path $out "Uzumaki-$release-windows-x64-setup.exe"
if (-not (Test-Path $setup)) { throw "no installer at $setup" }
Sign-Files $setup

# --- the zip -------------------------------------------------------------------------------------
# One top folder (Uzumaki\) inside, so unpacking anywhere gives a folder of its own.
$zip = Join-Path $out "Uzumaki-$release-windows-x64.zip"
Compress-Archive -Path $stage -DestinationPath $zip -CompressionLevel Optimal

foreach ($f in $setup, $zip) { Write-Sha256 $f }
Get-ChildItem $out | Format-Table Name, Length | Out-Host
