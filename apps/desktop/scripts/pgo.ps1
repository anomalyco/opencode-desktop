# Makes the release build's profile-guided optimisation profile, `apps/desktop/pgo/<target>.profdata`
# (and its stamp, `<target>.txt`), from a training run of the app as it ships:
#
#   1. builds the app instrumented (`cargo build --release --target <triple>` with `-Cprofile-generate`;
#      the exe lands in target/<triple>/release, apart from the plain release build);
#   2. runs -Train as `<Train> --exe <instrumented exe> --raw <folder>`: it starts the app the way a
#      user does, uses it, and quits it through its window (LLVM's runtime writes the counts at exit,
#      one `.profraw` per process, into the folder; set LLVM_PROFILE_FILE=<folder>\<name>-%p.profraw);
#   3. merges them, keeping only functions that ran (`llvm-profdata merge --sparse`).
#
#   pwsh apps/desktop/scripts/pgo.ps1 -Train "<command>" [-Target <triple>] [-Cargo <command>]
#
# The training run belongs to whoever measures the app (it needs a scratch profile and a window to
# drive); what it covers and how to refresh the profile: apps/desktop/pgo/README.md.
param(
    [Parameter(Mandatory)] [string] $Train,
    [string] $Target = '',
    [string] $Cargo = 'cargo'
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../../..')).Path
$hostTriple = ((rustc -vV | Select-String '^host: ').Line -replace '^host: ', '').Trim()
$triple = if ($Target) { $Target } else { $hostTriple }
$sysroot = (rustc --print sysroot).Trim()
$profdata = Join-Path $sysroot "lib/rustlib/$hostTriple/bin/llvm-profdata.exe"
if (-not (Test-Path $profdata)) {
    rustup component add llvm-tools | Out-Null
    if (-not (Test-Path $profdata)) { throw "no llvm-profdata at $profdata" }
}
$name = 'CARGO_TARGET_' + ($triple.ToUpperInvariant() -replace '[-.]', '_') + '_RUSTFLAGS'
$before = [Environment]::GetEnvironmentVariable($name)
$raw = Join-Path $root "target/pgo-raw/$triple"
Remove-Item -Recurse -Force $raw -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $raw | Out-Null
try {
    # `--target` keeps build scripts and proc macros out of it: only the app's own code counts.
    [Environment]::SetEnvironmentVariable($name, (@($before, '-Cprofile-generate=target/pgo-raw') | Where-Object { $_ }) -join ' ')
    $parts = $Cargo -split ' '
    & $parts[0] @($parts | Select-Object -Skip 1) build --release --target $triple -p opencode-desktop
    if ($LASTEXITCODE -ne 0) { throw 'the instrumented build failed' }
} finally {
    [Environment]::SetEnvironmentVariable($name, $before)
}
$exe = Join-Path $root "target/$triple/release/opencode-desktop.exe"
$command = $Train -split ' '
& $command[0] @($command | Select-Object -Skip 1) --exe $exe --raw $raw
if ($LASTEXITCODE -ne 0) { throw 'the training run failed' }
$files = @(Get-ChildItem $raw -Filter *.profraw)
if (-not $files) { throw "the training run wrote no .profraw into $raw" }
$out = Join-Path $root "apps/desktop/pgo/$triple.profdata"
& $profdata merge --sparse -o $out @($files.FullName)
if ($LASTEXITCODE -ne 0) { throw 'llvm-profdata merge failed' }
$summary = & $profdata show $out | Select-String 'Total functions'
$rune = (Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^rune = .*rev = "([0-9a-f]+)"').Matches[0].Groups[1].Value
@(
    "commit $(git -C $root rev-parse HEAD)"
    "rune $rune"
    "rustc $((rustc -V).Trim())"
    "processes $($files.Count)"
    "$summary".Trim()
) | Set-Content (Join-Path $root "apps/desktop/pgo/$triple.txt")
'{0}: {1:N0} bytes, {2} processes, {3}' -f $out, (Get-Item $out).Length, $files.Count, "$summary".Trim()
