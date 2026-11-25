# Builds OpenCode (Rune)'s Windows artifacts into -Out:
#
#   OpenCode-Rune-<version>-windows-<arch>.zip        the install folder (opencode-desktop.exe, opencode-desktop.dll,
#                                                     the toasts' icon, the licences, the launch's page list
#                                                     where this machine can run the app), the updater's
#                                                     windows-<arch> target
#   OpenCode-Rune-<version>-windows-<arch>-setup.exe  the setup, with that zip inside
#
#   pwsh apps/installer/scripts/package.ps1 -Out <folder> [-Profile release|dev] [-Arch x64|arm64] [-Cargo <command>] [-Payload <zip>]
#
# -Arch defaults to this machine's. Another arch cross-builds (`--target`), and the app's
# `--version` check is skipped, because this machine can't run it.
#
# -Payload builds only the setup, around a zip built before: the release workflow's
# package-windows job wraps the zip of the install folder that sign-windows signed, so the setup
# installs the same signed files the updater downloads. It writes the setup alone.
#
# Environment, as the macOS build reads it (every artifact of a build gets the same):
#   OPENCODE_RUNE_VERSION, OPENCODE_RUNE_BUILD_CHANNEL   required; `opencode-desktop --version` must report them
#   OPENCODE_RUNE_WINDOW_TITLE                           default "OpenCode (Rune)"
#   OPENCODE_RUNE_BUNDLE_ID                              default "anomaly.opencode-rune"
#   OPENCODE_RUNE_URL_SCHEME                             default "opencode-rune"
#
# -Cargo runs cargo another way, such as a build throttle ("pwsh tools/cargo.ps1 +1.99.1").
#
# .github/workflows/release.yml runs this script in build-windows (x64 and arm64;
# the unsigned setups go to the rolling GitHub pre-release), signs the install folder through
# Azure Artifact Signing, runs it again with -Payload around the signed zip, signs that setup,
# and publishes the zip as target windows-<arch> and the setup as windows-<arch>-setup.
param(
    [Parameter(Mandatory)] [string] $Out,
    [ValidateSet('release', 'dev')] [string] $Profile = 'release',
    [ValidateSet('', 'x64', 'arm64')] [string] $Arch = '',
    [string] $Cargo = 'cargo',
    [string] $Payload = ''
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

# The pages one launch of the just-built app uses, as the app lists them itself
# (`apps/desktop/src/workspace/lifecycle/launch/image_pages.rs`): shipped beside the exe as
# `launch-pages.bin`, for a build's first launch on a profile, which has no list of its own yet
# (after an install, and after every update). Returns the list's path, or nothing with a warning:
# a build never fails for it, it ships without the list and its first launch is as before.
#
# The app is started once, as `opencode-desktop --record-launch-pages <file>`
# (`apps/desktop/src/workspace/lifecycle/launch/record.rs`): a launch in a root of its own that
# opens the built-in demo session, makes no request of any kind (no server, no update check, no
# browser engine download), writes the list and ends by itself. This step adds what only a
# caller can: a copy of the install folder to start it from, a scratch home (every profile and
# temp folder variable under it), a desktop of its own so no window shows on the build machine's,
# and the software adapter (a build machine's GPU is not a user's). Three placeholders named
# `wsl.exe`, `ssh.exe` and `opencode.exe` stand beside the copy, so nothing this launch might
# start by name is the build machine's own; the copy and the home are deleted afterwards.
function New-LaunchPageList([string] $Stage, [string] $Image, [string] $Scratch) {
    # Absolute: the app is given the list's path, and it leaves its working folder as it starts.
    $run = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath((Join-Path $Scratch 'page-list'))
    try {
        Remove-Item -Recurse -Force $run -ErrorAction SilentlyContinue
        $app = Join-Path $run 'app'
        $userHome = Join-Path $run 'home'
        foreach ($dir in $app, (Join-Path $userHome 'home\AppData\Local'), (Join-Path $userHome 'home\AppData\Roaming'), (Join-Path $userHome 'tmp')) { New-Item -ItemType Directory -Force $dir | Out-Null }
        Copy-Item (Join-Path $Stage '*') $app
        $placeholder = Join-Path $env:SystemRoot 'System32\hostname.exe'
        foreach ($name in 'wsl.exe', 'ssh.exe', 'opencode.exe') { Copy-Item $placeholder (Join-Path $app $name) }
        $exe = Join-Path $app 'opencode-desktop.exe'
        $block = [ordered]@{
            SystemRoot = $env:SystemRoot; windir = $env:SystemRoot; SystemDrive = $env:SystemDrive; PATH = "$env:SystemRoot\System32;$env:SystemRoot"
            ProgramData = $env:ProgramData; ProgramFiles = $env:ProgramFiles; CommonProgramFiles = $env:CommonProgramFiles
            COMPUTERNAME = $env:COMPUTERNAME; USERNAME = $env:USERNAME; NUMBER_OF_PROCESSORS = $env:NUMBER_OF_PROCESSORS
            HOME = "$userHome\home"; USERPROFILE = "$userHome\home"
            APPDATA = "$userHome\home\AppData\Roaming"; LOCALAPPDATA = "$userHome\home\AppData\Local"; TEMP = "$userHome\tmp"; TMP = "$userHome\tmp"
            WGPU_ADAPTER_NAME = 'Microsoft Basic Render Driver'
        }
        $environment = (($block.Keys | Sort-Object { $_.ToUpperInvariant() } | ForEach-Object { "$_=$($block[$_])" }) -join "`0") + "`0"
        if (-not ('OpenCodeRune.HiddenLaunch' -as [type])) {
            Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices; using System.Text;
namespace OpenCodeRune {
  public static class HiddenLaunch {
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)] struct STARTUPINFO { public int cb; public string lpReserved; public string lpDesktop; public string lpTitle; public int dwX, dwY, dwXSize, dwYSize, dwXCountChars, dwYCountChars, dwFillAttribute, dwFlags; public short wShowWindow, cbReserved2; public IntPtr lpReserved2, hStdInput, hStdOutput, hStdError; }
    [StructLayout(LayoutKind.Sequential)] struct PROCESS_INFORMATION { public IntPtr hProcess, hThread; public int dwProcessId, dwThreadId; }
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)] static extern IntPtr CreateDesktopW(string name, IntPtr device, IntPtr devmode, int flags, uint access, IntPtr security);
    [DllImport("user32.dll")] static extern bool CloseDesktop(IntPtr desktop);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)] static extern bool CreateProcessW(string app, StringBuilder command, IntPtr process, IntPtr thread, bool inherit, uint flags, IntPtr environment, string directory, ref STARTUPINFO startup, out PROCESS_INFORMATION made);
    [DllImport("kernel32.dll")] static extern uint WaitForSingleObject(IntPtr handle, uint ms);
    [DllImport("kernel32.dll")] static extern bool TerminateProcess(IntPtr handle, uint code);
    [DllImport("kernel32.dll")] static extern bool GetExitCodeProcess(IntPtr handle, out uint code);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    // The process's exit code; -1: no desktop of its own could be made (nothing started); -2: the
    // process could not be made; -3: still running after the limit, ended here.
    public static long Run(string exe, string command, string directory, string environment, string desktop, uint limitMs) {
      IntPtr desk = CreateDesktopW(desktop, IntPtr.Zero, IntPtr.Zero, 0, 0x10000000, IntPtr.Zero);
      if (desk == IntPtr.Zero) return -1;
      var startup = new STARTUPINFO(); startup.cb = Marshal.SizeOf(typeof(STARTUPINFO)); startup.lpDesktop = desktop;
      PROCESS_INFORMATION made; IntPtr block = Marshal.StringToHGlobalUni(environment);
      // CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW
      bool ok = CreateProcessW(exe, new StringBuilder(command), IntPtr.Zero, IntPtr.Zero, false, 0x400 | 0x08000000, block, directory, ref startup, out made);
      Marshal.FreeHGlobal(block);
      if (!ok) { CloseDesktop(desk); return -2; }
      long result;
      if (WaitForSingleObject(made.hProcess, limitMs) != 0) { TerminateProcess(made.hProcess, 1); WaitForSingleObject(made.hProcess, 5000); result = -3; }
      else { uint code; GetExitCodeProcess(made.hProcess, out code); result = code; }
      CloseHandle(made.hThread); CloseHandle(made.hProcess); CloseDesktop(desk);
      return result;
    }
  }
}
'@
        }
        $made = Join-Path $run 'launch-pages.bin'
        $command = "`"$exe`" --record-launch-pages `"$made`""
        $clock = [Diagnostics.Stopwatch]::StartNew()
        $code = [OpenCodeRune.HiddenLaunch]::Run($exe, $command, $app, $environment, "opencode-rune-package-$PID", 90000)
        $took = '{0:n1} s' -f $clock.Elapsed.TotalSeconds
        # The app's own codes (record.rs): 2 it did not start, 3 it quit with no list, 4 it ran past its own limit.
        if ($code -ne 0) { throw "the launch did not end well ($(switch ($code) { -1 { 'no desktop of its own could be made' } -2 { 'the process could not be made' } -3 { 'still running after 90 s' } 2 { 'exit code 2: the app refused to start the run' } 3 { 'exit code 3: the app quit with no list written' } 4 { 'exit code 4: the app was still running after its own 45 s' } default { "exit code $code" } }), after $took)" }
        if (-not (Test-Path -LiteralPath $made)) { throw "the launch wrote no list ($took)" }
        # The app's own root for the run is under the scratch home's temp folder, and the app removes it.
        $kept = @(Get-ChildItem -LiteralPath (Join-Path $userHome 'tmp') -Directory -Filter 'opencode-rune-pages-*' -ErrorAction SilentlyContinue).Count
        # The list must name the image it ships beside: `RLP1`, the image's link time stamp and its
        # size (which signing leaves as they are), the count of runs, then (first page, pages) each.
        $bytes = [IO.File]::ReadAllBytes($made)
        $file = [IO.File]::OpenRead((Join-Path $Stage $Image))
        try {
            $reader = [IO.BinaryReader]::new($file)
            $file.Position = 0x3c; $pe = $reader.ReadInt32()
            $file.Position = $pe + 8; $stamp = $reader.ReadUInt32()
            $file.Position = $pe + 24 + 56; $size = $reader.ReadUInt32()
        } finally { $file.Dispose() }
        $runs = if ($bytes.Length -ge 16) { [BitConverter]::ToUInt32($bytes, 12) } else { 0 }
        if ($bytes.Length -lt 24 -or [Text.Encoding]::ASCII.GetString($bytes, 0, 4) -ne 'RLP1' -or [BitConverter]::ToUInt32($bytes, 4) -ne $stamp -or [BitConverter]::ToUInt32($bytes, 8) -ne $size -or $bytes.Length -ne 16 + $runs * 8) {
            throw "the list the launch wrote is not this build's ($($bytes.Length) bytes)"
        }
        $pages = 0
        for ($i = 0; $i -lt $runs; $i++) { $pages += [BitConverter]::ToUInt32($bytes, 20 + $i * 8) }
        $list = Join-Path $Scratch 'launch-pages.bin'
        [IO.File]::WriteAllBytes($list, $bytes)
        Write-Host ('page list: {0:n0} runs, {1:n0} pages ({2:n1} MiB) of {3}, from one launch ({4}; its own root {5})' -f $runs, $pages, ($pages * 4096 / 1MB), $Image, $took, $(if ($kept) { 'was NOT removed by the app' } else { 'removed by the app' }))
        return $list
    } catch {
        $why = "$_" -replace '\s+', ' '
        Write-Host "::warning::The launch's page list was NOT made: $why. This build ships without launch-pages.bin."
        return $null
    } finally {
        Remove-Item -Recurse -Force $run -ErrorAction SilentlyContinue
    }
}

