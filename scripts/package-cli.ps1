param(
    [string]$Version = "",
    [string]$Target = "x86_64-pc-windows-msvc",
    [string]$ApiUrl = "",
    [string]$WebUrl = "",
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

if (-not $Version) {
    $Version = (git rev-parse --short HEAD).Trim()
}
if (-not $ApiUrl) {
    $ApiUrl = if ($env:BINDHUB_DEFAULT_API_URL) {
        $env:BINDHUB_DEFAULT_API_URL
    } else {
        "https://staging-api.bindhub.dev/"
    }
}
if (-not $WebUrl) {
    $WebUrl = if ($env:BINDHUB_DEFAULT_WEB_URL) {
        $env:BINDHUB_DEFAULT_WEB_URL
    } else {
        "https://app-staging.bindhub.com"
    }
}

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $RepoRoot

if ($Target -ne "x86_64-pc-windows-msvc") {
    throw "Windows packaging currently supports x86_64-pc-windows-msvc only"
}

$PackageName = "bindhub-$Version-$Target"
$DistDir = Join-Path $RepoRoot "dist"
$StageDir = Join-Path $DistDir $PackageName
$Archive = Join-Path $DistDir "$PackageName.zip"
$Checksum = "$Archive.sha256"

Remove-Item -Recurse -Force -Path $StageDir -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $StageDir | Out-Null
New-Item -ItemType Directory -Force -Path $DistDir | Out-Null

$previousApiUrl = $env:BINDHUB_DEFAULT_API_URL
$previousWebUrl = $env:BINDHUB_DEFAULT_WEB_URL
try {
    $env:BINDHUB_DEFAULT_API_URL = $ApiUrl
    $env:BINDHUB_DEFAULT_WEB_URL = $WebUrl
    if (-not $SkipBuild) {
        rustup target add $Target
        cargo build --release --locked `
            -p loom-cli `
            -p bindhub-cli `
            -p bindhub-daemon `
            -p bindhub-metadata `
            --target $Target
    }
} finally {
    $env:BINDHUB_DEFAULT_API_URL = $previousApiUrl
    $env:BINDHUB_DEFAULT_WEB_URL = $previousWebUrl
}

$ReleaseDir = Join-Path $RepoRoot "target\$Target\release"
foreach ($binary in @("loom.exe", "bindhub.exe", "bindhub-daemon.exe", "bindhub-metadata.exe")) {
    Copy-Item -Force -Path (Join-Path $ReleaseDir $binary) -Destination (Join-Path $StageDir $binary)
}

Copy-Item -Force -Path (Join-Path $RepoRoot "README.md") -Destination (Join-Path $StageDir "README.md")
Copy-Item -Force -Path (Join-Path $RepoRoot "LICENSE") -Destination (Join-Path $StageDir "LICENSE")

@"
# Bindhub CLI local/dev overrides.
# Packaged production builds should already know the Bindhub API endpoint.

# BINDHUB_API_URL=https://staging-api.bindhub.dev/
# BINDHUB_WEB_URL=https://app-staging.bindhub.com
BINDHUB_CONFIG_DIR=.bindhub
"@ | Set-Content -Encoding UTF8 -Path (Join-Path $StageDir ".env.example")

Copy-Item -Force -Path (Join-Path $RepoRoot ".env.example") -Destination (Join-Path $StageDir ".env.operator.example")
New-Item -ItemType Directory -Force -Path (Join-Path $StageDir "scripts") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $StageDir "docs") | Out-Null
Copy-Item -Force -Path (Join-Path $RepoRoot "scripts\install-bindhub.ps1") -Destination (Join-Path $StageDir "scripts\install-bindhub.ps1")
Copy-Item -Force -Path (Join-Path $RepoRoot "scripts\install-bindhub.sh") -Destination (Join-Path $StageDir "scripts\install-bindhub.sh")
Copy-Item -Force -Path (Join-Path $RepoRoot "scripts\load-r2-env.sh") -Destination (Join-Path $StageDir "scripts\load-r2-env.sh")
Copy-Item -Force -Path (Join-Path $RepoRoot "scripts\bindhub-live-sync-alpha.sh") -Destination (Join-Path $StageDir "scripts\bindhub-live-sync-alpha.sh")
Copy-Item -Force -Path (Join-Path $RepoRoot "scripts\alpha-two-device-smoke.sh") -Destination (Join-Path $StageDir "scripts\alpha-two-device-smoke.sh")
Copy-Item -Force -Path (Join-Path $RepoRoot "docs\alpha-cli-distribution.md") -Destination (Join-Path $StageDir "docs\alpha-cli-distribution.md")

Remove-Item -Force -Path $Archive -ErrorAction SilentlyContinue
Add-Type -AssemblyName System.IO.Compression.FileSystem
[System.IO.Compression.ZipFile]::CreateFromDirectory($StageDir, $Archive, [System.IO.Compression.CompressionLevel]::Optimal, $true)

$Hash = (Get-FileHash -Algorithm SHA256 $Archive).Hash.ToLowerInvariant()
"$Hash  $(Split-Path $Archive -Leaf)" | Set-Content -Encoding ASCII -Path $Checksum

Write-Host "archive=$Archive"
Write-Host "checksum=$Checksum"
