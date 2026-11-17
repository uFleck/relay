# Fast local release path: warm target/, pnpm tauri build, sign, publish.
# The signing key is read from TAURI_SIGNING_PRIVATE_KEY_PATH or %USERPROFILE%\.relay\signing.key.
# This script never generates or prints a key.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$Version,

    [switch]$SkipInstall,
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$v = $Version.Trim()
if ($v.StartsWith("v")) {
    $v = $v.Substring(1)
}
if ($v -notmatch '^\d+\.\d+\.\d+([-+][0-9A-Za-z.-]+)?$') {
    throw "Version '$Version' is not semver (expected X.Y.Z, optionally -pre or +build)."
}
$tag = "v$v"

$total = [System.Diagnostics.Stopwatch]::StartNew()
$timings = [System.Collections.Generic.List[object]]::new()

function Format-Elapsed([TimeSpan]$Span) {
    return $Span.ToString("mm\:ss")
}

function Step {
    param(
        [string]$Name,
        [scriptblock]$Body
    )
    Write-Host ""
    Write-Host ("[{0}] {1}" -f (Format-Elapsed $total.Elapsed), $Name)
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    try {
        & $Body
    } finally {
        $sw.Stop()
        $script:timings.Add([pscustomobject]@{ Name = $Name; Seconds = $sw.Elapsed.TotalSeconds })
        Write-Host ("       done in {0:N1}s" -f $sw.Elapsed.TotalSeconds)
    }
}

Write-Host "Relay release $tag"
if ($DryRun) {
    Write-Host "DRY RUN: nothing is built, written or uploaded."
}

# --- Preconditions -----------------------------------------------------------

$status = git status --porcelain
if ($status) {
    if ($DryRun) {
        Write-Warning "Working tree is not clean; a real run would stop here."
    } else {
        throw "Working tree is not clean. Commit or stash first:`n$($status -join "`n")"
    }
}

$packageVersion = (Get-Content -LiteralPath package.json -Raw | ConvertFrom-Json).version
if ($packageVersion -ne $v) {
    if ($DryRun) {
        Write-Warning "package.json is $packageVersion, but the release argument is $v."
    } else {
        throw "package.json is $packageVersion, not $v. Run scripts/version.ps1 -Version $v -Tag first."
    }
}

if (-not (git tag --list $tag)) {
    if ($DryRun) {
        Write-Warning "Tag $tag does not exist."
    } else {
        throw "Tag $tag does not exist. Run scripts/version.ps1 -Version $v -Tag first."
    }
}

$keyPath = if ($env:TAURI_SIGNING_PRIVATE_KEY_PATH) {
    $env:TAURI_SIGNING_PRIVATE_KEY_PATH
} else {
    Join-Path $env:USERPROFILE ".relay\signing.key"
}
$keyPath = [Environment]::ExpandEnvironmentVariables($keyPath)
$passwordPath = "$keyPath.password"

if (-not (Test-Path -LiteralPath $keyPath)) {
    if ($DryRun) {
        Write-Warning "Signing key not found at $keyPath."
    } else {
        throw "Signing key not found at $keyPath. Run scripts/generate-signing-key.ps1 or set TAURI_SIGNING_PRIVATE_KEY_PATH."
    }
} else {
    # The CLI reads these; neither value is printed.
    if (-not $env:TAURI_SIGNING_PRIVATE_KEY) {
        $env:TAURI_SIGNING_PRIVATE_KEY = (Get-Content -LiteralPath $keyPath -Raw).Trim()
    }
    if (-not $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD -and (Test-Path -LiteralPath $passwordPath)) {
        $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = (Get-Content -LiteralPath $passwordPath -Raw).Trim()
    }
}

if (-not $DryRun) {
    $ghStatus = gh auth status 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) {
        throw "gh is not authenticated. Run 'gh auth login'. Output:`n$ghStatus"
    }
}

