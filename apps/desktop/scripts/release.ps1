# Builds OpenCode (Rune) as it ships: `cargo build --release` (fat LTO and one codegen unit, from
# Cargo.toml's release profile) with the profile-guided optimisation profile of the target,
# `apps/desktop/pgo/<target>.profdata`, when there is one. package.ps1 builds the app through this,
# and so should any release build that times the app: without the profile it's 14-22 % slower per
# frame (notes in apps/desktop/pgo/README.md).
#
#   pwsh apps/desktop/scripts/release.ps1 [-Target <triple>] [-Cargo <command>] [-Package <name>] [-Features <list>]
#
# -Cargo runs cargo another way, such as a build throttle ("pwsh tools/cargo.ps1"). The profile
# rides on CARGO_TARGET_<TRIPLE>_RUSTFLAGS, which cargo joins with .cargo/config.toml's flags for
# that target. The target is always named (`--target`), as pgo.ps1 names it for the training
# build: cargo gives a crate built for a named target other symbol hashes than one built for the
# host, and a profile only applies to the functions it names. So the exe is in
# target/<triple>/release/.
param(
    [string] $Target = '',
    [string] $Cargo = 'cargo',
    [string] $Package = 'opencode-desktop',
    [string] $Features = ''
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../../..')).Path
$hostTriple = ((rustc -vV | Select-String '^host: ').Line -replace '^host: ', '').Trim()
$triple = if ($Target) { $Target } else { $hostTriple }
$profdata = Join-Path $root "apps/desktop/pgo/$triple.profdata"
$name = 'CARGO_TARGET_' + ($triple.ToUpperInvariant() -replace '[-.]', '_') + '_RUSTFLAGS'
$before = [Environment]::GetEnvironmentVariable($name)
if (Test-Path $profdata) {
    # Functions changed since the training run keep no counts and build as without a profile; LLVM
    # would warn once per such function.
    [Environment]::SetEnvironmentVariable($name, (@($before, "-Cprofile-use=$profdata", '-Cllvm-args=-no-pgo-warn-mismatch') | Where-Object { $_ }) -join ' ')
    $stamp = Join-Path $root "apps/desktop/pgo/$triple.txt"
    $trained = if (Test-Path $stamp) { (Get-Content $stamp | Select-String '^commit ').Line -replace '^commit ', '' }
    if ($trained) {
        $since = (git -C $root rev-list --count "$trained..HEAD" -- apps crates Cargo.lock 2>$null)
        Write-Host "profile-guided: $triple.profdata, trained at $($trained.Substring(0, 12)), $since commits to the app since"
        # A day of commits (58, with Rune bumps) left a profile worth 4-9 % per frame where a fresh one gave
        # 23-30 %: the functions it names have changed. Retrain before a release.
        if ([int]$since -ge 20) { Write-Warning "the profile is $since commits old and mostly no longer matches the code: retrain it (apps/desktop/scripts/pgo.ps1) before a release" }
    }
} else {
    Write-Host "no profile for ${triple}: building without profile-guided optimisation"
}
try {
    $parts = $Cargo -split ' '
    $targetArgs = @('--target', $triple)
    $featureArgs = if ($Features) { @('--features', $Features) } else { @() }
    & $parts[0] @($parts | Select-Object -Skip 1) build --release -p $Package @targetArgs @featureArgs
    if ($LASTEXITCODE -ne 0) { throw "cargo build --release failed" }
} finally {
    [Environment]::SetEnvironmentVariable($name, $before)
}
