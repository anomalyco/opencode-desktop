<#
.SYNOPSIS
  Runs all cheap repository guardrails before pushing.

.DESCRIPTION
  Runs arch-check, file-size, folder-shape, dialog-size, and tables-check using
  Bun (preferred) or Node.js >= 18, and exits non-zero if any check fails or if
  neither runtime is installed.

  Slow compile/lint checks remain separate:
    - scripts/clippy.sh
    - cargo check -p session-view --examples
    - pwsh scripts/web.ps1 -Check

.EXAMPLE
  pwsh scripts/guardrails.ps1
#>
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$script = Join-Path $PSScriptRoot 'guardrails/run.mjs'

$js = (Get-Command bun -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1).Source
if ($js) {
  & $js --version *> $null
  if ($LASTEXITCODE -ne 0) { $js = $null }
}
if (-not $js) {
  $node = (Get-Command node -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1).Source
  if ($node) {
    $ver = (& $node --version 2>$null)
    if ($LASTEXITCODE -eq 0 -and $ver -match '^v(\d+)' -and [int]$Matches[1] -ge 18) {
      $js = $node
    }
  }
}
if (-not $js) {
  Write-Error 'guardrails require bun or node >= 18 on PATH (neither working runtime was found)'
  exit 127
}

Push-Location $root
try {
  & $js $script @Args
  exit $LASTEXITCODE
} finally {
  Pop-Location
}
