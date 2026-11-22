# Builds the browser engine for Windows and packages it as the app downloads it:
#   <out>\OpenCode-Browser-Engine-<version>-windows-<arch>.tar.zst
#   <out>\index.json   (the release index the app reads; each run adds or replaces its own target)
# The archive holds one folder, `OpenCode Browser Engine`: both executables and the part of CEF's
# runtime a side-panel browser uses.
# Nothing here signs or publishes. With a publisher trust rule, every .exe and .dll in the stage
# folder is signed between a -StageOnly run and an -ArchiveOnly run (notes for the release
# workflow: the browser-pane README).
#
#   pwsh apps/browser-engine/scripts/package-windows.ps1 [-Arch x64|arm64] [-Out <dir>] [-TargetDir <dir>] [-Cargo <cargo or wrapper.ps1>]
#   ... -TargetDir <dir>            cargo's target directory and CEF's unpack folder (default:
#                                   CARGO_TARGET_DIR, else C:\t on GitHub Actions, else apps\browser-engine\target)
#   ... -StageOnly                  stops after the folder (<out>\stage-<arch>), for signing its programs
#   ... -ArchiveOnly <stage folder>  archives a (signed) stage and adds it to <out>\index.json
#   bun apps/browser-engine/scripts/serve.ts [<out>]   # serves it where a development build looks
#
# Needs: Rust's target for -Arch, MSVC's build tools for it, and a Windows tar.exe with zstd
# (Windows 11). CEF is Spotify's prebuilt minimal distribution, fetched once and checked against
# the SHA-256 pinned below; cef-dll-sys in Cargo.lock must name the same CEF build.
param(
  [ValidateSet('x64', 'arm64')][string]$Arch = 'x64',
  [string]$Out,
  [string]$TargetDir,
  [string]$Cargo = 'cargo',
  # Package what cargo last built (a build run elsewhere, or a repackage).
  [switch]$SkipBuild,
  # A release signs every .exe and .dll between the two halves: build and stage, then archive.
  [switch]$StageOnly,
  [string]$ArchiveOnly
)
$ErrorActionPreference = 'Stop'
$Engine = Split-Path -Parent $PSScriptRoot
$Repo = Split-Path -Parent (Split-Path -Parent $Engine)
# CEF's wrapper is compiled by MSVC under cargo's target directory, and its deepest object
# (`<target>\<triple>\release\build\cef-dll-sys-<hash>\out\build\libcef_dll_wrapper\CMakeFiles\
# libcef_dll_wrapper.dir\cpptoc\test\api_version_test_ref_ptr_client_child_v2_cpptoc.cc.obj`) passes
# MAX_PATH (260) under a CI checkout (C1083 "Cannot open compiler generated file"), so CI builds at
# the drive root.
if (-not $TargetDir) {
  $TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } elseif ($env:GITHUB_ACTIONS -eq 'true') { 'C:\t' } else { Join-Path $Engine 'target' }
}
$TargetDir = [IO.Path]::GetFullPath($TargetDir)
if (-not $Out) { $Out = Join-Path $TargetDir 'package' }

$CefVersion = '156.0.26+g8ea0c73+chromium-156.0.8078.88'
$Targets = @{
  x64   = @{ Platform = 'windows64'; Dir = 'cef_windows_x86_64'; Triple = 'x86_64-pc-windows-msvc'; Sha256 = '2b0aa070793a6bb858b737f5edf9cb01a3ef7b9adcad5efe1e08affaf250f809' }
  arm64 = @{ Platform = 'windowsarm64'; Dir = 'cef_windows_aarch64'; Triple = 'aarch64-pc-windows-msvc'; Sha256 = 'de261a5a24862082c1a0bfd7ffb6a0bb2c074bced7598b3bd69e3cdbadbb395d' }
}
$T = $Targets[$Arch]
$CefBuild = $CefVersion.Split('+')[0]

