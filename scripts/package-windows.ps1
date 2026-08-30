# Package Momo's Music Manager for Windows as a zip + SHA256.
#
# Usage:
#   powershell -File scripts/package-windows.ps1 [-TargetTriple x86_64-pc-windows-msvc]
#
# Defaults to the host triple. Binary must already be built:
#   cargo build --release --target <triple>
#
# Output (in target/<triple>/release/ unless CARGO_TARGET_DIR is set):
#   momos-music-manager-<version>-<os>-<arch>.zip
#   momos-music-manager-<version>-<os>-<arch>.zip.sha256
param(
    [string]$TargetTriple = ""
)

$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

if ($TargetTriple -eq "") {
    $TargetTriple = rustc -vV | Select-String '^host: ' | ForEach-Object { $_.Line -replace '^host: ', '' }
}

$AppName = "momos-music-manager"
$Version = (cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).packages[0].version
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { "target" }
$ReleaseDir = Join-Path (Join-Path $TargetDir $TargetTriple) "release"
$BinPath = Join-Path $ReleaseDir "$AppName.exe"

switch -Regex ($TargetTriple) {
    '^x86_64-pc-windows-msvc$'   { $OsArch = "windows-x64" }
    '^aarch64-pc-windows-msvc$'  { $OsArch = "windows-arm64" }
    default {
        Write-Error "Unsupported Windows target triple: $TargetTriple"
    }
}

$ArtifactBase = "${AppName}-${Version}-${OsArch}"
$StagingDir = Join-Path $TargetDir "pkg-win-staging-${OsArch}"
$Archive = Join-Path $TargetDir "${ArtifactBase}.zip"

Write-Host "=== Packaging $AppName v$Version for $OsArch ($TargetTriple) ==="

if (-not (Test-Path $BinPath)) {
    Write-Error "Binary not found at $BinPath - run 'cargo build --release --target $TargetTriple' first"
}

Write-Host "--- Staging files ---"
if (Test-Path $StagingDir) { Remove-Item -Recurse -Force $StagingDir }
New-Item -ItemType Directory -Path $StagingDir | Out-Null

Copy-Item $BinPath (Join-Path $StagingDir "$AppName.exe")
Copy-Item "README.md" (Join-Path $StagingDir "README.md")
Set-Content -Path (Join-Path $StagingDir "VERSION") -Value $Version
Set-Content -Path (Join-Path $StagingDir "RUN.txt") -Value @"
Momo's Music Manager v$Version ($OsArch)

Run the server (headless):
  $AppName.exe serve --host 0.0.0.0 --port 3000 --no-browser

Then open http://<host>:3000 in a browser.

For background/service operation use NSSM (https://nssm.cc) or Task Scheduler.
The binary is self-contained (SQLite compiled in, TLS via rustls).
"@

Write-Host "--- Creating archive ---"
if (Test-Path $Archive) { Remove-Item -Force $Archive }
Compress-Archive -Path (Join-Path $StagingDir "*") -DestinationPath $Archive

Write-Host "--- Creating checksum ---"
$Hash = (Get-FileHash -Algorithm SHA256 $Archive).Hash.ToLower()
Set-Content -Path "${Archive}.sha256" -Value "${Hash}  ${ArtifactBase}.zip"

Remove-Item -Recurse -Force $StagingDir

Write-Host ""
Write-Host "=== Done ==="
Write-Host "Archive:  $Archive"
Write-Host "Checksum: ${Archive}.sha256"
Get-Item $Archive | Select-Object FullName, Length
