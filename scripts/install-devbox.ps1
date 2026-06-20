param(
    [string]$Version = "latest",
    [string]$Repo = "Snowy7/devbox",
    [string]$InstallDir = "$env:LOCALAPPDATA\Devbox\bin"
)

$ErrorActionPreference = "Stop"
$Target = "x86_64-pc-windows-msvc"

function Get-ReleaseTag {
    param([string]$Requested)

    if ($Requested -and $Requested -ne "latest") {
        return $Requested
    }

    $releases = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases" -Headers @{
        "User-Agent" = "devbox-installer"
    }
    $release = @($releases | Where-Object { -not $_.draft } | Select-Object -First 1)
    if (-not $release) {
        throw "No Devbox releases found for $Repo"
    }
    return $release.tag_name
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
$BaseUrl = "https://github.com/$Repo/releases/download/$Tag"
$TempDir = Join-Path ([IO.Path]::GetTempPath()) ("devbox-install-" + [guid]::NewGuid().ToString("N"))
$Archive = Join-Path $TempDir $Asset
$Checksum = "$Archive.sha256"

New-Item -ItemType Directory -Force -Path $TempDir | Out-Null
try {
    Invoke-WebRequest -Uri "$BaseUrl/$Asset" -OutFile $Archive
    Invoke-WebRequest -Uri "$BaseUrl/$Asset.sha256" -OutFile $Checksum

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

    Add-UserPath -PathToAdd $InstallDir

    Write-Host "Devbox $Tag installed to $InstallDir"
    Write-Host "Open a new terminal, then run: devbox --help"
    Write-Host "To update later, rerun this script."
} finally {
    Remove-Item -Recurse -Force -Path $TempDir -ErrorAction SilentlyContinue
}
