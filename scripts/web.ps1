<#
.SYNOPSIS
  Builds OpenCode (Rune) for the browser (apps/web) and serves it on a local port.

.DESCRIPTION
  Builds apps/web for wasm32-unknown-unknown, runs wasm-bindgen, and puts the page in
  target/web/. Then serves that folder on http://127.0.0.1:<Port>; open it in Chrome or Edge
  (WebGPU) and give it a server URL, or open it with ?server=<url> (and &password=<pw>).

  The OpenCode server must allow the page's origin: a page on http://localhost:<any> or
  http://127.0.0.1:<any> always is. A page anywhere else needs `opencode serve --cors <origin>`.

  Needs: rustup's wasm32-unknown-unknown target, wasm-bindgen-cli at the Cargo.lock version
  (cargo install wasm-bindgen-cli --version <v> --locked), WASI SDK 33 or newer for Luau's C++
  (WASI_SDK_PATH, or -WasiSdk), and Bun for the server.

.EXAMPLE
  pwsh scripts/web.ps1                      # release build, serve on 8790
  pwsh scripts/web.ps1 -Dev -Port 8791      # quicker build, bigger and slower page
  pwsh scripts/web.ps1 -NoServe             # build only (CI)
  pwsh scripts/web.ps1 -NoBuild             # serve the last build
  pwsh scripts/web.ps1 -SkipCargo           # package what cargo last built (a build run elsewhere)
  pwsh scripts/web.ps1 -Check               # cargo check the app for the web, nothing else (before a push)
  pwsh scripts/web.ps1 -Check -Cargo <wrapper.ps1>   # the same through a cargo wrapper script
