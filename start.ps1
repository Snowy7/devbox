param(
    [switch]$WithDesktop
)

$ErrorActionPreference = "Stop"

$repo = Split-Path -Parent $MyInvocation.MyCommand.Path

function Start-BindhubService {
    param(
        [string]$Name,
        [string]$Command
    )

    Write-Host "Starting $Name..."
    Start-Process powershell -ArgumentList @(
        "-NoExit",
        "-ExecutionPolicy", "Bypass",
        "-Command", "Set-Location '$repo'; $Command"
    ) | Out-Null
}

Start-BindhubService "Bindhub API" "pnpm dev:api"
Start-BindhubService "Bindhub dashboard" "pnpm dev:web"
Start-BindhubService "Bindhub public site" "pnpm dev:site"

if ($WithDesktop) {
    Start-BindhubService "Bindhub desktop renderer" "pnpm dev:desktop"
}

Write-Host ""
Write-Host "Bindhub local stack started."
Write-Host "API:       http://127.0.0.1:3001"
Write-Host "Dashboard: http://localhost:3000"
Write-Host "Site/docs: http://localhost:3002"
Write-Host ""
Write-Host "Use -WithDesktop to also start the desktop renderer."
