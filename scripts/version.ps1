# Updates the version in package.json, tauri.conf.json, all Cargo.toml version
# fields and Cargo.lock. Does not commit or push. With -Tag it also creates the
# annotated vX.Y.Z tag locally.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$Version,

    [switch]$Tag
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

# JSON files: let Node rewrite them so key order and 2-space formatting stay stable.
$env:RELAY_VERSION = $v
$nodeScript = @'
const fs = require("fs");
const version = process.env.RELAY_VERSION;
for (const file of ["package.json", "src-tauri/tauri.conf.json"]) {
  const json = JSON.parse(fs.readFileSync(file, "utf8"));
  json.version = version;
  fs.writeFileSync(file, JSON.stringify(json, null, 2) + "\n");
  console.log(`  ${file} -> ${version}`);
}
'@
$tmp = Join-Path $env:TEMP ("relay-version-" + [guid]::NewGuid().ToString("N") + ".cjs")
try {
    [System.IO.File]::WriteAllText($tmp, $nodeScript, [System.Text.UTF8Encoding]::new($false))
    node $tmp
    if ($LASTEXITCODE -ne 0) {
        throw "Node failed to update the JSON versions (exit $LASTEXITCODE)."
    }
} finally {
    Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
}

# Cargo.toml: only touch the `version` line inside the named table.
# Returns "changed", "unchanged", "missing" (optional file/table) or
# "no-version-line".
function Set-CargoTableVersion {
    param(
        [string]$File,
        [string]$Table,
        [string]$NewVersion,
        [switch]$Optional
    )

    $path = Join-Path $root $File
    if (-not (Test-Path -LiteralPath $path)) {
        if ($Optional) { return "missing" }
        throw "Missing $File"
    }
    $header = "[$Table]"
    $lines = [System.IO.File]::ReadAllLines($path)
    $inTable = $false
    $done = $false
    $changed = $false
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $line = $lines[$i].Trim()
        if ($line -match '^\[.*\]$') {
            $inTable = ($line -eq $header)
        } elseif ($inTable -and -not $done -and $line -match '^version\s*=\s*"([^"]+)"') {
            $done = $true
            if ($Matches[1] -ne $NewVersion) {
                $lines[$i] = "version = `"$NewVersion`""
                $changed = $true
            }
        }
    }
    if (-not $done) {
        return "no-version-line"
    }
    [System.IO.File]::WriteAllText($path, (($lines -join "`n") + "`n"), [System.Text.UTF8Encoding]::new($false))
    if ($changed) { return "changed" }
    return "unchanged"
}

$cargoChanged = $false
foreach ($file in @("src-tauri/Cargo.toml", "crates/relay-core/Cargo.toml")) {
    $result = Set-CargoTableVersion -File $file -Table "package" -NewVersion $v
    switch ($result) {
        "changed" {
            $cargoChanged = $true
            Write-Host "  $file [package] -> $v"
        }
        "unchanged" {
            Write-Host "  $file [package] already $v"
        }
        default {
            throw "No version line in the [package] table of $file"
        }
    }
}

# [workspace.package].version is optional: a workspace may not declare one.
$workspaceResult = Set-CargoTableVersion -File "Cargo.toml" -Table "workspace.package" -NewVersion $v -Optional
switch ($workspaceResult) {
    "changed" {
        $cargoChanged = $true
        Write-Host "  Cargo.toml [workspace.package] -> $v"
    }
    "unchanged" {
        Write-Host "  Cargo.toml [workspace.package] already $v"
    }
    "missing" {
        Write-Host "  Cargo.toml has no [workspace.package] table; skipped."
    }
    default {
        Write-Warning "Cargo.toml [workspace.package] has no version line; skipped (members may inherit it)."
    }
}

# Cargo.lock records the local package versions. Refresh it offline so a version
# bump does not need the network and does not leave the tree dirty after a build.
if ($cargoChanged) {
    Write-Host "Refreshing Cargo.lock (offline, no network)..."
    $cargoLog = Join-Path $env:TEMP ("relay-cargo-metadata-" + [guid]::NewGuid().ToString("N") + ".log")
    try {
        cargo metadata --offline --format-version 1 > $null 2> $cargoLog
        $cargoExit = $LASTEXITCODE
        if ($cargoExit -ne 0) {
            Write-Warning "cargo metadata --offline failed (exit $cargoExit). No network was used; Cargo may still have rewritten the local package entries in Cargo.lock, so check 'git diff Cargo.lock' before committing. The next build will complete the lock."
            $logText = (Get-Content -LiteralPath $cargoLog -Raw -ErrorAction SilentlyContinue)
            if ($logText) {
                $excerpt = (($logText -split "`r?`n") | Select-Object -First 8) -join "`n"
                Write-Host $excerpt
            }
        } else {
            Write-Host "  Cargo.lock refreshed."
        }
    } finally {
        Remove-Item -LiteralPath $cargoLog -Force -ErrorAction SilentlyContinue
    }
} else {
    Write-Host "Cargo versions unchanged; Cargo.lock not touched."
}

Write-Host "Version set to $v."

if ($Tag) {
    $tag = "v$v"
    $existing = git tag --list $tag
    if ($existing) {
        throw "Tag $tag already exists."
    }
    $dirty = git status --porcelain
    if ($dirty) {
        throw "The working tree has uncommitted changes. The tag would point at HEAD, not at the version bump. Commit the bump first, then tag."
    }
    git tag -a $tag -m "Relay $tag"
    if ($LASTEXITCODE -ne 0) {
        throw "git tag failed (exit $LASTEXITCODE)."
    }
    Write-Host "Created $tag locally. Push it when the version commit is ready: git push origin $tag"
} else {
    Write-Host "No tag created. Re-run with -Tag to create v$v, or tag manually."
}
