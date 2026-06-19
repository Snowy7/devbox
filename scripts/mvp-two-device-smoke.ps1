param(
    [switch]$Help
)

$ErrorActionPreference = "Stop"

if ($Help) {
    @"
Usage: powershell -ExecutionPolicy Bypass -File scripts/mvp-two-device-smoke.ps1

Runs a deterministic local MVP smoke test that proves:
  - Loom local-only capture/checkpoint works
  - Loom local remote sync/clone works
  - Devbox hosted share/clone works through a local devbox-api
  - Git metadata and generated dependencies are not materialized
  - Plain and nested folders materialize
  - Divergent sync refuses safely
  - Secret-looking files are blocked and not uploaded

Environment:
  LOOM_BIN                 Optional path to a built loom binary.
  DEVBOX_BIN               Optional path to a built devbox binary.
  DEVBOX_API_BIN           Optional path to a built devbox-api binary.
  DEVBOX_MVP_SMOKE_DIR     Optional working directory to reuse.
  DEVBOX_CLEAN_SMOKE_DIR   Set true to remove the generated temp directory after a pass.
"@
    exit 0
}

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

if ($env:DEVBOX_MVP_SMOKE_DIR) {
    $WorkDir = $env:DEVBOX_MVP_SMOKE_DIR
    New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null
    $Cleanup = $false
} else {
    $WorkDir = Join-Path ([System.IO.Path]::GetTempPath()) ("devbox-mvp-smoke." + [System.Guid]::NewGuid().ToString("N").Substring(0, 8))
    New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null
    $Cleanup = $false
}

if ($env:DEVBOX_CLEAN_SMOKE_DIR -eq "true") {
    $Cleanup = $true
}

$EvidenceDir = Join-Path $WorkDir "evidence"
New-Item -ItemType Directory -Force -Path $EvidenceDir | Out-Null
$ApiProcess = $null

function Fail($Message) {
    Write-Error "[FAIL] $Message"
    Write-Host "evidence=$EvidenceDir"
    throw $Message
}

function Redact-Text($Text) {
    $Text = $Text -replace "devbox-local-session-[A-Za-z0-9]+", "devbox-local-session-<redacted>"
    $Text = $Text -replace "devbox://\S+", "devbox://<redacted>"
    $Text = $Text -replace "sk-[A-Za-z0-9_.-]+", "sk-<redacted>"
    $Text = $Text -replace "github_pat_[A-Za-z0-9_]+", "github_pat_<redacted>"
    $Text = $Text -replace "ghp_[A-Za-z0-9_]+", "ghp_<redacted>"
    $Text
}

function Redact-File($RawPath, $OutputPath) {
    if (Test-Path $RawPath) {
        $text = Get-Content -Raw -Path $RawPath
        Set-Content -Path $OutputPath -Value (Redact-Text $text)
        Remove-Item -Force -Path $RawPath
    } else {
        Set-Content -Path $OutputPath -Value ""
    }
}

function Redact-FileCopy($RawPath, $OutputPath) {
    if (Test-Path $RawPath) {
        $text = Get-Content -Raw -Path $RawPath
        Set-Content -Path $OutputPath -Value (Redact-Text $text)
    } else {
        Set-Content -Path $OutputPath -Value ""
    }
}

