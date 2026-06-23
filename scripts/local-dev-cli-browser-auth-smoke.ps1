param(
    [string]$ApiBind = "127.0.0.1:38787",
    [string]$WebBaseUrl = "",
    [string]$ServiceToken = "local-dev-cli-auth-smoke-token"
)

$ErrorActionPreference = "Stop"

$repo = Split-Path -Parent $PSScriptRoot
$target = Join-Path $repo "target\debug"
$apiExe = Join-Path $target "devbox-api.exe"
$cliExe = Join-Path $target "devbox.exe"

Push-Location $repo
try {
    cargo build -p devbox-api -p devbox-cli

    $root = Join-Path ([System.IO.Path]::GetTempPath()) ("devbox-api-smoke-" + [System.Guid]::NewGuid())
    $config = Join-Path ([System.IO.Path]::GetTempPath()) ("devbox-cli-smoke-" + [System.Guid]::NewGuid())
    $folder = Join-Path ([System.IO.Path]::GetTempPath()) ("devbox-folder-smoke-" + [System.Guid]::NewGuid())
    New-Item -ItemType Directory -Force $root, $config, $folder | Out-Null
    Set-Content -Path (Join-Path $folder "README.md") -Value "smoke"

    $env:DEVBOX_API_METADATA_MODE = "memory"
    $env:DEVBOX_API_SERVICE_TOKEN = $ServiceToken
    $env:DEVBOX_CONFIG_DIR = $config
    $api = Start-Process -FilePath $apiExe -ArgumentList @("--root", $root, "--bind", $ApiBind) -PassThru -WindowStyle Hidden
    $apiUrl = "http://$ApiBind"

    for ($i = 0; $i -lt 50; $i++) {
        try {
            Invoke-RestMethod -Uri "$apiUrl/ready" -Method Get | Out-Null
            break
        } catch {
            Start-Sleep -Milliseconds 100
        }
    }

    $loginOut = Join-Path $root "login.out"
    $loginErr = Join-Path $root "login.err"
    $login = Start-Process -FilePath $cliExe -ArgumentList @(
        "login",
        "--api", $apiUrl,
        "--web", ($(if ($WebBaseUrl) { $WebBaseUrl } else { "http://localhost:3000" })),
        "--device-name", "Smoke-machine",
        "--no-browser"
    ) -RedirectStandardOutput $loginOut -RedirectStandardError $loginErr -PassThru -WindowStyle Hidden

    $code = $null
    for ($i = 0; $i -lt 50; $i++) {
        if (Test-Path $loginOut) {
            $text = Get-Content $loginOut -Raw
            if ($text -match "User code: ([A-Z0-9-]+)") {
                $code = $Matches[1]
                break
            }
        }
        Start-Sleep -Milliseconds 100
    }
    if (-not $code) {
        throw "CLI did not print a user code. stdout: $(Get-Content $loginOut -Raw) stderr: $(Get-Content $loginErr -Raw)"
    }

    if ($WebBaseUrl) {
        Invoke-WebRequest -Uri "$($WebBaseUrl.TrimEnd('/'))/auth/cli?code=$code" -Method Get | Out-Null
    } else {
        Write-Host "No -WebBaseUrl supplied; using explicit local-dev API approval simulation."
        Invoke-RestMethod -Uri "$apiUrl/v1/auth/cli-device-flow/$code/approve" -Method Post -Headers @{
            "x-devbox-api-service-token" = $ServiceToken
        } -ContentType "application/json" -Body (@{
            user_id = "local-dev-smoke"
            session_id = "local-dev-cli-smoke-$code"
            organization_id = $null
        } | ConvertTo-Json) | Out-Null
    }

    if (-not $login.WaitForExit(15000)) {
        throw "devbox login timed out. stdout: $(Get-Content $loginOut -Raw) stderr: $(Get-Content $loginErr -Raw)"
    }
    $login.Refresh()
    $loginStdout = Get-Content $loginOut -Raw
    if (-not $loginStdout.Contains("Logged in to Devbox")) {
        throw "devbox login failed. stdout: $(Get-Content $loginOut -Raw) stderr: $(Get-Content $loginErr -Raw)"
    }

    & $cliExe share $folder --no-background-sync
    if ($LASTEXITCODE -ne 0) {
        throw "devbox share failed"
    }

    Write-Host "CLI browser auth smoke passed"
} finally {
    if ($api -and -not $api.HasExited) {
        Stop-Process -Id $api.Id -Force
    }
    Pop-Location
}
