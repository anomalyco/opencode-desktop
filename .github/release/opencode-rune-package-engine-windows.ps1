# opencode-rune-package-engine-windows.ps1: release-boundary unpack and deterministic archive
# helper for the Windows browser engine (runs inside sign-engine-windows in the release
# environment, where only /.github/release/ is checked out).
#
#   -Mode Unpack  -Arch x64|arm64 -WorkDir <dir> -InputTar <unsigned.tar>
#       Validates tar entries before extraction (no traversal, no absolute paths, no links,
#       rooted strictly under "OpenCode Browser Engine/"), extracts into <WorkDir>\windows-<arch>,
#       refuses any reparse points, and enforces the exact 5-file PE allowlist before signing.
#
#   -Mode Archive -Arch x64|arm64 -WorkDir <dir> -OutDir <dir> -Version <X.Y.Z.R>
#       Re-checks the exact 5-file PE allowlist and reparse points on the signed folder, then
#       writes <OutDir>\OpenCode-Browser-Engine-<Version>-windows-<arch>.tar.zst (ustar, zstd -19,
#       biggest-first order matching apps/browser-engine/scripts/package-windows.ps1).
[CmdletBinding()]
param(
  [Parameter(Mandatory)][ValidateSet('Unpack', 'Archive')][string]$Mode,
  [Parameter(Mandatory)][ValidateSet('x64', 'arm64')][string]$Arch,
  [Parameter(Mandatory)][string]$WorkDir,
  [string]$InputTar = '',
  [string]$OutDir = '',
  [string]$Version = ''
)
$ErrorActionPreference = 'Stop'

$Bundle = 'OpenCode Browser Engine'
$ExpectedPE = @(
  'chrome_elf.dll',
  'dxcompiler.dll',
  'libcef.dll',
  'opencode-browser-engine.dll',
  'opencode-browser-engine.exe'
)
$AllowedNonPE = @(
  'CREDITS.html',
  'LICENSE.txt',
  'chrome_100_percent.pak',
  'chrome_200_percent.pak',
  'icudtl.dat',
  'resources.pak',
  'v8_context_snapshot.bin'
)
$ArchiveOrder = @(
  'libcef.dll',
  'dxcompiler.dll',
  'resources.pak',
  'icudtl.dat',
  'chrome_elf.dll',
  'chrome_200_percent.pak',
  'v8_context_snapshot.bin',
  'chrome_100_percent.pak',
  'CREDITS.html',
  'LICENSE.txt',
  'opencode-browser-engine.exe',
  'opencode-browser-engine.dll'
)

