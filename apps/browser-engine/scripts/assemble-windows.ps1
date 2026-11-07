# Builds the engine for Windows and assembles the `OpenCode Browser Engine` folder the app runs:
# both executables next to CEF's runtime (libcef.dll and the other DLLs, .pak and .bin files,
# icudtl.dat, en-US locale). Dot-sourced by install-local.ps1 and package-release.ps1.

$EngineVersion = '156.0.14.1'
$EngineDir = Split-Path -Parent $PSScriptRoot

function Build-Engine([switch]$Release) {
  if (-not $env:CEF_PATH) { $env:CEF_PATH = Join-Path $env:LOCALAPPDATA 'cef' }
  New-Item -ItemType Directory -Force $env:CEF_PATH | Out-Null
  $profileArgs = if ($Release) { @('--release') } else { @() }
  Write-Host "Building opencode-browser-engine $EngineVersion ($(if ($Release) { 'release' } else { 'debug' }))..."
  cargo build @profileArgs --manifest-path (Join-Path $EngineDir 'Cargo.toml') --bins
  if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
  Join-Path $EngineDir "target\$(if ($Release) { 'release' } else { 'debug' })"
}

# The CEF distribution cef-dll-sys unpacked under CEF_PATH (`<version>\cef_windows_x86_64`).
function Find-CefDir {
  $dir = Get-ChildItem $env:CEF_PATH -Directory -Recurse -Depth 1 -Filter 'cef_windows_*' |
    Where-Object { Test-Path (Join-Path $_.FullName 'libcef.dll') } |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
  if (-not $dir) { throw "no CEF distribution under $env:CEF_PATH" }
  $dir.FullName
}

# Assembles `<Parent>\OpenCode Browser Engine` from the binaries in `$BinDir`.
function Assemble-Engine([string]$BinDir, [string]$Parent) {
  $cef = Find-CefDir
  $app = Join-Path $Parent 'OpenCode Browser Engine'
  New-Item -ItemType Directory -Force (Join-Path $app 'locales') | Out-Null
  Copy-Item (Join-Path $BinDir 'opencode-browser-engine.exe'), (Join-Path $BinDir 'opencode-browser-engine-helper.exe') $app
  Get-ChildItem $cef -File | Where-Object { $_.Extension -in '.dll', '.pak', '.bin', '.dat', '.json' -or $_.Name -eq 'CREDITS.html' } |
    Copy-Item -Destination $app
  Copy-Item (Join-Path $cef 'locales\en-US.pak') (Join-Path $app 'locales')
  $app
}