if ($DryRun) {
    Write-Host ""
    Write-Host "Would run:"
    if (-not $SkipInstall) {
        Write-Host "  pnpm install --frozen-lockfile"
    }
    Write-Host "  pnpm tauri build"
    Write-Host "  write latest.json for $tag"
    Write-Host "  gh release create $tag <installer>.exe <installer>.exe.sig latest.json --title `"Relay $tag`" --generate-notes"
    $total.Stop()
    Write-Host ("Total: {0:N1}s" -f $total.Elapsed.TotalSeconds)
    return
}

# --- Build and publish -------------------------------------------------------

if ($SkipInstall) {
    Write-Host "Skipping pnpm install (-SkipInstall)."
} else {
    Step "pnpm install --frozen-lockfile" {
        pnpm install --frozen-lockfile
        if ($LASTEXITCODE -ne 0) {
            throw "pnpm install failed (exit $LASTEXITCODE)."
        }
    }
}

Step "pnpm tauri build" {
    pnpm tauri build
    if ($LASTEXITCODE -ne 0) {
        throw "pnpm tauri build failed (exit $LASTEXITCODE)."
    }
}

$script:installer = $null
$script:signature = $null
Step "find NSIS installer and signature" {
    $cargoTarget = if ($env:CARGO_TARGET_DIR) {
        $env:CARGO_TARGET_DIR
    } else {
        Join-Path $root "target"
    }
    $nsisDir = Join-Path $cargoTarget "release\bundle\nsis"
    if (-not (Test-Path -LiteralPath $nsisDir)) {
        throw "No NSIS bundle at $nsisDir"
    }
    $candidates = Get-ChildItem -LiteralPath $nsisDir -Filter "*.exe" | Sort-Object LastWriteTime -Descending
    $script:installer = $candidates | Where-Object { $_.Name -like "*$v*" } | Select-Object -First 1
    if (-not $script:installer) {
        $script:installer = $candidates | Select-Object -First 1
    }
    if (-not $script:installer) {
        throw "No installer .exe in $nsisDir"
    }
    $script:signature = "$($script:installer.FullName).sig"
    if (-not (Test-Path -LiteralPath $script:signature)) {
        throw "Missing updater signature: $($script:signature)"
    }
    Write-Host "  $($script:installer.FullName)"
    Write-Host "  $($script:signature)"
}

$latestPath = Join-Path $root "latest.json"
Step "write latest.json" {
    $url = "https://github.com/uFleck/relay/releases/download/$tag/$([uri]::EscapeDataString($script:installer.Name))"
    $latest = [ordered]@{
        version   = $v
        pub_date  = [DateTime]::UtcNow.ToString("yyyy-MM-ddTHH:mm:ssZ")
        platforms = [ordered]@{
            "windows-x86_64" = [ordered]@{
                signature = (Get-Content -LiteralPath $script:signature -Raw).Trim()
                url       = $url
            }
        }
    }
    $json = ($latest | ConvertTo-Json -Depth 6) + "`n"
    [System.IO.File]::WriteAllText($latestPath, $json, [System.Text.UTF8Encoding]::new($false))
    Write-Host "  $latestPath"
    Write-Host "  $url"
}

Step "gh release create $tag" {
    gh release create $tag $script:installer.FullName $script:signature $latestPath --title "Relay $tag" --generate-notes
    if ($LASTEXITCODE -ne 0) {
        throw "gh release create failed (exit $LASTEXITCODE)."
    }
}

$total.Stop()
Write-Host ""
Write-Host "Timings"
foreach ($timing in $timings) {
    Write-Host ("  {0,-26} {1,7:N1}s" -f $timing.Name, $timing.Seconds)
}
$totalText = "{0:N1}s ({1})" -f $total.Elapsed.TotalSeconds, (Format-Elapsed $total.Elapsed)
Write-Host "Total: $totalText"

if ($total.Elapsed.TotalSeconds -gt 420) {
    throw "Release took $totalText, over the 420s budget."
}