function Assert-CleanBundle([string]$StageRoot) {
  $top = @(Get-ChildItem -LiteralPath $StageRoot -Force)
  if ($top.Count -ne 1 -or $top[0].Name -ne $Bundle -or -not $top[0].PSIsContainer) {
    throw "Expected a single top-level directory '$Bundle' in $StageRoot."
  }
  if (($top[0].Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw "Bundle root '$($top[0].FullName)' is a reparse point."
  }
  $app = $top[0].FullName
  $all = @(Get-ChildItem -LiteralPath $app -Recurse -Force)
  foreach ($item in $all) {
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
      throw "Unexpected reparse point in bundle: $($item.FullName)"
    }
  }
  $files = @($all | Where-Object { -not $_.PSIsContainer })
  if ($files.Count -eq 0) { throw "Bundle $app contains no files." }

  $peFiles = @()
  foreach ($f in $files) {
    $rel = $f.FullName.Substring($app.Length + 1).Replace('\', '/')
    if ($rel -match '\.(exe|dll)$') {
      $peFiles += $rel
    } elseif ($AllowedNonPE -contains $rel) {
      continue
    } elseif ($rel -match '^locales/[A-Za-z0-9_-]+\.pak$') {
      continue
    } else {
      throw "Unexpected file in Windows engine bundle: $rel"
    }
  }
  $sortedPE = @($peFiles | Sort-Object)
  $expectedSorted = @($ExpectedPE | Sort-Object)
  if (($sortedPE -join "`n") -ne ($expectedSorted -join "`n")) {
    throw "PE allowlist mismatch in ${app}. Expected: [$($expectedSorted -join ', ')]. Found: [$($sortedPE -join ', ')]."
  }
  $exePath = Join-Path $app 'opencode-browser-engine.exe'
  $peBytes = [IO.File]::ReadAllBytes($exePath)
  $peOff = [BitConverter]::ToInt32($peBytes, 0x3C)
  $optMagic = [BitConverter]::ToUInt16($peBytes, $peOff + 24)
  if ($optMagic -ne 0x20B) {
    throw "Unexpected PE optional header magic 0x$($optMagic.ToString('x')) in $exePath."
  }
  $stackReserve = [BitConverter]::ToUInt64($peBytes, $peOff + 24 + 72)
  if ($stackReserve -lt 8388608) {
    throw "$exePath has SizeOfStackReserve=$stackReserve (< 8 MiB required for MV3 workers, #582)."
  }
  return $app
}

$tarExe = Join-Path $env:SystemRoot 'System32\tar.exe'
if (-not (Test-Path -LiteralPath $tarExe)) { throw "Missing Windows tar.exe at $tarExe." }

$stage = Join-Path $WorkDir "windows-$Arch"

if ($Mode -eq 'Unpack') {
  if (-not $InputTar -or -not (Test-Path -LiteralPath $InputTar)) {
    throw "Missing -InputTar '$InputTar'."
  }
  $entries = @(& $tarExe -tf $InputTar)
  if ($LASTEXITCODE -ne 0 -or $entries.Count -eq 0) {
    throw "Could not list tar entries in $InputTar."
  }
  $verbose = @(& $tarExe -tvf $InputTar)
  if ($LASTEXITCODE -ne 0 -or $verbose.Count -ne $entries.Count) {
    throw "Could not inspect tar metadata in $InputTar."
  }
  for ($i = 0; $i -lt $entries.Count; $i++) {
    $raw = $entries[$i]
    $modeChar = if ($verbose[$i].Length -gt 0) { $verbose[$i].Substring(0, 1) } else { '' }
    if ($modeChar -ne '-' -and $modeChar -ne 'd') {
      throw "Refusing non-regular tar entry ($modeChar): $raw"
    }
    $norm = $raw.TrimEnd('/')
    if ($norm -match '^[\\/]' -or $norm -match ':' -or $norm.Contains('\') -or ($norm.Split('/') -contains '..') -or ($norm.Split('/') -contains '.')) {
      throw "Refusing unsafe tar path: $raw"
    }
    if ($norm -ne $Bundle -and -not $norm.StartsWith("$Bundle/")) {
      throw "Refusing tar path outside '$Bundle': $raw"
    }
  }

  if (Test-Path -LiteralPath $stage) { Remove-Item -Recurse -Force -LiteralPath $stage }
  New-Item -ItemType Directory -Force -Path $stage | Out-Null
  & $tarExe -xf $InputTar -C $stage
  if ($LASTEXITCODE -ne 0) { throw "Failed to unpack $InputTar." }
  $app = Assert-CleanBundle $stage
  $localeCount = @(Get-ChildItem -LiteralPath (Join-Path $app 'locales') -Filter '*.pak').Count
  Write-Output "ok - unpacked windows-$Arch ($($ExpectedPE.Count) PE files verified, $localeCount locales)"
  return
}

if ($Version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$') {
  throw "Invalid -Version '$Version' (expected X.Y.Z.R)."
}
if (-not $OutDir) { throw 'Missing -OutDir.' }
$app = Assert-CleanBundle $stage
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$locales = @(Get-ChildItem -LiteralPath (Join-Path $app 'locales') -Filter '*.pak' | Sort-Object Name)
$archiveEntries = @(
  $ArchiveOrder | Where-Object { Test-Path -LiteralPath (Join-Path $app $_) }
) + @(
  $locales | ForEach-Object { "locales/$($_.Name)" }
) | ForEach-Object { "$Bundle/$_" }

$name = "OpenCode-Browser-Engine-$Version-windows-$Arch.tar.zst"
$zst = Join-Path $OutDir $name
if (Test-Path -LiteralPath $zst) { Remove-Item -Force -LiteralPath $zst }
& $tarExe --format ustar --zstd --options 'zstd:compression-level=19' -cf $zst -C $stage @archiveEntries
if ($LASTEXITCODE -ne 0) { throw "Failed to archive $name." }
$size = (Get-Item -LiteralPath $zst).Length
$sha = (Get-FileHash -Algorithm SHA256 -LiteralPath $zst).Hash.ToLower()
Write-Output "ok - archived $name ($size bytes, sha256 $sha)"
