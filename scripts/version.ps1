# Updates the version in package.json, tauri.conf.json and both Cargo.toml files.
# Does not commit or push. With -Tag it also creates the annotated vX.Y.Z tag locally.
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

# Cargo.toml: only touch the `version` line inside [package].
foreach ($file in @("src-tauri/Cargo.toml", "crates/relay-core/Cargo.toml")) {
    $path = Join-Path $root $file
    $lines = [System.IO.File]::ReadAllLines($path)
    $inPackage = $false
    $done = $false
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $line = $lines[$i].Trim()
        if ($line -match '^\[.*\]$') {
            $inPackage = ($line -eq "[package]")
        } elseif ($inPackage -and -not $done -and $line -match '^version\s*=') {
            $lines[$i] = "version = `"$v`""
            $done = $true
        }
    }
    if (-not $done) {
        throw "No version line in the [package] table of $file"
    }
    [System.IO.File]::WriteAllText($path, (($lines -join "`n") + "`n"), [System.Text.UTF8Encoding]::new($false))
    Write-Host "  $file -> $v"
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
        Write-Warning "The working tree has uncommitted changes. The tag will point at HEAD, not at the version bump. Commit the bump first if you want the tag to contain it."
    }
    git tag -a $tag -m "Relay $tag"
    if ($LASTEXITCODE -ne 0) {
        throw "git tag failed (exit $LASTEXITCODE)."
    }
    Write-Host "Created $tag locally. Push it when the version commit is ready: git push origin $tag"
} else {
    Write-Host "No tag created. Re-run with -Tag to create v$v, or tag manually."
}
