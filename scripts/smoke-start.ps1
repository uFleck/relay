# Startup smoke check. Launches a Relay build, waits for a real window, then checks that the
# process is still alive after the whole setup path has run. A panic during setup closes the
# process after the first paint, so "window appeared" alone is not enough.
[CmdletBinding()]
param(
  [string]$Exe,
  [int]$WaitSeconds = 12,
  [int]$AliveSeconds = 8
)

$ErrorActionPreference = "Stop"
if (-not $Exe) {
  $root = Split-Path -Parent $PSScriptRoot
  $Exe = Join-Path $root 'target\release\relay.exe'
}
if (-not (Test-Path -LiteralPath $Exe)) { throw "Executable not found: $Exe" }

$dir = Join-Path $env:TEMP 'relay-smoke-start'
New-Item -ItemType Directory -Force -Path $dir | Out-Null
$out = Join-Path $dir 'stdout.log'
$err = Join-Path $dir 'stderr.log'
Remove-Item $out, $err -ErrorAction SilentlyContinue

$proc = Start-Process -FilePath $Exe -PassThru -RedirectStandardOutput $out -RedirectStandardError $err
try {
  $deadline = (Get-Date).AddSeconds($WaitSeconds)
  $window = $false
  while ((Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 250
    $proc.Refresh()
    if ($proc.HasExited) { break }
    if ($proc.MainWindowHandle -ne 0) { $window = $true; break }
  }
  if (-not $window) { throw "No main window within ${WaitSeconds}s (exited=$($proc.HasExited))." }

  $deadline = (Get-Date).AddSeconds($AliveSeconds)
  while ((Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 250
    $proc.Refresh()
    if ($proc.HasExited) { break }
  }
  if ($proc.HasExited) {
    $log = if (Test-Path $err) { Get-Content $err -Raw } else { '' }
    throw "Relay exited during startup (code $($proc.ExitCode)).`n$log"
  }
  Write-Output "PASS: window appeared and process stayed alive for ${AliveSeconds}s (pid $($proc.Id))"
} finally {
  if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
}
