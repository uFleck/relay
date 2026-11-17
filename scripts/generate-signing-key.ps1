# Generates the Tauri updater key pair once and prints ONLY the public key (base64).
# The private key is written to %USERPROFILE%\.relay\signing.key and never printed.
# The password is either -Password or a random one saved next to the key as
# %USERPROFILE%\.relay\signing.key.password; release.ps1 reads that file.
[CmdletBinding()]
param(
    [string]$Password,
    [switch]$Force
)

$ErrorActionPreference = "Stop"

$dir = Join-Path $env:USERPROFILE ".relay"
$keyPath = Join-Path $dir "signing.key"
$publicPath = "$keyPath.pub"
$passwordPath = "$keyPath.password"

if (((Test-Path -LiteralPath $keyPath) -or (Test-Path -LiteralPath $publicPath)) -and -not $Force) {
    throw "A signing key already exists at $keyPath. Re-run with -Force only to rotate it (installed apps stop accepting updates signed by the old key)."
}

New-Item -ItemType Directory -Force -Path $dir | Out-Null

$generatedPassword = $false
if ([string]::IsNullOrWhiteSpace($Password)) {
    $bytes = New-Object byte[] 24
    $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    try {
        $rng.GetBytes($bytes)
    } finally {
        $rng.Dispose()
    }
    $Password = [Convert]::ToBase64String($bytes)
    $generatedPassword = $true
}

$tauriArgs = @("tauri", "signer", "generate", "-w", $keyPath, "--password", $Password)
if ($Force) {
    $tauriArgs += "--force"
}

# The CLI may echo key material; capture it and only ever emit the public key.
$output = & pnpm @tauriArgs 2>&1 | Out-String
if ($LASTEXITCODE -ne 0) {
    throw "pnpm tauri signer generate failed (exit $LASTEXITCODE). Output withheld because it may contain the private key."
}

$escape = [char]27
$clean = $output -replace "$escape\[[0-9;]*m", ""

# With -w the CLI prints "Public: <path>" and writes the base64 public key to
# <key>.pub. Read that file; the path in the output is only a fallback.
if (Test-Path -LiteralPath $publicPath) {
    $publicKey = (Get-Content -LiteralPath $publicPath -Raw).Trim()
} else {
    $match = [regex]::Match($clean, '(?im)^\s*Public(?:\s+Key)?:\s*(.+?)\s*$')
    if (-not $match.Success) {
        throw "Could not find the public key in tauri signer generate output. Output withheld because it may contain the private key."
    }
    $printedPath = $match.Groups[1].Value -replace '\s+\(Keep it secret!\)$', ''
    if (-not (Test-Path -LiteralPath $printedPath)) {
        throw "tauri signer generate did not leave a public key at $publicPath. Output withheld because it may contain the private key."
    }
    $publicKey = (Get-Content -LiteralPath $printedPath -Raw).Trim()
}

if ([string]::IsNullOrWhiteSpace($publicKey)) {
    throw "The public key file at $publicPath is empty."
}

if ($generatedPassword) {
    [System.IO.File]::WriteAllText($passwordPath, $Password + "`n", [System.Text.UTF8Encoding]::new($false))
} elseif ($Force -and (Test-Path -LiteralPath $passwordPath)) {
    # A rotated key with a supplied password must not leave the old generated password behind.
    Remove-Item -LiteralPath $passwordPath -Force
}

Write-Output $publicKey
