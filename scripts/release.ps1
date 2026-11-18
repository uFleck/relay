# Fast local release path: preflight, warm target/, pnpm tauri build, sign, publish.
# The signing key is read from TAURI_SIGNING_PRIVATE_KEY_PATH or %USERPROFILE%\.relay\signing.key.
# This script never generates or prints a key.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$Version,

    [switch]$SkipInstall,
    [switch]$DryRun,
    [switch]$ForceOverBudget
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

# A prerelease suffix (a '-' before any '+build') must not become GitHub's
# "latest": the updater endpoint is /releases/latest, which excludes prereleases.
$core = if ($v.Contains("+")) { $v.Substring(0, $v.IndexOf("+")) } else { $v }
$isPrerelease = $core.Contains("-")
$releaseTitle = if ($isPrerelease) { "Relay $tag (pre-release)" } else { "Relay $tag" }

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

# Preflight failures stop a real run. -DryRun only reports them, so the planned
# steps can still be inspected (and nothing is built, written or uploaded).
function Fail([string]$Message) {
    if ($DryRun) {
        Write-Warning $Message
    } else {
        throw $Message
    }
}

function Get-CargoTableVersion {
    param(
        [string]$Path,
        [string]$Table
    )
    if (-not (Test-Path -LiteralPath $Path)) {
        return $null
    }
    $header = "[$Table]"
    $inTable = $false
    foreach ($line in [System.IO.File]::ReadAllLines($Path)) {
        $trimmed = $line.Trim()
        if ($trimmed -match '^\[.*\]$') {
            $inTable = ($trimmed -eq $header)
            continue
        }
        if ($inTable -and $trimmed -match '^version\s*=\s*"([^"]+)"') {
            return $Matches[1]
        }
    }
    return $null
}

function Resolve-GitHubRepo {
    if ($env:GH_REPO) {
        return $env:GH_REPO.Trim()
    }
    $url = git remote get-url origin 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $url) {
        return $null
    }
    $url = $url.Trim()
    if ($url -match 'github\.com[:/](?<owner>[^/]+)/(?<repo>[^/]+?)(\.git)?/?$') {
        return "$($Matches['owner'])/$($Matches['repo'])"
    }
    if ($url -match '^(?<owner>[^/]+)/(?<repo>[^/]+?)(\.git)?$') {
        return "$($Matches['owner'])/$($Matches['repo'])"
    }
    return $null
}

Write-Host "Relay release $tag"
if ($isPrerelease) {
    Write-Host "Prerelease: GitHub gets --prerelease; /releases/latest stays on the newest stable release."
}
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

# Every version field the build reads must equal the release argument. A partial
# bump would build one version and publish latest.json for another.
$versionChecks = @(
    [pscustomobject]@{ Field = "package.json"; Optional = $false; Value = (Get-Content -LiteralPath "package.json" -Raw | ConvertFrom-Json).version },
    [pscustomobject]@{ Field = "src-tauri/tauri.conf.json"; Optional = $false; Value = (Get-Content -LiteralPath "src-tauri/tauri.conf.json" -Raw | ConvertFrom-Json).version },
    [pscustomobject]@{ Field = "src-tauri/Cargo.toml [package]"; Optional = $false; Value = (Get-CargoTableVersion "src-tauri/Cargo.toml" "package") },
    [pscustomobject]@{ Field = "crates/relay-core/Cargo.toml [package]"; Optional = $false; Value = (Get-CargoTableVersion "crates/relay-core/Cargo.toml" "package") },
    [pscustomobject]@{ Field = "Cargo.toml [workspace.package]"; Optional = $true; Value = (Get-CargoTableVersion "Cargo.toml" "workspace.package") }
)

$mismatches = [System.Collections.Generic.List[string]]::new()
$checkedFields = 0
foreach ($check in $versionChecks) {
    if ([string]::IsNullOrWhiteSpace($check.Value)) {
        if ($check.Optional) {
            continue
        }
        Fail "Cannot read a version from $($check.Field)."
        continue
    }
    $checkedFields++
    if ($check.Value -ne $v) {
        $mismatches.Add("$($check.Field) is $($check.Value), expected $v")
    }
}
if ($mismatches.Count -gt 0) {
    Fail ("Version mismatch for {0}: {1}. Run scripts/version.ps1 -Version {2}, commit the bump, then move the tag." -f $tag, ($mismatches -join "; "), $v)
} else {
    Write-Host "Version fields match $tag ($checkedFields checked)."
}

if (-not (git tag --list $tag)) {
    if ($DryRun) {
        Write-Warning "Tag $tag does not exist."
    } else {
        throw "Tag $tag does not exist. Run scripts/version.ps1 -Version $v -Tag first."
    }
} else {
    # The installer must be built from the commit the tag names, or the release
    # would advertise one version for a different tree.
    $tagCommit = (git rev-list -n 1 $tag 2>$null | Out-String).Trim()
    $headCommit = (git rev-parse HEAD 2>$null | Out-String).Trim()
    if ($tagCommit -and $headCommit -and $tagCommit -ne $headCommit) {
        if ($DryRun) {
            Write-Warning "Tag $tag points at $tagCommit, not HEAD $headCommit."
        } else {
            throw "Tag $tag points at $tagCommit, not HEAD $headCommit. Check out the tagged commit or move the tag before releasing."
        }
    }
}

