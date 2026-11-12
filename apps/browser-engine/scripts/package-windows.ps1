# Builds the browser engine for Windows and packages it as the app downloads it:
#   <out>\OpenCode-Browser-Engine-<version>-windows-<arch>.tar.zst
#   <out>\index.json   (the release index the app reads; each run adds or replaces its own target)
# The archive holds one folder, `OpenCode Browser Engine`: both executables and the part of CEF's
# runtime a side-panel browser uses.
# TODO(IMPORTANT before release): the engine's delivery and trust root are decided when the app
# moves into the opencode repo. Nothing here signs or publishes: with a publisher trust rule, every
# .exe and .dll in the stage folder is signed before step 4 archives it.
#
#   pwsh apps/browser-engine/scripts/package-windows.ps1 [-Arch x64|arm64] [-Out <dir>] [-Cargo <cargo or wrapper.ps1>]
#   bun apps/browser-engine/scripts/serve.ts [<out>]   # serves it where a development build looks
#
# Needs: Rust's target for -Arch, MSVC's build tools for it, and a Windows tar.exe with zstd
# (Windows 11). CEF is Spotify's prebuilt minimal distribution, fetched once and checked against
# the SHA-256 pinned below; cef-dll-sys in Cargo.lock must name the same CEF build.
param(
  [ValidateSet('x64', 'arm64')][string]$Arch = 'x64',
  [string]$Out,
  [string]$Cargo = 'cargo',
  # Package what cargo last built (a build run elsewhere, or a repackage).
  [switch]$SkipBuild
)
$ErrorActionPreference = 'Stop'
$Engine = Split-Path -Parent $PSScriptRoot
$Repo = Split-Path -Parent (Split-Path -Parent $Engine)
if (-not $Out) { $Out = Join-Path $Engine 'target\package' }

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

# 1. CEF, laid out as cef-dll-sys expects it under CEF_PATH (<build>\<dir>, with archive.json).
$env:CEF_PATH = Join-Path $Engine 'target\cef'
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

# 2. The engine.
if (-not $SkipBuild) {
  Write-Host "Building opencode-browser-engine $Version ($($T.Triple))"
  $cargoArgs = @('build', '--release', '--locked', '--manifest-path', (Join-Path $Engine 'Cargo.toml'), '--bins', '--target', $T.Triple)
  # The C runtime linked in, as the app's, so the engine's executables need no Visual C++
  # Redistributable. RUSTFLAGS replaces the repository's .cargo/config.toml flags, whose
  # profile-guided ones are the app's.
  $rustflags = $env:RUSTFLAGS
  $env:RUSTFLAGS = '-C target-feature=+crt-static -C link-arg=/NODEFAULTLIB:libucrt.lib -C link-arg=/DEFAULTLIB:ucrt.lib'
  try { if ($Cargo -like '*.ps1') { & pwsh -NoProfile $Cargo @cargoArgs } else { & $Cargo @cargoArgs } } finally { $env:RUSTFLAGS = $rustflags }
  if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
}
$bin = Join-Path $Engine "target\$($T.Triple)\release"

# 3. The folder: what a side-panel browser loads, biggest first so the app can check it while the
#    rest arrives. Left out of CEF's distribution: its static-link sandbox launchers
#    (bootstrap*.exe), the import library, SwiftShader and the Vulkan loader (without a GPU,
#    Chromium draws in software and WebGL runs on WARP), the bundled d3dcompiler_47.dll (WebGL's
#    shaders compile with the one Windows 10 and 11 ship), the gendered locale variants, and every
#    locale the app has no language for (it starts the engine in its own). DXC (dxcompiler.dll)
#    stays: WebGPU can't create a device without it.
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
Copy-Item (Join-Path $bin 'opencode-browser-engine.exe'), (Join-Path $bin 'opencode-browser-engine-helper.exe') $app
$entries = @($files + @('opencode-browser-engine.exe', 'opencode-browser-engine-helper.exe') + ($locales | ForEach-Object { "locales/$_.pak" })) |
  ForEach-Object { "OpenCode Browser Engine/$_" }

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