# The engine's version: CEF's, then the engine revision the app accepts (ENGINE_REVISION).
$installer = Get-Content -Raw (Join-Path $Repo 'crates\browser-pane\src\installer.rs')
$Revision = [regex]::Match($installer, 'pub const ENGINE_REVISION: u32 = (\d+);').Groups[1].Value
if (-not $Revision) { throw 'ENGINE_REVISION not found in crates/browser-pane/src/installer.rs' }
$Version = "$CefBuild.$Revision"
$lock = Get-Content -Raw (Join-Path $Engine 'Cargo.lock')
if ($lock -notmatch "name = `"cef-dll-sys`"\r?\nversion = `"[^`"]+\+$([regex]::Escape($CefBuild))`"") {
  throw "Cargo.lock's cef-dll-sys isn't CEF ${CefBuild}: update `$CefVersion and the SHA-256s here with it."
}

if (-not $ArchiveOnly) {
# 1. CEF, laid out as cef-dll-sys expects it under CEF_PATH (<build>\<dir>, with archive.json).
$env:CEF_PATH = Join-Path $TargetDir 'cef'
$cefDir = Join-Path $env:CEF_PATH "$CefBuild\$($T.Dir)"
$archiveName = "cef_binary_${CefVersion}_$($T.Platform)_minimal.tar.bz2"
if (-not (Test-Path (Join-Path $cefDir 'archive.json'))) {
  $download = Join-Path $env:CEF_PATH $archiveName
  if (-not (Test-Path $download)) {
    New-Item -ItemType Directory -Force $env:CEF_PATH | Out-Null
    Write-Host "Downloading $archiveName"
    curl.exe -fL --retry 3 -o "$download.part" ('https://cef-builds.spotifycdn.com/' + [uri]::EscapeDataString($archiveName))
    if ($LASTEXITCODE -ne 0) { throw "couldn't download $archiveName" }
    Move-Item -Force "$download.part" $download
  }
  $sha = (Get-FileHash -Algorithm SHA256 $download).Hash.ToLower()
  if ($sha -ne $T.Sha256) { Remove-Item $download; throw "$archiveName has SHA-256 ${sha}, not the pinned $($T.Sha256)" }
  $unpack = Join-Path $env:CEF_PATH "unpack-$PID"
  New-Item -ItemType Directory -Force $unpack | Out-Null
  & "$env:SystemRoot\System32\tar.exe" -xf $download -C $unpack
  if ($LASTEXITCODE -ne 0) { throw "couldn't unpack $archiveName" }
  $root = Join-Path $unpack ($archiveName -replace '\.tar\.bz2$', '')
  Remove-Item -Recurse -Force $cefDir -ErrorAction SilentlyContinue
  New-Item -ItemType Directory -Force (Split-Path $cefDir) | Out-Null
  Move-Item (Join-Path $root 'Release') $cefDir
  Get-ChildItem (Join-Path $root 'Resources') | Move-Item -Destination $cefDir
  foreach ($name in 'CMakeLists.txt', 'cmake', 'include', 'libcef_dll', 'CREDITS.html', 'LICENSE.txt') { Move-Item (Join-Path $root $name) $cefDir }
  $sha1 = (Get-FileHash -Algorithm SHA1 $download).Hash.ToLower()
  @{ type = 'minimal'; name = $archiveName; sha1 = $sha1 } | ConvertTo-Json | Set-Content -Encoding utf8NoBOM (Join-Path $cefDir 'archive.json')
  Remove-Item -Recurse -Force $unpack
}

# 2. The engine (client DLL exporting `RunWinMain`, launched by CEF's sandboxed `bootstrap.exe`).
if (-not $SkipBuild) {
  Write-Host "Building opencode-browser-engine $Version ($($T.Triple))"
  $cargoArgs = @('build', '--release', '--locked', '--manifest-path', (Join-Path $Engine 'Cargo.toml'), '--lib', '--target', $T.Triple, '--target-dir', $TargetDir)
  # The C runtime linked in, as the app's, so the engine's DLL needs no Visual C++
  # Redistributable. RUSTFLAGS replaces the repository's .cargo/config.toml flags, whose
  # profile-guided ones are the app's.
  $rustflags = $env:RUSTFLAGS
  $env:RUSTFLAGS = '-C target-feature=+crt-static -C link-arg=/NODEFAULTLIB:libucrt.lib -C link-arg=/DEFAULTLIB:ucrt.lib'
  try { if ($Cargo -like '*.ps1') { & pwsh -NoProfile $Cargo @cargoArgs } else { & $Cargo @cargoArgs } } finally { $env:RUSTFLAGS = $rustflags }
  if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
}
$bin = Join-Path $TargetDir "$($T.Triple)\release"

# 3. The folder: what a side-panel browser loads, biggest first so the app can check it while the
#    rest arrives. CEF's prebuilt `bootstrap.exe` (statically linked with `cef_sandbox`) is renamed
#    to `opencode-browser-engine.exe` and patched to an 8 MiB stack reserve (#582, matching
#    `chrome.exe` so V8 compiling large MV3 service workers never overflows the default 1 MiB stack);
#    it loads our `opencode-browser-engine.dll` (`RunWinMain`) for both browser and sandboxed
#    subprocesses. Left out of CEF's distribution: the console bootstrap (`bootstrapc.exe`), the
#    import library, SwiftShader and the Vulkan loader (without a GPU, Chromium draws in software
#    and WebGL runs on WARP), the bundled d3dcompiler_47.dll (Windows ships it), gendered locale
#    variants, and unused locales. DXC (dxcompiler.dll) stays: WebGPU needs it.
$files = @('libcef.dll', 'dxcompiler.dll', 'resources.pak', 'icudtl.dat', 'chrome_elf.dll', 'chrome_200_percent.pak', 'v8_context_snapshot.bin', 'chrome_100_percent.pak', 'CREDITS.html', 'LICENSE.txt')
$appLocales = (Get-Content -Raw (Join-Path $Repo 'crates\opencode-i18n\locales\locales.json') | ConvertFrom-Json).locales.id
# The app's language ids that Chromium names otherwise (browser-pane `chromium_locale`).
$alias = @{ en = 'en-US'; zh = 'zh-CN'; zht = 'zh-TW'; no = 'nb' }
$locales = $appLocales | ForEach-Object { if ($alias[$_]) { $alias[$_] } else { $_ } } |
  Where-Object { Test-Path (Join-Path $cefDir "locales\$_.pak") } | Sort-Object -Unique
$stage = Join-Path $Out "stage-$Arch"
$app = Join-Path $stage 'OpenCode Browser Engine'
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force (Join-Path $app 'locales') | Out-Null
foreach ($f in $files) { Copy-Item (Join-Path $cefDir $f) $app }
foreach ($l in $locales) { Copy-Item (Join-Path $cefDir "locales\$l.pak") (Join-Path $app 'locales') }
$engineExe = Join-Path $app 'opencode-browser-engine.exe'
$engineDll = Join-Path $app 'opencode-browser-engine.dll'
Copy-Item (Join-Path $cefDir 'bootstrap.exe') $engineExe
Copy-Item (Join-Path $bin 'opencode_browser_engine.dll') $engineDll

# Patch `bootstrap.exe`'s PE stack reserve from 1 MiB to 8 MiB before signing (#582).
$editbin = (Get-Command editbin.exe -ErrorAction SilentlyContinue).Source
if (-not $editbin) {
  $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
  if (Test-Path -LiteralPath $vswhere) {
    $vs = (& $vswhere -latest -products * -property installationPath | Select-Object -First 1)
    if ($vs) {
      $cands = @(Get-ChildItem "$vs\VC\Tools\MSVC" -Recurse -Filter 'editbin.exe' -ErrorAction SilentlyContinue |
        Sort-Object FullName -Descending | Select-Object -ExpandProperty FullName)
      $hostPref = if ([Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq [Runtime.InteropServices.Architecture]::Arm64) {
        'Hostarm64\\arm64\\editbin\.exe$'
      } else {
        'Hostx64\\x64\\editbin\.exe$'
      }
      $editbin = ($cands | Where-Object { $_ -match $hostPref } | Select-Object -First 1)
      if (-not $editbin) { $editbin = ($cands | Where-Object { $_ -match 'Hostx64\\x64\\editbin\.exe$' } | Select-Object -First 1) }
      if (-not $editbin) { $editbin = ($cands | Select-Object -First 1) }
    }
  }
}
if (-not $editbin) { throw 'editbin.exe not found on PATH or in Visual Studio Build Tools.' }
& $editbin /NOLOGO /STACK:8388608 $engineExe
if ($LASTEXITCODE -ne 0) { throw "editbin /STACK:8388608 failed on $engineExe" }
$peBytes = [IO.File]::ReadAllBytes($engineExe)
$peOff = [BitConverter]::ToInt32($peBytes, 0x3C)
$optMagic = [BitConverter]::ToUInt16($peBytes, $peOff + 24)
if ($optMagic -ne 0x20B) { throw "Unexpected PE optional header magic 0x$($optMagic.ToString('x')) in $engineExe" }
$stackReserve = [BitConverter]::ToUInt64($peBytes, $peOff + 24 + 72)
if ($stackReserve -lt 8388608) { throw "Expected >= 8 MiB stack reserve on $engineExe, got $stackReserve" }

$entries = @($files + @('opencode-browser-engine.exe', 'opencode-browser-engine.dll') + ($locales | ForEach-Object { "locales/$_.pak" })) |
  ForEach-Object { "OpenCode Browser Engine/$_" }
if ($StageOnly) {
  Write-Host "staged $stage"
  return
}
} else {
  # The stage a -StageOnly run made, its programs signed since: archived in the same order.
  $stage = $ArchiveOnly
  New-Item -ItemType Directory -Force $Out | Out-Null
  $app = Join-Path $stage 'OpenCode Browser Engine'
  $order = @('libcef.dll', 'dxcompiler.dll', 'resources.pak', 'icudtl.dat', 'chrome_elf.dll', 'chrome_200_percent.pak', 'v8_context_snapshot.bin', 'chrome_100_percent.pak', 'CREDITS.html', 'LICENSE.txt', 'opencode-browser-engine.exe', 'opencode-browser-engine.dll')
  $locales = @(Get-ChildItem (Join-Path $app 'locales') -Filter *.pak | ForEach-Object BaseName)
  $entries = @($order | Where-Object { Test-Path (Join-Path $app $_) }) + @(Get-ChildItem (Join-Path $app 'locales') -Filter *.pak | Sort-Object Name | ForEach-Object { "locales/$($_.Name)" }) |
    ForEach-Object { "OpenCode Browser Engine/$_" }
}

# 4. The archive: ustar, zstd level 19 (an 8 MB window, so unpacking needs little memory).
$name = "OpenCode-Browser-Engine-$Version-windows-$Arch.tar.zst"
$zst = Join-Path $Out $name
Remove-Item $zst -ErrorAction SilentlyContinue
& "$env:SystemRoot\System32\tar.exe" --format ustar --zstd --options 'zstd:compression-level=19' -cf $zst -C $stage @entries
if ($LASTEXITCODE -ne 0) { throw 'tar failed' }
$size = (Get-Item $zst).Length
$sha256 = (Get-FileHash -Algorithm SHA256 $zst).Hash.ToLower()

# 5. The index, in the release service's manifest shape: the app reads release.version and
#    release.targets.<os>-<arch>.{url, size, sha256}, url relative to the index.
$indexPath = Join-Path $Out 'index.json'
$index = if (Test-Path $indexPath) { Get-Content -Raw $indexPath | ConvertFrom-Json -AsHashtable } else { $null }
if (-not $index -or $index.release.version -ne "$CefBuild-$Revision") {
  $index = @{ schema = 1; artifact = 'browser-engine'; release = @{ version = "$CefBuild-$Revision"; targets = @{} } }
}
$index.release.targets["windows-$Arch"] = @{ url = $name; size = $size; sha256 = $sha256 }
$index | ConvertTo-Json -Depth 6 | Set-Content -Encoding utf8NoBOM $indexPath

$raw = (Get-ChildItem $app -Recurse -File | Measure-Object Length -Sum).Sum
Write-Host ("{0}: {1:N0} bytes ({2:N0} unpacked, {3} files, {4} locales)" -f $name, $size, $raw, $entries.Count, $locales.Count)
Write-Host "sha256 $sha256"
