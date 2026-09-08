$ErrorActionPreference = 'Stop'
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$root = Split-Path -Parent $scriptDir
Set-Location $root

Write-Host 'Building StarSync MinerScan portable release...'
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build --release failed with exit code $LASTEXITCODE" }

$distDir = Join-Path $root 'dist'
if (Test-Path $distDir) {
    Remove-Item $distDir -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $distDir | Out-Null

$appExe = Join-Path $root 'target\release\starsync-minerscan.exe'
$portableExe = Join-Path $distDir 'StarSyncMinerScan_0.6.8.exe'
Copy-Item $appExe $portableExe -Force

$readme = @"
StarSync MinerScan 0.6.8 Portable Final

No installer is required.

Usage:
  1. Extract this archive to any writable directory.
  2. Start StarSyncMinerScan_0.6.8.exe.

On first start MinerScan automatically creates:
  %LOCALAPPDATA%\StarSync MinerScan\settings.json
  %LOCALAPPDATA%\StarSync MinerScan\minerscan-events.log
  %LOCALAPPDATA%\StarSync MinerScan\cache\
  %LOCALAPPDATA%\StarSync MinerScan\session-reports\
  %LOCALAPPDATA%\StarSync MinerScan\database\

SCUnpacked updates:
  Place a scunpacked-data ZIP or extracted dataset in:
  %LOCALAPPDATA%\StarSync MinerScan\database

  Then use Settings > Data Sources > INSTALL/UPDATE.

The executable already contains the embedded baseline and works without an external database package.
"@
$installation = Join-Path $distDir 'INSTALLATION.txt'
Set-Content -Path $installation -Value $readme -Encoding UTF8

$zipPath = Join-Path $distDir 'StarSyncMinerScan_0.6.8_Portable_Final.zip'
Compress-Archive -Path $portableExe, $installation -DestinationPath $zipPath -CompressionLevel Optimal

Get-FileHash $portableExe, $zipPath -Algorithm SHA256 | Format-Table -AutoSize
Write-Host "Portable distribution ready: $distDir"
