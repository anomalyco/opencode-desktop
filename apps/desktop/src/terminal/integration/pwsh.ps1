# Prompt marks for PowerShell 7 and Windows PowerShell, run by the terminal when the user turns on
# "Mark commands" for that shell: OSC 133 (A prompt, B input, C output, D;<exit> done) and the
# folder as OSC 9;9, around the profile's own prompt. Nothing is written to the profile.
if (-not $global:__ocMarks) {
  $global:__ocMarks = $true
  $global:__ocRan = $false
  $global:__ocPrompt = $function:prompt
  function global:prompt {
    $ok = $?
    $code = if ($ok) { 0 } elseif ($LASTEXITCODE) { $LASTEXITCODE } else { 1 }
    $e = [char]27; $a = [char]7
    $done = if ($global:__ocRan) { "$e]133;D;$code$a" } else { '' }
    $global:__ocRan = $false
    $done + "$e]9;9;`"$($PWD.ProviderPath)`"$a$e]133;A$a" + (& $global:__ocPrompt) + "$e]133;B$a"
  }
  if (-not (Get-Module PSReadLine)) { Import-Module PSReadLine -ErrorAction SilentlyContinue }
  if (Test-Path Function:\PSConsoleHostReadLine) {
    $global:__ocReadLine = $function:PSConsoleHostReadLine
    function global:PSConsoleHostReadLine {
      $line = & $global:__ocReadLine
      $global:__ocRan = $true
      [Console]::Write("$([char]27)]133;C$([char]7)")
      $line
    }
  }
}
