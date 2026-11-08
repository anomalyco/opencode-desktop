# Builds OpenCode (Rune)'s Windows artifacts into -Out:
#
#   OpenCode-Rune-<version>-windows-x64.zip        the install folder, the updater's windows-x64 target
#   OpenCode-Rune-<version>-windows-x64-setup.exe  the setup, with that zip inside
#
#   pwsh apps/installer/scripts/package.ps1 -Out <folder> [-Profile release|dev] [-Cargo <command>]
#
# Environment, as the macOS build reads it (every artifact of a build gets the same):
#   OPENCODE_RUNE_VERSION, OPENCODE_RUNE_BUILD_CHANNEL   required; `opencode-desktop --version` must report them
#   OPENCODE_RUNE_WINDOW_TITLE                           default "OpenCode (Rune)"
#   OPENCODE_RUNE_BUNDLE_ID                              default "anomaly.opencode-rune"
#   OPENCODE_RUNE_URL_SCHEME                             default "opencode-rune"
#
# -Cargo runs cargo another way, such as a build throttle ("pwsh tools/cargo.ps1 +1.99.1").
#
# TODO(IMPORTANT before release): add a build-windows job to .github/workflows/opencode-rune-release.yml (windows-latest, x64 and arm64) that runs this script, signs the artifacts (below), uploads the zip as target windows-x64 / windows-arm64 in opencode-rune/desktop/<channel>.json with its size and SHA-256, and publishes the setup as the Windows download.
# TODO(IMPORTANT before release): Authenticode-sign opencode-desktop.exe before it's zipped and the setup after it's built (signtool sign /fd sha256 /tr <timestamp> ...), with the certificate the owner picks; the updater refuses unsigned builds (updater::windows::EXPECTED_PUBLISHER).
param(
    [Parameter(Mandatory)] [string] $Out,
    [ValidateSet('release', 'dev')] [string] $Profile = 'release',
    [string] $Cargo = 'cargo'
)
$ErrorActionPreference = 'Stop'

$version = $env:OPENCODE_RUNE_VERSION
$channel = $env:OPENCODE_RUNE_BUILD_CHANNEL
if (-not $version -or -not $channel) { throw 'Set OPENCODE_RUNE_VERSION and OPENCODE_RUNE_BUILD_CHANNEL.' }
$root = (Resolve-Path (Join-Path $PSScriptRoot '../../..')).Path
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
$bin = Join-Path $target ($(if ($Profile -eq 'release') { 'release' } else { 'debug' }))
$release = if ($Profile -eq 'release') { @('--release') } else { @() }
$stem = "OpenCode-Rune-$version-windows-x64"
New-Item -ItemType Directory -Force $Out | Out-Null
$Out = (Resolve-Path $Out).Path

function Invoke-Cargo([string[]] $Arguments) {
    $parts = $Cargo -split ' '
    & $parts[0] @($parts | Select-Object -Skip 1) @Arguments
    if ($LASTEXITCODE -ne 0) { throw "cargo $($Arguments -join ' ') failed" }
}

Invoke-Cargo (@('build', '-p', 'opencode-desktop') + $release)
$exe = Join-Path $bin 'opencode-desktop.exe'
$reported = (& $exe --version | Out-String).Trim()
if ($reported -ne "opencode-rune $version ($channel)") { throw "opencode-desktop --version reported '$reported', expected 'opencode-rune $version ($channel)'" }

# The install folder: the app alone. The browser engine downloads in the background.
$stage = Join-Path $Out 'stage'
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory $stage | Out-Null
Copy-Item $exe $stage
$zip = Join-Path $Out "$stem.zip"
Remove-Item -Force $zip -ErrorAction SilentlyContinue
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
Remove-Item -Recurse -Force $stage

$env:OPENCODE_RUNE_SETUP_PAYLOAD = $zip
try {
    Invoke-Cargo (@('build', '-p', 'opencode-rune-setup') + $release)
} finally {
    Remove-Item Env:OPENCODE_RUNE_SETUP_PAYLOAD
}
$setup = Join-Path $Out "$stem-setup.exe"
Copy-Item (Join-Path $bin 'opencode-rune-setup.exe') $setup -Force

foreach ($file in $zip, $setup) {
    $hash = (Get-FileHash -Algorithm SHA256 $file).Hash.ToLowerInvariant()
    '{0}  {1}  {2:N0} bytes' -f $hash, (Split-Path -Leaf $file), (Get-Item $file).Length
}
