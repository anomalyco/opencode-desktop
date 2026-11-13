# Builds OpenCode (Rune) as it ships: `cargo build --release` (fat LTO and one codegen unit, from
# Cargo.toml's release profile) with the profile-guided optimisation profile of the target,
# `apps/desktop/pgo/<target>.profdata`, when there is one. package.ps1 builds the app through this,
# and so should any release build that times the app: without the profile it's 14-22 % slower per
# frame (notes in apps/desktop/pgo/README.md).
#
#   pwsh apps/desktop/scripts/release.ps1 [-Target <triple>] [-Cargo <command>] [-Package <name>] [-Features <list>] [-StaleProfileOk]
#
# A profile 20 or more app commits older than the code (or one whose training commit isn't in this
# clone's history) is stale: it keeps a quarter of its gain (a day of commits gave 4-8 % per frame
# instead of 23-30 %). Outside the release workflow this refuses to build with one: retrain it first
# (apps/desktop/scripts/pgo.ps1), or pass
# -StaleProfileOk (or set OPENCODE_RUNE_STALE_PROFILE_OK=1, which reaches it through package.ps1) for a
# build whose speed doesn't matter. The release workflow (OPENCODE_RUNE_BUILD_CHANNEL set) only warns.
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
    [string] $Features = '',
    [switch] $StaleProfileOk
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
    # Commits to the app since the training commit; none when the stamp or that commit is missing
    # (a shallow clone, as CI's checkout is, has no history to count).
    $known = $false
    if ($trained) { git -C $root cat-file -e "$trained^{commit}" 2>$null; $known = $LASTEXITCODE -eq 0 }
    $since = if ($known) { [int](git -C $root rev-list --count "$trained..HEAD" -- apps crates Cargo.lock) } else { $null }
    $age = if ($null -ne $since) { "$since commits to the app since" } else { 'its age unknown (the training commit is not in this clone)' }
    Write-Host "profile-guided: $triple.profdata, trained at $(if ($trained) { $trained.Substring(0, 12) } else { '?' }), $age"
    if ($null -eq $since -or $since -ge 20) {
        $why = "the profile-guided optimisation profile is stale ($age): it no longer names most of the code, so the build keeps a quarter of its speed-up. Retrain it (apps/desktop/scripts/pgo.ps1, apps/desktop/pgo/README.md)"
        $ok = $StaleProfileOk -or $env:OPENCODE_RUNE_STALE_PROFILE_OK -eq '1'
        if ($env:OPENCODE_RUNE_BUILD_CHANNEL) {
            # The release workflow's call (docs/releasing.md); refusing there is its owner's decision.
            if ($env:GITHUB_ACTIONS) { Write-Host "::warning::$why" } else { Write-Warning $why }
        } elseif ($ok) {
            Write-Warning "$why (building anyway: -StaleProfileOk)"
        } else {
            throw "$why; or pass -StaleProfileOk (OPENCODE_RUNE_STALE_PROFILE_OK=1) for a build whose speed doesn't matter"
        }
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
