param(
    [string]$Version = "latest",
    [string]$Repo = "Snowy7/devbox",
    [string]$InstallDir = "$env:LOCALAPPDATA\Devbox\bin",
    [switch]$NoPath
)

$ErrorActionPreference = "Stop"
$Target = "x86_64-pc-windows-msvc"

function Get-ReleaseTag {
    param([string]$Requested)

    if ($Requested -and $Requested -ne "latest") {
        return $Requested
    }

    $releases = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases" -Headers (GithubHeaders)
    $release = @($releases | Where-Object { -not $_.draft } | Select-Object -First 1)
    if (-not $release) {
        throw "No Devbox releases found for $Repo"
    }
    return $release.tag_name
}

function GithubHeaders {
    param([switch]$Download)

    $headers = @{
        "User-Agent" = "devbox-installer"
        "Accept" = if ($Download) { "application/octet-stream" } else { "application/vnd.github+json" }
    }
    $token = if ($env:DEVBOX_GITHUB_TOKEN) {
        $env:DEVBOX_GITHUB_TOKEN
    } else {
        $env:GITHUB_TOKEN
    }
    if ($token) {
        $headers["Authorization"] = "Bearer $token"
    }
    return $headers
}

function Get-ReleaseAssetUrl {
    param(
        [string]$Tag,
        [string]$AssetName
    )

    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/tags/$Tag" -Headers (GithubHeaders)
    $asset = @($release.assets | Where-Object { $_.name -eq $AssetName } | Select-Object -First 1)
    if (-not $asset) {
        throw "Release $Tag does not have asset $AssetName"
    }
    return $asset.url
}

function Add-UserPath {
    param([string]$PathToAdd)

    $current = [Environment]::GetEnvironmentVariable("Path", "User")
    $parts = @()
    if ($current) {
        $parts = $current -split ";" | Where-Object { $_ }
    }
    if ($parts -notcontains $PathToAdd) {
        $next = (@($parts) + $PathToAdd) -join ";"
        [Environment]::SetEnvironmentVariable("Path", $next, "User")
    }
    if (($env:Path -split ";") -notcontains $PathToAdd) {
        $env:Path = "$PathToAdd;$env:Path"
    }
}

$Tag = Get-ReleaseTag -Requested $Version
$Asset = "devbox-$Tag-$Target.zip"
$AssetUrl = Get-ReleaseAssetUrl -Tag $Tag -AssetName $Asset
$ChecksumUrl = Get-ReleaseAssetUrl -Tag $Tag -AssetName "$Asset.sha256"
$TempDir = Join-Path ([IO.Path]::GetTempPath()) ("devbox-install-" + [guid]::NewGuid().ToString("N"))
$Archive = Join-Path $TempDir $Asset
$Checksum = "$Archive.sha256"

New-Item -ItemType Directory -Force -Path $TempDir | Out-Null
try {
    Invoke-WebRequest -Uri $AssetUrl -OutFile $Archive -Headers (GithubHeaders -Download)
    Invoke-WebRequest -Uri $ChecksumUrl -OutFile $Checksum -Headers (GithubHeaders -Download)

    $expected = (Get-Content $Checksum -Raw).Trim().Split(" ", [StringSplitOptions]::RemoveEmptyEntries)[0].ToLowerInvariant()
    $actual = (Get-FileHash -Algorithm SHA256 $Archive).Hash.ToLowerInvariant()
    if ($expected -ne $actual) {
        throw "Checksum mismatch for $Asset"
    }

    Expand-Archive -Path $Archive -DestinationPath $TempDir -Force
    $PackageDir = Join-Path $TempDir "devbox-$Tag-$Target"
    if (-not (Test-Path $PackageDir)) {
        throw "Release archive did not contain $PackageDir"
    }

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    foreach ($binary in @("loom.exe", "devbox.exe", "devbox-daemon.exe", "devbox-metadata.exe")) {
        Copy-Item -Force -Path (Join-Path $PackageDir $binary) -Destination (Join-Path $InstallDir $binary)
    }

    if (-not $NoPath) {
        Add-UserPath -PathToAdd $InstallDir
    }

    Write-Host "Devbox $Tag installed to $InstallDir"
    if ($NoPath) {
        Write-Host "PATH was not changed because -NoPath was supplied."
    } else {
        Write-Host "Open a new terminal, then run: devbox --help"
    }
    Write-Host "To update later, rerun this script."
} finally {
    Remove-Item -Recurse -Force -Path $TempDir -ErrorAction SilentlyContinue
}
