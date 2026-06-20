param(
    [string]$Version = "",
    [string]$Target = "x86_64-pc-windows-msvc",
    [string]$ApiUrl = "",
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

if (-not $Version) {
    $Version = (git rev-parse --short HEAD).Trim()
}
if (-not $ApiUrl) {
    $ApiUrl = if ($env:DEVBOX_DEFAULT_API_URL) {
        $env:DEVBOX_DEFAULT_API_URL
    } else {
        "https://devbox-staging.up.railway.app"
    }
}

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $RepoRoot

if ($Target -ne "x86_64-pc-windows-msvc") {
    throw "Windows packaging currently supports x86_64-pc-windows-msvc only"
}

$PackageName = "devbox-$Version-$Target"
$DistDir = Join-Path $RepoRoot "dist"
$StageDir = Join-Path $DistDir $PackageName
$Archive = Join-Path $DistDir "$PackageName.zip"
$Checksum = "$Archive.sha256"

Remove-Item -Recurse -Force -Path $StageDir -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $StageDir | Out-Null
New-Item -ItemType Directory -Force -Path $DistDir | Out-Null

$previousApiUrl = $env:DEVBOX_DEFAULT_API_URL
try {
    $env:DEVBOX_DEFAULT_API_URL = $ApiUrl
    if (-not $SkipBuild) {
        rustup target add $Target
        cargo build --release --locked `
            -p loom-cli `
            -p devbox-cli `
            -p devbox-daemon `
            -p devbox-metadata `
            --target $Target
    }
} finally {
    $env:DEVBOX_DEFAULT_API_URL = $previousApiUrl
}

$ReleaseDir = Join-Path $RepoRoot "target\$Target\release"
foreach ($binary in @("loom.exe", "devbox.exe", "devbox-daemon.exe", "devbox-metadata.exe")) {
    Copy-Item -Force -Path (Join-Path $ReleaseDir $binary) -Destination (Join-Path $StageDir $binary)
}

Copy-Item -Force -Path (Join-Path $RepoRoot "README.md") -Destination (Join-Path $StageDir "README.md")
Copy-Item -Force -Path (Join-Path $RepoRoot "LICENSE") -Destination (Join-Path $StageDir "LICENSE")

@"
# Devbox CLI local/dev overrides.
# Packaged production builds should already know the Devbox API endpoint.

# DEVBOX_API_URL=https://devbox-staging.up.railway.app
DEVBOX_CONFIG_DIR=.devbox
"@ | Set-Content -Encoding UTF8 -Path (Join-Path $StageDir ".env.example")

Copy-Item -Force -Path (Join-Path $RepoRoot ".env.example") -Destination (Join-Path $StageDir ".env.operator.example")
New-Item -ItemType Directory -Force -Path (Join-Path $StageDir "scripts") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $StageDir "docs") | Out-Null
Copy-Item -Force -Path (Join-Path $RepoRoot "scripts\install-devbox.ps1") -Destination (Join-Path $StageDir "scripts\install-devbox.ps1")
Copy-Item -Force -Path (Join-Path $RepoRoot "scripts\install-devbox.sh") -Destination (Join-Path $StageDir "scripts\install-devbox.sh")
Copy-Item -Force -Path (Join-Path $RepoRoot "scripts\load-r2-env.sh") -Destination (Join-Path $StageDir "scripts\load-r2-env.sh")
Copy-Item -Force -Path (Join-Path $RepoRoot "scripts\devbox-live-sync-alpha.sh") -Destination (Join-Path $StageDir "scripts\devbox-live-sync-alpha.sh")
Copy-Item -Force -Path (Join-Path $RepoRoot "scripts\alpha-two-device-smoke.sh") -Destination (Join-Path $StageDir "scripts\alpha-two-device-smoke.sh")
Copy-Item -Force -Path (Join-Path $RepoRoot "docs\alpha-cli-distribution.md") -Destination (Join-Path $StageDir "docs\alpha-cli-distribution.md")

Remove-Item -Force -Path $Archive -ErrorAction SilentlyContinue
Add-Type -AssemblyName System.IO.Compression.FileSystem
[System.IO.Compression.ZipFile]::CreateFromDirectory($StageDir, $Archive, [System.IO.Compression.CompressionLevel]::Optimal, $true)

$Hash = (Get-FileHash -Algorithm SHA256 $Archive).Hash.ToLowerInvariant()
"$Hash  $(Split-Path $Archive -Leaf)" | Set-Content -Encoding ASCII -Path $Checksum

Write-Host "archive=$Archive"
Write-Host "checksum=$Checksum"
