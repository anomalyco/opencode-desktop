# Windows: builds the engine and installs `OpenCode Browser Engine` where the app looks for it,
# `%LOCALAPPDATA%\co.anomaly.opencode\browser-engine\<version>\` (or a directory passed as -Dest).
#   pwsh apps/browser-engine/scripts/install-local.ps1 [-Release] [-Dest <dir>]
param([switch]$Release, [string]$Dest)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'assemble-windows.ps1')

if (-not $Dest) { $Dest = Join-Path $env:LOCALAPPDATA "co.anomaly.opencode\browser-engine\$EngineVersion" }
$bin = Build-Engine -Release:$Release
$stage = "$Dest.staging.$PID"
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
Assemble-Engine $bin $stage | Out-Null
if (Test-Path $Dest) { Remove-Item -Recurse -Force $Dest }
New-Item -ItemType Directory -Force (Split-Path $Dest) | Out-Null
Move-Item $stage $Dest
Write-Host "Installed: $(Join-Path $Dest 'OpenCode Browser Engine')"