#>
param(
  [int]$Port = 8790,
  [switch]$Dev,
  [switch]$NoBuild,
  [switch]$SkipCargo,
  [switch]$NoServe,
  [switch]$Check,
  [string]$Cargo = $env:CARGO,
  [string]$WasiSdk = $env:WASI_SDK_PATH,
  [string]$Out
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
if (-not $Out) { $Out = Join-Path $root 'target/web' }
$buildProfile = if ($Dev) { 'debug' } else { 'release' }

if (-not $NoBuild) {
  if (-not $WasiSdk -or -not (Test-Path (Join-Path $WasiSdk 'bin'))) {
    throw 'Set WASI_SDK_PATH (or pass -WasiSdk) to an unpacked WASI SDK 33 or newer: Luau builds for the web with its clang.'
  }
  $exe = if ($IsWindows) { '.exe' } else { '' }
  $env:CC_wasm32_unknown_unknown = Join-Path $WasiSdk "bin/clang$exe"
  $env:CXX_wasm32_unknown_unknown = Join-Path $WasiSdk "bin/clang++$exe"
  $env:AR_wasm32_unknown_unknown = Join-Path $WasiSdk "bin/llvm-ar$exe"
  $env:CXXSTDLIB_wasm32_unknown_unknown = 'c++'
  $env:CFLAGS_wasm32_unknown_unknown = '--target=wasm32-wasip1'
  # The wasm target's C++ alone (cc's per-target variable): the app's build script also compiles
  # Luau for the host, which must not get these. WebAssembly exception handling, as Rune's web build.
  $env:CXXFLAGS_wasm32_unknown_unknown = '--target=wasm32-wasip1 -fexceptions -fwasm-exceptions -mllvm -wasm-use-legacy-eh=false'
  $env:WASI_SDK_PATH = $WasiSdk
  # A wrapper script runs through pwsh; anything else is cargo itself.
  $runCargo = if (-not $Cargo) { 'cargo' } elseif ($Cargo -like '*.ps1') { 'pwsh' } else { $Cargo }
  $wrap = if ($Cargo -like '*.ps1') { @('-NoProfile', '-File', $Cargo) } else { @() }
  if ($Check) {
    Push-Location $root
    try { & $runCargo @wrap check -p opencode-web --lib --target wasm32-unknown-unknown; exit $LASTEXITCODE } finally { Pop-Location }
  }
  $cargoArgs = @('build', '-p', 'opencode-web', '--lib', '--target', 'wasm32-unknown-unknown')
  if (-not $Dev) {
    # Small rather than fast to build: one codegen unit, size-optimised, thin LTO, no symbols.
    $cargoArgs += '--release'
    $env:CARGO_PROFILE_RELEASE_OPT_LEVEL = 's'
    $env:CARGO_PROFILE_RELEASE_LTO = 'thin'
    $env:CARGO_PROFILE_RELEASE_CODEGEN_UNITS = '1'
    $env:CARGO_PROFILE_RELEASE_DEBUG = '0'
    $env:CARGO_PROFILE_RELEASE_STRIP = 'debuginfo'
    $env:CARGO_PROFILE_RELEASE_INCREMENTAL = 'false'
  } else {
    $env:CARGO_PROFILE_DEV_DEBUG = '0'
  }
  if (-not $SkipCargo) {
    Push-Location $root
    try { & $runCargo @wrap @cargoArgs; if ($LASTEXITCODE) { throw "cargo build failed ($LASTEXITCODE)" } } finally { Pop-Location }
  }

  $meta = & cargo metadata --format-version 1 --no-deps --manifest-path (Join-Path $root 'Cargo.toml') | ConvertFrom-Json
  $wasm = Join-Path $meta.target_directory "wasm32-unknown-unknown/$buildProfile/opencode_web.wasm"
  $want = (Select-String (Join-Path $root 'Cargo.lock') -Pattern '^name = "wasm-bindgen"$' -Context 0, 1).Context.PostContext[0] -replace '.*"(.*)".*', '$1'
  $have = (& wasm-bindgen --version) -replace 'wasm-bindgen ', ''
  if ($have -ne $want) { throw "wasm-bindgen $have doesn't match Cargo.lock's ${want}: cargo install wasm-bindgen-cli --version $want --locked" }

  # Build into a staging folder, then swap it in, so a page loading mid-build never mixes builds.
  $next = "$Out.next"
  Remove-Item -Recurse -Force $next -ErrorAction SilentlyContinue
  New-Item -ItemType Directory -Force $next | Out-Null
  $bindgen = @('--target', 'web', '--out-dir', $next, '--out-name', 'app')
  if (-not $Dev) { $bindgen += '--remove-name-section' }
  & wasm-bindgen @bindgen $wasm
  if ($LASTEXITCODE) { throw "wasm-bindgen failed ($LASTEXITCODE)" }
  Copy-Item (Join-Path $root 'apps/web/index.html') $next
  # Rune draws the first frame with Inter cut to Latin, Greek and Cyrillic and then fetches the
  # full faces from beside app.js: copy those of the Rune this build compiled.
  $full = & cargo metadata --format-version 1 --filter-platform wasm32-unknown-unknown --manifest-path (Join-Path $root 'Cargo.toml') | ConvertFrom-Json
  $rune = $full.packages | Where-Object { $_.name -eq 'rune' } | Select-Object -First 1
  if ($rune) { Copy-Item (Join-Path (Split-Path $rune.manifest_path) 'assets/web/Inter-*.ttf') $next }
  Remove-Item -Recurse -Force $Out -ErrorAction SilentlyContinue
  Move-Item $next $Out
  $size = (Get-Item (Join-Path $Out 'app_bg.wasm')).Length
  Write-Host ("Built {0}: app_bg.wasm {1:N0} bytes. Folder: {2}" -f $buildProfile, $size, $Out)
}

if (-not $NoServe) {
  if (-not (Test-Path (Join-Path $Out 'index.html'))) { throw "Nothing built in $Out yet: run without -NoBuild first." }
  Write-Host "Serving $Out on http://127.0.0.1:$Port/ (Ctrl+C stops it)"
  & bun (Join-Path $PSScriptRoot 'web-serve.ts') $Out $Port
}