if ($Payload) {
    $zip = (Resolve-Path $Payload).Path
} else {
    # A release app is the shipped build: its profile-guided optimisation profile comes with it.
    if ($Profile -eq 'release') {
        # A pull request from a branch with "symbols" in its name (or OPENCODE_RUNE_SYMBOLS=1) builds
        # the shipped code with line tables and leaves its PDB in -Out, for profilers that name the
        # source (perf/README.md). Only the debug directory of the exe differs from a build without:
        # release.ps1 asks for the line tables, in a way that keeps every symbol's name.
        $symbols = $env:OPENCODE_RUNE_SYMBOLS -eq '1' -or $env:GITHUB_HEAD_REF -match 'symbols'
        # The shipped app is two files: a thin opencode-desktop.exe (apps/desktop-thin) that loads the
        # app from opencode-desktop.dll beside it (apps/desktop-library: the same code, allocator and
        # profile as the one-file exe). Windows Defender checks a new exe before its first process
        # exists at 30-36 ms per MiB of the exe, and does not check a library that way
        # (apps/desktop-thin/src/main.rs has the numbers). A build that records a profile
        # (release.ps1's `pgo-train/` branches) stays one file, as its training run drives it; so does a
        # pull request from a branch with "one-file" in its name (or OPENCODE_RUNE_ONE_FILE=1): the same
        # code in one exe, to time against.
        $thin = -not ($env:OPENCODE_RUNE_PGO_GENERATE -eq '1' -or $env:GITHUB_HEAD_REF -match '^pgo-train/' -or $env:OPENCODE_RUNE_ONE_FILE -eq '1' -or $env:GITHUB_HEAD_REF -match 'one-file')
        & (Join-Path $root 'apps/desktop/scripts/release.ps1') -Cargo $Cargo -Target $triple -Package $(if ($thin) { 'opencode-desktop-library' } else { 'opencode-desktop' })
        if ($LASTEXITCODE -ne 0) { throw 'the release build failed' }
        if ($thin) { Invoke-Cargo @('build', '--release', '-p', 'opencode-desktop-thin', '--target', $triple) }
        # release.ps1 always names the target, so the app is under target/<triple>/ even when native.
        $appBin = if ($cross) { $bin } else { Join-Path $target "$triple/release" }
    } else {
        $thin = $false
        Invoke-Cargo (@('build', '-p', 'opencode-desktop') + $flags)
        $appBin = $bin
    }

    # The install folder: the app and its toasts' icon, which the setup registers from there
    # (`win_install::register`). The browser engine downloads in the background.
    $stage = Join-Path $Out 'stage'
    Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
    New-Item -ItemType Directory $stage | Out-Null
    $exe = Join-Path $stage 'opencode-desktop.exe'
    if ($thin) {
        Copy-Item (Join-Path $appBin 'opencode-desktop-thin.exe') $exe
        Copy-Item (Join-Path $appBin 'opencode_desktop_app.dll') (Join-Path $stage 'opencode-desktop.dll')
    } else {
        Copy-Item (Join-Path $appBin 'opencode-desktop.exe') $exe
    }
    if (-not $cross) {
        $reported = (& $exe --version | Out-String).Trim()
        if ($reported -ne "opencode-rune $version ($channel)") { throw "opencode-desktop --version reported '$reported', expected 'opencode-rune $version ($channel)'" }
    }
    # The launch's page list, for the shipped build this machine can run. Not for a build that
    # records a profile (its counts are the training run's alone), nor a cross-build (its app
    # can't start here: it ships without a list, as every build did before).
    # OPENCODE_RUNE_PAGE_LIST=off leaves the step out.
    $training = $env:OPENCODE_RUNE_PGO_GENERATE -eq '1' -or $env:GITHUB_HEAD_REF -match '^pgo-train/'
    if ($cross -or $Profile -ne 'release' -or $training -or $env:OPENCODE_RUNE_PAGE_LIST -eq 'off') {
        Write-Host "page list: not made for this build ($(if ($cross) { "a cross-build for $Arch" } elseif ($Profile -ne 'release') { "profile $Profile" } elseif ($training) { 'a profile-recording build' } else { 'OPENCODE_RUNE_PAGE_LIST=off' }))"
    } else {
        $list = New-LaunchPageList -Stage $stage -Image $(if ($thin) { 'opencode-desktop.dll' } else { 'opencode-desktop.exe' }) -Scratch $Out
        if ($list) { Move-Item -Force $list (Join-Path $stage 'launch-pages.bin') }
    }
    Copy-Item (Join-Path $root 'apps/desktop/assets/icon/art-512.png') (Join-Path $stage 'notification-icon.png')
    Copy-Item (Join-Path $root 'LICENSE') (Join-Path $stage 'LICENSE')
    Copy-Item (Join-Path $root 'THIRD_PARTY_NOTICES.md') (Join-Path $stage 'THIRD_PARTY_NOTICES.md')
    $zip = Join-Path $Out "$stem.zip"
    Remove-Item -Force $zip -ErrorAction SilentlyContinue
    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
    Remove-Item -Recurse -Force $stage
    # The PDB keeps the name its image looks for beside itself: the library's, which holds the app.
    if ($Profile -eq 'release' -and $symbols) { Copy-Item (Join-Path $appBin $(if ($thin) { 'opencode_desktop_app.pdb' } else { 'opencode_desktop.pdb' })) $Out }
}

$env:OPENCODE_RUNE_SETUP_PAYLOAD = $zip
try {
    Invoke-Cargo (@('build', '-p', 'opencode-rune-setup') + $flags)
} finally {
    Remove-Item Env:OPENCODE_RUNE_SETUP_PAYLOAD
}
$setup = Join-Path $Out "$stem-setup.exe"
Copy-Item (Join-Path $bin 'opencode-rune-setup.exe') $setup -Force

foreach ($file in @(if (-not $Payload) { $zip }) + $setup) {
    $hash = (Get-FileHash -Algorithm SHA256 $file).Hash.ToLowerInvariant()
    '{0}  {1}  {2:N0} bytes' -f $hash, (Split-Path -Leaf $file), (Get-Item $file).Length
}
