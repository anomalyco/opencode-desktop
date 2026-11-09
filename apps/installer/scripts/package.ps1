# Builds OpenCode (Rune)'s Windows artifacts into -Out:
#
#   OpenCode-Rune-<version>-windows-<arch>.zip        the install folder, the updater's windows-<arch> target
#   OpenCode-Rune-<version>-windows-<arch>-setup.exe  the setup, with that zip inside
#
#   pwsh apps/installer/scripts/package.ps1 -Out <folder> [-Profile release|dev] [-Arch x64|arm64] [-Cargo <command>]
#
# -Arch defaults to this machine's. Another arch cross-builds (`--target`), and the app's
# `--version` check is skipped, because this machine can't run it.
#
# Environment, as the macOS build reads it (every artifact of a build gets the same):
#   OPENCODE_RUNE_VERSION, OPENCODE_RUNE_BUILD_CHANNEL   required; `opencode-desktop --version` must report them
#   OPENCODE_RUNE_WINDOW_TITLE                           default "OpenCode (Rune)"
#   OPENCODE_RUNE_BUNDLE_ID                              default "anomaly.opencode-rune"
#   OPENCODE_RUNE_URL_SCHEME                             default "opencode-rune"
#
# -Cargo runs cargo another way, such as a build throttle ("pwsh tools/cargo.ps1 +1.99.1").
#
# TODO(IMPORTANT before release): .github/workflows/opencode-rune-release.yml runs this script (build-windows, x64 and arm64) and publishes the unsigned setups as a GitHub pre-release; once sign-windows signs them (below), upload the zip as target windows-x64 / windows-arm64 in opencode-rune/desktop/<channel>.json with its size and SHA-256, and publish the setup as the Windows download.
# TODO(IMPORTANT before release): Authenticode-sign opencode-desktop.exe before it's zipped and the setup after it's built (signtool sign /fd sha256 /tr <timestamp> ...), with the certificate the owner picks; the updater refuses unsigned builds (updater::windows::EXPECTED_PUBLISHER).
param(
    [Parameter(Mandatory)] [string] $Out,
    [ValidateSet('release', 'dev')] [string] $Profile = 'release',
    [ValidateSet('', 'x64', 'arm64')] [string] $Arch = '',
    [string] $Cargo = 'cargo'
)
$ErrorActionPreference = 'Stop'

$version = $env:OPENCODE_RUNE_VERSION
$channel = $env:OPENCODE_RUNE_BUILD_CHANNEL
if (-not $version -or -not $channel) { throw 'Set OPENCODE_RUNE_VERSION and OPENCODE_RUNE_BUILD_CHANNEL.' }
$root = (Resolve-Path (Join-Path $PSScriptRoot '../../..')).Path
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
$hostArch = if ([Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq 'Arm64') { 'arm64' } else { 'x64' }
if (-not $Arch) { $Arch = $hostArch }
$cross = $Arch -ne $hostArch
$triple = @{ x64 = 'x86_64-pc-windows-msvc'; arm64 = 'aarch64-pc-windows-msvc' }[$Arch]
if ($cross) { $target = Join-Path $target $triple }
$bin = Join-Path $target ($(if ($Profile -eq 'release') { 'release' } else { 'debug' }))
$flags = @(if ($Profile -eq 'release') { '--release' }) + @(if ($cross) { '--target', $triple })
$stem = "OpenCode-Rune-$version-windows-$Arch"
New-Item -ItemType Directory -Force $Out | Out-Null
$Out = (Resolve-Path $Out).Path

function Invoke-Cargo([string[]] $Arguments) {
    $parts = $Cargo -split ' '
    & $parts[0] @($parts | Select-Object -Skip 1) @Arguments
    if ($LASTEXITCODE -ne 0) { throw "cargo $($Arguments -join ' ') failed" }
}

Invoke-Cargo (@('build', '-p', 'opencode-desktop') + $flags)
$exe = Join-Path $bin 'opencode-desktop.exe'
if (-not $cross) {
    $reported = (& $exe --version | Out-String).Trim()
    if ($reported -ne "opencode-rune $version ($channel)") { throw "opencode-desktop --version reported '$reported', expected 'opencode-rune $version ($channel)'" }
}

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
    Invoke-Cargo (@('build', '-p', 'opencode-rune-setup') + $flags)
} finally {
    Remove-Item Env:OPENCODE_RUNE_SETUP_PAYLOAD
}
$setup = Join-Path $Out "$stem-setup.exe"
Copy-Item (Join-Path $bin 'opencode-rune-setup.exe') $setup -Force

foreach ($file in $zip, $setup) {
    $hash = (Get-FileHash -Algorithm SHA256 $file).Hash.ToLowerInvariant()
    '{0}  {1}  {2:N0} bytes' -f $hash, (Split-Path -Leaf $file), (Get-Item $file).Length
}
