param(
    [Parameter(Mandatory = $true)]
    [string]$Tag,
    [string]$ApiUrl = ""
)

$ErrorActionPreference = "Stop"

if (-not $ApiUrl) {
    $ApiUrl = if ($env:DEVBOX_DEFAULT_API_URL) {
        $env:DEVBOX_DEFAULT_API_URL
    } else {
        "https://devbox-staging.up.railway.app"
    }
}

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $RepoRoot

gh auth status | Out-Null

$Status = git status --porcelain
if ($Status) {
    throw "working tree has uncommitted changes; commit before publishing a release"
}

git rev-parse -q --verify "refs/tags/$Tag" *> $null
if ($LASTEXITCODE -ne 0) {
    git tag $Tag
}

git push origin $Tag

& "$PSScriptRoot\package-cli.ps1" -Version $Tag -ApiUrl $ApiUrl

$Assets = @(
    Join-Path $RepoRoot "dist\devbox-$Tag-x86_64-pc-windows-msvc.zip"
    Join-Path $RepoRoot "dist\devbox-$Tag-x86_64-pc-windows-msvc.zip.sha256"
)
foreach ($asset in $Assets) {
    if (-not (Test-Path $asset)) {
        throw "missing release asset: $asset"
    }
}

$ReleaseExists = $true
$previousErrorActionPreference = $ErrorActionPreference
$ErrorActionPreference = "Continue"
gh release view $Tag *> $null
$releaseViewExitCode = $LASTEXITCODE
$ErrorActionPreference = $previousErrorActionPreference
if ($releaseViewExitCode -ne 0) {
    $ReleaseExists = $false
}

if ($ReleaseExists) {
    gh release upload $Tag @Assets --clobber
} else {
    $ghArgs = @(
        "release", "create", $Tag
    ) + $Assets + @(
        "--title", "Devbox CLI $Tag",
        "--notes", "Alpha command-line tools for Loom and Devbox. The default API is https://devbox-staging.up.railway.app. OAuth, signed installers, and production hardening are not included yet."
    )
    if ($Tag.Contains("-")) {
        $ghArgs += "--prerelease"
    }
    gh @ghArgs
}

Write-Host "published Windows assets for $Tag"
