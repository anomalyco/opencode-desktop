# Windows: builds the engine (release), assembles `OpenCode Browser Engine`, and writes
#   <out-dir>\OpenCode-Browser-Engine-<version>-windows-x64.zip
#   <out-dir>\SHA256SUMS
# The zip holds the folder at its root, as the app's installer expects. With WINDOWS_SIGN_THUMBPRINT
# set, signtool signs both executables (SHA-256, timestamped) before zipping.
#   pwsh apps/browser-engine/scripts/package-release.ps1 <out-dir>
# TODO(IMPORTANT before release): build + sign (Authenticode) the Windows engine in CI with package-release.ps1, upload to the release CDN, then set ENGINE_DEFAULT_URL/SHA256 here.
# ("here" is crates/browser-pane/src/installer.rs. The CDN path follows macOS:
# https://anoma.ly/release/opencode-rune/files/<sha256>/OpenCode-Browser-Engine-<version>-windows-x64.zip.
# Until then Windows has no download: the app skips its background download, and
# install-local.ps1 installs a local build.)
param([Parameter(Mandatory)][string]$OutDir)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'assemble-windows.ps1')

$bin = Build-Engine -Release
$work = Join-Path ([IO.Path]::GetTempPath()) "opencode-browser-engine-$PID"
Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
$app = Assemble-Engine $bin $work
if ($env:WINDOWS_SIGN_THUMBPRINT) {
  $exes = Get-ChildItem $app -Filter '*.exe' | ForEach-Object FullName
  signtool sign /sha1 $env:WINDOWS_SIGN_THUMBPRINT /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 @exes
  if ($LASTEXITCODE -ne 0) { throw 'signtool failed' }
}
New-Item -ItemType Directory -Force $OutDir | Out-Null
$name = "OpenCode-Browser-Engine-$EngineVersion-windows-x64.zip"
$zip = Join-Path (Resolve-Path $OutDir) $name
Remove-Item $zip, (Join-Path $OutDir 'SHA256SUMS') -ErrorAction SilentlyContinue
Compress-Archive -Path $app -DestinationPath $zip
$hash = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
"$hash  $name" | Set-Content -NoNewline (Join-Path $OutDir 'SHA256SUMS')
Remove-Item -Recurse -Force $work
Write-Host "$hash  $name"
Get-Item $zip | Select-Object Name, Length