# gh is required: without it the existing-release probe cannot run, and the
# publish would fail after a full build.
$gh = Get-Command gh -ErrorAction SilentlyContinue
if (-not $gh) {
    if ($DryRun) {
        Write-Warning "gh (GitHub CLI) is not on PATH; a real run would stop here."
    } else {
        throw "gh (GitHub CLI) is not on PATH. Install it, run 'gh auth login', then retry. The release was not built."
    }
} else {
    $repo = Resolve-GitHubRepo
    if (-not $repo) {
        if ($DryRun) {
            Write-Warning "Cannot resolve the GitHub repository: no 'origin' remote and GH_REPO is not set. A real run would stop here."
        } else {
            throw "Cannot resolve the GitHub repository: no 'origin' remote and GH_REPO is not set. Add origin (git remote add origin <url>) or set GH_REPO=owner/repo, then retry."
        }
    } else {
        gh auth status 2>&1 | Out-Null
        $authExit = $LASTEXITCODE
        # Read-only probe. gh prints "release not found" when the tag has no release.
        $releaseOutput = (gh release view $tag --repo $repo 2>&1 | Out-String).Trim()
        $releaseExit = $LASTEXITCODE

        if ($releaseExit -eq 0) {
            Fail "Release $tag already exists on $repo. Refusing to clobber it: delete that release or release a new version."
        } elseif ($releaseOutput -match '(?i)release not found') {
            Write-Host "No existing release for $tag on $repo."
            if ($authExit -ne 0) {
                # Not fatal: the probe above proved the active account can reach the repo.
                Write-Warning "gh auth status exits $authExit (often an invalid secondary account); the release probe worked, so the active account is usable. Run 'gh auth status' to inspect."
            }
        } else {
            $message = "Could not confirm that release $tag is absent on $repo (gh release view exit $releaseExit):`n$releaseOutput"
            if ($authExit -ne 0) {
                $message += "`ngh auth status exits $authExit; run 'gh auth login'."
            }
            Fail $message
        }
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

if ($DryRun) {
    Write-Host ""
    Write-Host "Would run:"
    if (-not $SkipInstall) {
        Write-Host "  pnpm install --frozen-lockfile"
    }
    Write-Host "  pnpm tauri build"
    Write-Host "  check budget gate: warn above 300s, fail above 420s before upload unless -ForceOverBudget (upload excluded)"
    Write-Host "  require <product>_${v}_<arch>-setup.exe and its .sig (no newest-.exe fallback)"
    Write-Host "  write latest.json for $tag"
    $plannedGh = "gh release create $tag <installer>.exe <installer>.exe.sig latest.json --verify-tag --title `"$releaseTitle`" --generate-notes"
    if ($isPrerelease) {
        $plannedGh += " --prerelease"
    }
    Write-Host "  $plannedGh"
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

# Budget gate: measured at the end of install + build, before anything is
# published. Upload time is deliberately excluded, so a slow upload can no
# longer turn a published release red.
$buildSeconds = $total.Elapsed.TotalSeconds
Write-Host ""
Write-Host ("Budget gate: {0:N1}s at the end of the build (install + build; upload excluded)." -f $buildSeconds)
if ($buildSeconds -gt 300) {
    Write-Warning ("Build path is over the 5:00 target ({0:N1}s); the hard limit is 420s." -f $buildSeconds)
}
if ($buildSeconds -gt 420) {
    if ($ForceOverBudget) {
        Write-Warning ("Build path is over the 420s budget ({0:N1}s); continuing because -ForceOverBudget was passed." -f $buildSeconds)
    } else {
        throw ("Build path took {0:N1}s, over the 420s budget (upload excluded). Nothing was published. Re-run with -ForceOverBudget to publish anyway." -f $buildSeconds)
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
    $candidates = @(Get-ChildItem -LiteralPath $nsisDir -Filter "*.exe")
    if ($candidates.Count -eq 0) {
        throw "No installer .exe in $nsisDir"
    }
    # Exact version required: a stale or partially built bundle for another
    # version must not be uploaded under this tag.
    $versioned = @($candidates | Where-Object { $_.Name -like "*_${v}_*-setup.exe" })
    if ($versioned.Count -eq 0) {
        $found = ($candidates | ForEach-Object { $_.Name }) -join ", "
        throw "No NSIS installer for $v in $nsisDir (expected <product>_${v}_<arch>-setup.exe). Found: $found. Re-run pnpm tauri build."
    }
    if ($versioned.Count -gt 1) {
        $found = ($versioned | ForEach-Object { $_.Name }) -join ", "
        throw "More than one NSIS installer for $v in ${nsisDir}: $found. Remove the stale copies and rebuild."
    }
    $script:installer = $versioned[0]
    $script:signature = "$($script:installer.FullName).sig"
    if (-not (Test-Path -LiteralPath $script:signature)) {
        throw "Missing updater signature: $($script:signature)"
    }
    Write-Host "  $($script:installer.FullName)"
    Write-Host "  $($script:signature)"
}

$latestPath = Join-Path $root "latest.json"
Step "write latest.json" {
    $releaseRepo = if ($env:GH_REPO) { $env:GH_REPO.Trim() } else { Resolve-GitHubRepo }
    if (-not $releaseRepo) {
        throw "Cannot write latest.json: no GitHub repository (add origin or set GH_REPO)."
    }
    $url = "https://github.com/$releaseRepo/releases/download/$tag/$([uri]::EscapeDataString($script:installer.Name))"
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
    if ($isPrerelease) {
        Write-Host "  prerelease: latest.json is still attached; the updater's /releases/latest stays on stable."
    }
}

Step "gh release create $tag" {
    $ghArgs = @(
        "release", "create", $tag,
        $script:installer.FullName,
        $script:signature,
        $latestPath,
        "--verify-tag",
        "--title", $releaseTitle,
        "--generate-notes"
    )
    if ($isPrerelease) {
        $ghArgs += "--prerelease"
    }
    gh @ghArgs
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
Write-Host "Total incl. upload: $totalText"
Write-Host ("Budget gate measured {0:N1}s before upload; the upload time is excluded from the 420s gate." -f $buildSeconds)