function Invoke-Logged {
    param(
        [string]$Name,
        [string]$Exe,
        [string[]]$CommandArgs,
        [hashtable]$Env = @{},
        [switch]$ExpectFailure
    )

    $stdout = Join-Path $EvidenceDir "$Name.stdout.log"
    $stderr = Join-Path $EvidenceDir "$Name.stderr.log"
    $stdoutRaw = "$stdout.raw"
    $stderrRaw = "$stderr.raw"
    $oldEnv = @{}

    Write-Host ("[RUN] {0}{1}" -f $Name, $(if ($ExpectFailure) { " (expecting refusal)" } else { "" }))

    foreach ($key in $Env.Keys) {
        $oldEnv[$key] = [Environment]::GetEnvironmentVariable($key, "Process")
        [Environment]::SetEnvironmentVariable($key, [string]$Env[$key], "Process")
    }

    try {
        $oldErrorActionPreference = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        & $Exe @CommandArgs 1> $stdoutRaw 2> $stderrRaw
        $code = if ($null -eq $LASTEXITCODE) { 0 } else { $LASTEXITCODE }
    } finally {
        $ErrorActionPreference = $oldErrorActionPreference
        foreach ($key in $Env.Keys) {
            [Environment]::SetEnvironmentVariable($key, $oldEnv[$key], "Process")
        }
    }

    Redact-File $stdoutRaw $stdout
    Redact-File $stderrRaw $stderr

    if ($ExpectFailure) {
        if ($code -eq 0) {
            Fail "$Name unexpectedly succeeded"
        }
        Write-Host "[PASS] $Name refused safely"
        return
    }

    if ($code -ne 0) {
        Write-Host "[FAIL] $Name exited $code"
        Get-Content -Tail 40 -Path $stderr | ForEach-Object { Write-Host $_ }
        Fail "$Name failed"
    }

    Write-Host "[PASS] $Name"
}

function Expect-Contains($Path, $Needle) {
    $text = Get-Content -Raw -Path $Path
    if (-not $text.Contains($Needle)) {
        Fail "$Path did not contain '$Needle'"
    }
}

function Expect-FileText($Path, $Expected) {
    if (-not (Test-Path $Path)) {
        Fail "missing file $Path"
    }
    $actual = (Get-Content -Raw -Path $Path).TrimEnd("`r", "`n")
    if ($actual -ne $Expected) {
        Fail "unexpected contents in $Path"
    }
}

function Expect-Absent($Path) {
    if (Test-Path $Path) {
        Fail "unexpected path exists: $Path"
    }
}

function Get-BinaryPath($EnvValue, $Package, $Binary) {
    if ($EnvValue) {
        return $EnvValue
    }

    $candidate = Join-Path $RepoRoot "target\debug\$Binary.exe"
    if (-not (Test-Path $candidate)) {
        Push-Location $RepoRoot
        try {
            & cargo build --quiet -p $Package
            if ($LASTEXITCODE -ne 0) {
                Fail "cargo build failed for $Package"
            }
        } finally {
            Pop-Location
        }
    }
    $candidate
}

function New-TextFile($Path, $Content) {
    $parent = Split-Path -Parent $Path
    if ($parent) {
        New-Item -ItemType Directory -Force -Path $parent | Out-Null
    }
    Set-Content -NoNewline -Path $Path -Value $Content
}

function Tree-Contains($Root, $Needle) {
    if (-not (Test-Path $Root)) {
        return $false
    }
    foreach ($file in Get-ChildItem -Recurse -File -Force -Path $Root) {
        if (Select-String -Path $file.FullName -SimpleMatch -Quiet -Pattern $Needle) {
            return $true
        }
    }
    $false
}

try {
    $LoomBin = Get-BinaryPath $env:LOOM_BIN "loom-cli" "loom"
    $DevboxBin = Get-BinaryPath $env:DEVBOX_BIN "devbox-cli" "devbox"
    $DevboxApiBin = Get-BinaryPath $env:DEVBOX_API_BIN "devbox-api" "devbox-api"

    Write-Host "Devbox MVP two-device smoke"
    Write-Host "workdir=$WorkDir"
    Write-Host "evidence=$EvidenceDir"

    $EngineSource = Join-Path $WorkDir "engine-source"
    $EngineRemote = Join-Path $WorkDir "engine-remote"
    $EngineClone = Join-Path $WorkDir "engine-clone"
    $PlainSource = Join-Path $WorkDir "plain-source"
    $PlainRemote = Join-Path $WorkDir "plain-remote"
    $PlainClone = Join-Path $WorkDir "plain-clone"
    $SecretSource = Join-Path $WorkDir "secret-source"
    $SecretRemote = Join-Path $WorkDir "secret-remote"
    $ConflictSource = Join-Path $WorkDir "conflict-source"
    $ConflictRemote = Join-Path $WorkDir "conflict-remote"

    New-TextFile (Join-Path $EngineSource "README.md") "engine source"
    New-TextFile (Join-Path $EngineSource "apps\api\app.txt") "api"
    New-TextFile (Join-Path $EngineSource "apps\web\src\index.ts") "web"
    New-TextFile (Join-Path $EngineSource ".git\config") "[core]`nrepositoryformatversion = 0"
    New-TextFile (Join-Path $EngineSource ".git\HEAD") "ref: refs/heads/main"
    New-TextFile (Join-Path $EngineSource "node_modules\pkg\index.js") "generated"
    New-TextFile (Join-Path $EngineSource "apps\web\dist\app.js") "bundle"
    New-TextFile (Join-Path $PlainSource "README.md") "plain folder"
    New-TextFile (Join-Path $PlainSource "nested\docs\guide.md") "nested plain folder"
    New-TextFile (Join-Path $SecretSource "README.md") "safe"
    $RawSecret = "sk-abcdefghijklmnopqrstuvwxyzABCDEFGH123456"
    New-TextFile (Join-Path $SecretSource "secrets.env") "OPENAI_API_KEY=$RawSecret"
    New-TextFile (Join-Path $ConflictSource "README.md") "one"

    Invoke-Logged -Name "01-loom-local-track" -Exe $LoomBin -CommandArgs @("track", $EngineSource)
    Expect-Contains (Join-Path $EvidenceDir "01-loom-local-track.stdout.log") "Initialized Loom tracking"
    Expect-Contains (Join-Path $EvidenceDir "01-loom-local-track.stdout.log") "Policy:"
    Invoke-Logged -Name "02-loom-local-checkpoint" -Exe $LoomBin -CommandArgs @("checkpoint", $EngineSource, "-m", "MVP-baseline")
    New-TextFile (Join-Path $EngineSource "README.md") "engine source v2"
    Invoke-Logged -Name "03-loom-local-status-after-edit" -Exe $LoomBin -CommandArgs @("status", $EngineSource)
    Expect-Contains (Join-Path $EvidenceDir "03-loom-local-status-after-edit.stdout.log") "Captured new folder revision"

    Invoke-Logged -Name "04-loom-remote-add" -Exe $LoomBin -CommandArgs @("remote", "add", "local", $EngineRemote, $EngineSource)
    Invoke-Logged -Name "05-loom-remote-sync" -Exe $LoomBin -CommandArgs @("sync", $EngineSource)
    Invoke-Logged -Name "06-loom-remote-clone" -Exe $LoomBin -CommandArgs @("clone", $EngineRemote, $EngineClone)
    Expect-FileText (Join-Path $EngineClone "README.md") "engine source v2"
    Expect-FileText (Join-Path $EngineClone "apps\api\app.txt") "api"
    Expect-FileText (Join-Path $EngineClone "apps\web\src\index.ts") "web"
    Expect-Absent (Join-Path $EngineClone ".git")
    Expect-Absent (Join-Path $EngineClone "node_modules")
    Expect-Absent (Join-Path $EngineClone "apps\web\dist")

    Invoke-Logged -Name "07-plain-track" -Exe $LoomBin -CommandArgs @("track", $PlainSource)
    Invoke-Logged -Name "08-plain-remote-add" -Exe $LoomBin -CommandArgs @("remote", "add", "local", $PlainRemote, $PlainSource)
    Invoke-Logged -Name "09-plain-sync" -Exe $LoomBin -CommandArgs @("sync", $PlainSource)
    Invoke-Logged -Name "10-plain-clone" -Exe $LoomBin -CommandArgs @("clone", $PlainRemote, $PlainClone)
    Expect-FileText (Join-Path $PlainClone "README.md") "plain folder"
    Expect-FileText (Join-Path $PlainClone "nested\docs\guide.md") "nested plain folder"

    Invoke-Logged -Name "11-secret-track" -Exe $LoomBin -CommandArgs @("track", $SecretSource)
    Expect-Contains (Join-Path $EvidenceDir "11-secret-track.stdout.log") "secret-blocked"
    Invoke-Logged -Name "12-secret-remote-add" -Exe $LoomBin -CommandArgs @("remote", "add", "local", $SecretRemote, $SecretSource)
    Invoke-Logged -Name "13-secret-sync-refusal" -Exe $LoomBin -CommandArgs @("sync", $SecretSource) -ExpectFailure
    Expect-Contains (Join-Path $EvidenceDir "13-secret-sync-refusal.stderr.log") "secret-blocked"
    if ((Tree-Contains $SecretRemote $RawSecret) -or (Tree-Contains (Join-Path $SecretSource ".loom\objects") $RawSecret)) {
        Fail "raw secret was written to a remote or Loom object cache"
    }

    Invoke-Logged -Name "14-conflict-track" -Exe $LoomBin -CommandArgs @("track", $ConflictSource)
    Invoke-Logged -Name "15-conflict-remote-add" -Exe $LoomBin -CommandArgs @("remote", "add", "local", $ConflictRemote, $ConflictSource)
    New-TextFile (Join-Path $ConflictRemote "cursors\shared-folder.txt") "folder-revision-b3-divergent"
    Invoke-Logged -Name "16-conflict-sync-refusal" -Exe $LoomBin -CommandArgs @("sync", $ConflictSource) -ExpectFailure
    Expect-Contains (Join-Path $EvidenceDir "16-conflict-sync-refusal.stderr.log") "diverged"

    $ApiRoot = Join-Path $WorkDir "api-root"
    $ApiStdoutRaw = Join-Path $EvidenceDir "17-devbox-api.stdout.log.raw"
    $ApiStderrRaw = Join-Path $EvidenceDir "17-devbox-api.stderr.log.raw"
    $ApiProcess = Start-Process -FilePath $DevboxApiBin -ArgumentList @("--root", $ApiRoot, "--bind", "127.0.0.1:0") -RedirectStandardOutput $ApiStdoutRaw -RedirectStandardError $ApiStderrRaw -PassThru -WindowStyle Hidden
    $ApiUrl = ""
    for ($i = 0; $i -lt 100; $i++) {
        if ($ApiProcess.HasExited) {
            Redact-FileCopy $ApiStdoutRaw (Join-Path $EvidenceDir "17-devbox-api.stdout.log")
            Redact-FileCopy $ApiStderrRaw (Join-Path $EvidenceDir "17-devbox-api.stderr.log")
            Fail "devbox-api exited before it was ready"
        }
        if (Test-Path $ApiStdoutRaw) {
            $apiLog = Get-Content -Raw -Path $ApiStdoutRaw
            if ($null -eq $apiLog) {
                $apiLog = ""
            }
            $match = [regex]::Match($apiLog, "http://127\.0\.0\.1:[0-9]+")
            if ($match.Success) {
                $ApiUrl = $match.Value
                break
            }
        }
        Start-Sleep -Milliseconds 50
    }
    Redact-FileCopy $ApiStdoutRaw (Join-Path $EvidenceDir "17-devbox-api.stdout.log")
    Redact-FileCopy $ApiStderrRaw (Join-Path $EvidenceDir "17-devbox-api.stderr.log")
    if (-not $ApiUrl) {
        Fail "could not parse devbox-api URL"
    }
    Write-Host "[PASS] 17-devbox-api-start"

    $ProductSource = Join-Path $WorkDir "product-source"
    $ProductTarget = Join-Path $WorkDir "product-target"
    $SourceConfig = Join-Path $WorkDir "source-config"
    $TargetConfig = Join-Path $WorkDir "target-config"
    New-TextFile (Join-Path $ProductSource "README.md") "product source"
    New-TextFile (Join-Path $ProductSource "app\main.txt") "nested product file"
    New-TextFile (Join-Path $ProductSource ".git\config") "[core]`nrepositoryformatversion = 0"
    New-TextFile (Join-Path $ProductSource "node_modules\pkg\index.js") "generated"

    Invoke-Logged -Name "18-devbox-source-login" -Exe $DevboxBin -CommandArgs @("login", "--api", $ApiUrl, "--account", "mvp", "--device-name", "MVP-desktop") -Env @{ DEVBOX_CONFIG_DIR = $SourceConfig }
    Invoke-Logged -Name "19-devbox-source-share" -Exe $DevboxBin -CommandArgs @("share", $ProductSource, "--no-background-sync") -Env @{ DEVBOX_CONFIG_DIR = $SourceConfig }
    Invoke-Logged -Name "20-devbox-target-login" -Exe $DevboxBin -CommandArgs @("login", "--api", $ApiUrl, "--account", "mvp", "--device-name", "MVP-laptop") -Env @{ DEVBOX_CONFIG_DIR = $TargetConfig }
    Invoke-Logged -Name "21-devbox-target-clone" -Exe $DevboxBin -CommandArgs @("clone", "product-source", $ProductTarget, "--no-background-sync") -Env @{ DEVBOX_CONFIG_DIR = $TargetConfig }
    Expect-FileText (Join-Path $ProductTarget "README.md") "product source"
    Expect-FileText (Join-Path $ProductTarget "app\main.txt") "nested product file"
    Expect-Absent (Join-Path $ProductTarget ".git")
    Expect-Absent (Join-Path $ProductTarget "node_modules")

    New-TextFile (Join-Path $ProductSource "README.md") "product source after edit"
    Invoke-Logged -Name "22-devbox-source-push-edit" -Exe $DevboxBin -CommandArgs @("resume", $ProductSource, "--no-background-sync") -Env @{ DEVBOX_CONFIG_DIR = $SourceConfig }
    Invoke-Logged -Name "23-devbox-target-sync-edit" -Exe $DevboxBin -CommandArgs @("sync", "run-loop", $ProductTarget, "--max-cycles", "1") -Env @{ DEVBOX_CONFIG_DIR = $TargetConfig }
    Expect-FileText (Join-Path $ProductTarget "README.md") "product source after edit"

    Invoke-Logged -Name "24-devbox-status" -Exe $DevboxBin -CommandArgs @("status") -Env @{ DEVBOX_CONFIG_DIR = $TargetConfig }
    Expect-Contains (Join-Path $EvidenceDir "24-devbox-status.stdout.log") "Logged in: yes"
    Expect-Contains (Join-Path $EvidenceDir "24-devbox-status.stdout.log") "Shared folders:"

    if (Tree-Contains $EvidenceDir $RawSecret) {
        Fail "evidence logs contain an unredacted secret fixture"
    }

    Get-ChildItem -Recurse -File -Force -Path $EngineClone, $PlainClone, $ProductTarget |
        Sort-Object FullName |
        ForEach-Object { $_.FullName } |
        Set-Content -Path (Join-Path $EvidenceDir "materialized-files.txt")

    @"
Devbox MVP two-device smoke passed.

Workdir: $WorkDir
API: $ApiUrl

Proofs:
- Loom local-only capture/checkpoint/status works.
- Loom local filesystem remote sync and clone work.
- Devbox hosted login/share/clone works through local devbox-api.
- Editing the source shared folder propagates to the target through sync.
- Git metadata is preserved locally and not materialized into clones.
- Generated dependency/build output directories are suppressed.
- Plain and nested folders materialize.
- Divergent remote cursor state refuses safely.
- Secret-looking files are blocked before sync and raw secret bytes are absent from remote/object cache/evidence.

Evidence logs are in this directory. Session tokens and devbox URLs are redacted.
"@ | Set-Content -Path (Join-Path $EvidenceDir "SUMMARY.txt")

    Write-Host "mvp smoke passed"
    Write-Host "evidence=$EvidenceDir"
    Write-Host ("summary=" + (Join-Path $EvidenceDir "SUMMARY.txt"))
} finally {
    if ($ApiProcess -and -not $ApiProcess.HasExited) {
        Stop-Process -Id $ApiProcess.Id -Force -ErrorAction SilentlyContinue
        $ApiProcess.WaitForExit()
    }
    if ($Cleanup -and (Test-Path $WorkDir)) {
        Remove-Item -Recurse -Force -LiteralPath $WorkDir
    }
}
