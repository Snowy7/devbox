param(
    [switch]$Help
)

$ErrorActionPreference = "Stop"

if ($Help) {
    @"
Usage: powershell -ExecutionPolicy Bypass -File scripts/alpha-workspace-adapters-smoke.ps1

Runs deterministic local alpha evidence for the workspace adapter arc:
  - human sparse folder flow through bindhub login/share/clone --sparse/status/warm/hydrate/keep/free-space
  - agent virtual workspace flow through loom workspace open/read/write/exec/diff/checkpoint/discard
  - materialized sandbox fallback flow with safe capture and unsafe host mutation refusal
  - filesystem adapter alpha flow with native fail-closed status and local-dev metadata simulation

Environment:
  LOOM_BIN                         Optional path to a built loom binary.
  BINDHUB_BIN                       Optional path to a built Bindhub binary.
  BINDHUB_API_BIN                   Optional path to a built bindhub-api binary.
  BINDHUB_ADAPTER_SMOKE_DIR         Optional working directory to reuse.
  BINDHUB_CLEAN_SMOKE_DIR           Set true to remove the generated temp directory after a pass.
"@
    exit 0
}

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

if ($env:BINDHUB_ADAPTER_SMOKE_DIR) {
    $WorkDir = $env:BINDHUB_ADAPTER_SMOKE_DIR
    New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null
    $Cleanup = $false
} else {
    $WorkDir = Join-Path ([System.IO.Path]::GetTempPath()) ("Bindhub-adapter-smoke." + [System.Guid]::NewGuid().ToString("N").Substring(0, 8))
    New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null
    $Cleanup = $false
}

if ($env:BINDHUB_CLEAN_SMOKE_DIR -eq "true") {
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
    $Text = $Text -replace "Bindhub-local-session-[A-Za-z0-9]+", "Bindhub-local-session-<redacted>"
    $Text = $Text -replace "bindhub://\S+", "bindhub://<redacted>"
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

    $exeName = if ($env:OS -eq "Windows_NT") { "$Binary.exe" } else { $Binary }
    $candidate = Join-Path $RepoRoot "target\debug\$exeName"
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
    $utf8NoBom = New-Object System.Text.UTF8Encoding $false
    [System.IO.File]::WriteAllText($Path, $Content, $utf8NoBom)
}

function Remove-SmokePath($Path) {
    if (-not (Test-Path $Path)) {
        return
    }
    $resolved = (Resolve-Path -LiteralPath $Path).Path
    $root = (Resolve-Path -LiteralPath $WorkDir).Path
    if (-not $resolved.StartsWith($root, [System.StringComparison]::OrdinalIgnoreCase)) {
        Fail "refusing to remove path outside smoke workdir: $resolved"
    }
    Remove-Item -Recurse -Force -LiteralPath $Path
}

try {
    $LoomBin = Get-BinaryPath $env:LOOM_BIN "loom-cli" "loom"
    $BindhubBin = Get-BinaryPath $env:BINDHUB_BIN "bindhub-cli" "Bindhub"
    $BindhubApiBin = Get-BinaryPath $env:BINDHUB_API_BIN "bindhub-api" "bindhub-api"

    Write-Host "bindhub/Loom workspace adapter alpha smoke"
    Write-Host "workdir=$WorkDir"
    Write-Host "evidence=$EvidenceDir"

    $ApiRoot = Join-Path $WorkDir "api-root"
    $ApiStdoutRaw = Join-Path $EvidenceDir "00-bindhub-api.stdout.log.raw"
    $ApiStderrRaw = Join-Path $EvidenceDir "00-bindhub-api.stderr.log.raw"
    $OldApiMetadataMode = [Environment]::GetEnvironmentVariable("BINDHUB_API_METADATA_MODE", "Process")
    [Environment]::SetEnvironmentVariable("BINDHUB_API_METADATA_MODE", "memory", "Process")
    try {
        $startArgs = @{
            FilePath = $BindhubApiBin
            ArgumentList = @("--root", $ApiRoot, "--bind", "127.0.0.1:0")
            RedirectStandardOutput = $ApiStdoutRaw
            RedirectStandardError = $ApiStderrRaw
            PassThru = $true
        }
        if ($env:OS -eq "Windows_NT") {
            $startArgs.WindowStyle = "Hidden"
        }
        $ApiProcess = Start-Process @startArgs
    } finally {
        [Environment]::SetEnvironmentVariable("BINDHUB_API_METADATA_MODE", $OldApiMetadataMode, "Process")
    }
    $ApiUrl = ""
    for ($i = 0; $i -lt 100; $i++) {
        if ($ApiProcess.HasExited) {
            Redact-FileCopy $ApiStdoutRaw (Join-Path $EvidenceDir "00-bindhub-api.stdout.log")
            Redact-FileCopy $ApiStderrRaw (Join-Path $EvidenceDir "00-bindhub-api.stderr.log")
            Fail "bindhub-api exited before it was ready"
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
    Redact-FileCopy $ApiStdoutRaw (Join-Path $EvidenceDir "00-bindhub-api.stdout.log")
    Redact-FileCopy $ApiStderrRaw (Join-Path $EvidenceDir "00-bindhub-api.stderr.log")
    if (-not $ApiUrl) {
        Fail "could not parse bindhub-api URL"
    }
    Write-Host "[PASS] 00-bindhub-api-start"

    $SourceConfig = Join-Path $WorkDir "source-config"
    $TargetConfig = Join-Path $WorkDir "target-config"
    $ProductSource = Join-Path $WorkDir "human-source"
    $ProductSparseTarget = Join-Path $WorkDir "human-sparse-target"
    New-TextFile (Join-Path $ProductSource "README.md") "hello"
    New-TextFile (Join-Path $ProductSource "Cargo.toml") "[package]`nname='a'"
    New-TextFile (Join-Path $ProductSource "src\main.rs") "fn main() {}"
    New-TextFile (Join-Path $ProductSource "config\app.toml") "debug=1"
    New-TextFile (Join-Path $ProductSource "docs\guide.md") "guide"
    New-TextFile (Join-Path $ProductSource "big.bin") ("x" * 128)

    Invoke-Logged -Name "01-human-source-login" -Exe $BindhubBin -CommandArgs @("login", "--api", $ApiUrl, "--account", "adapter-alpha", "--device-name", "Adapter desktop") -Env @{ BINDHUB_CONFIG_DIR = $SourceConfig }
    Invoke-Logged -Name "02-human-share" -Exe $BindhubBin -CommandArgs @("share", $ProductSource, "--no-background-sync") -Env @{ BINDHUB_CONFIG_DIR = $SourceConfig }
    Expect-Contains (Join-Path $EvidenceDir "02-human-share.stdout.log") "Shared folder: human-source"
    Invoke-Logged -Name "03-human-target-login" -Exe $BindhubBin -CommandArgs @("login", "--api", $ApiUrl, "--account", "adapter-alpha", "--device-name", "Adapter laptop") -Env @{ BINDHUB_CONFIG_DIR = $TargetConfig }
    Invoke-Logged -Name "04-human-sparse-clone" -Exe $BindhubBin -CommandArgs @("clone", "human-source", $ProductSparseTarget, "--sparse", "--no-background-sync") -Env @{ BINDHUB_CONFIG_DIR = $TargetConfig }
    Expect-Contains (Join-Path $EvidenceDir "04-human-sparse-clone.stdout.log") "Files: available on demand"
    Expect-Absent (Join-Path $ProductSparseTarget "README.md")
    Invoke-Logged -Name "05-human-sparse-status" -Exe $BindhubBin -CommandArgs @("status", $ProductSparseTarget) -Env @{ BINDHUB_CONFIG_DIR = $TargetConfig }
    Expect-Contains (Join-Path $EvidenceDir "05-human-sparse-status.stdout.log") "Cloud-only:"
    Invoke-Logged -Name "06-human-hydrate-readme" -Exe $BindhubBin -CommandArgs @("hydrate", (Join-Path $ProductSparseTarget "README.md")) -Env @{ BINDHUB_CONFIG_DIR = $TargetConfig }
    Expect-Contains (Join-Path $EvidenceDir "06-human-hydrate-readme.stdout.log") "Hydrated: README.md"
    Expect-FileText (Join-Path $ProductSparseTarget "README.md") "hello"
    Invoke-Logged -Name "07-human-warm-small-files" -Exe $BindhubBin -CommandArgs @("warm", $ProductSparseTarget, "--max-bytes", "40") -Env @{ BINDHUB_CONFIG_DIR = $TargetConfig }
    Expect-Contains (Join-Path $EvidenceDir "07-human-warm-small-files.stdout.log") "Selected:"
    Expect-FileText (Join-Path $ProductSparseTarget "src\main.rs") "fn main() {}"
    Expect-FileText (Join-Path $ProductSparseTarget "config\app.toml") "debug=1"
    Expect-Absent (Join-Path $ProductSparseTarget "big.bin")
    Invoke-Logged -Name "08-human-keep-readme" -Exe $BindhubBin -CommandArgs @("keep", (Join-Path $ProductSparseTarget "README.md")) -Env @{ BINDHUB_CONFIG_DIR = $TargetConfig }
    Expect-Contains (Join-Path $EvidenceDir "08-human-keep-readme.stdout.log") "Kept for offline: README.md"
    New-TextFile (Join-Path $ProductSparseTarget "src\main.rs") "dirty local change"
    Invoke-Logged -Name "09-human-free-space-success" -Exe $BindhubBin -CommandArgs @("free-space", $ProductSparseTarget, "--max-bytes", "0") -Env @{ BINDHUB_CONFIG_DIR = $TargetConfig }
    Expect-Contains (Join-Path $EvidenceDir "09-human-free-space-success.stdout.log") "Safety: changed and kept files were left alone"
    Expect-Contains (Join-Path $EvidenceDir "09-human-free-space-success.stdout.log") "Skipped:"
    Expect-FileText (Join-Path $ProductSparseTarget "README.md") "hello"
    Expect-FileText (Join-Path $ProductSparseTarget "src\main.rs") "dirty local change"
    Expect-Absent (Join-Path $ProductSparseTarget "config\app.toml")
    Invoke-Logged -Name "10-human-status-after-free-space" -Exe $BindhubBin -CommandArgs @("status", $ProductSparseTarget) -Env @{ BINDHUB_CONFIG_DIR = $TargetConfig }
    Expect-Contains (Join-Path $EvidenceDir "10-human-status-after-free-space.stdout.log") "Kept offline:"
    Expect-Contains (Join-Path $EvidenceDir "10-human-status-after-free-space.stdout.log") "Changed locally:"

    $RefusalSource = Join-Path $WorkDir "human-free-space-refusal"
    New-TextFile (Join-Path $RefusalSource "README.md") "backed up proof"
    Invoke-Logged -Name "11-human-refusal-share" -Exe $BindhubBin -CommandArgs @("share", $RefusalSource, "--no-background-sync") -Env @{ BINDHUB_CONFIG_DIR = $SourceConfig }
    $ApiObjects = Join-Path $ApiRoot "objects"
    if (-not (Test-Path $ApiObjects)) {
        Fail "could not find hosted object directory for free-space refusal"
    }
    Remove-SmokePath $ApiObjects
    Invoke-Logged -Name "12-human-free-space-refusal" -Exe $BindhubBin -CommandArgs @("free-space", $RefusalSource, "--max-bytes", "0") -Env @{ BINDHUB_CONFIG_DIR = $SourceConfig } -ExpectFailure
    Expect-Contains (Join-Path $EvidenceDir "12-human-free-space-refusal.stderr.log") "not safely backed up"
    Expect-FileText (Join-Path $RefusalSource "README.md") "backed up proof"

    $AgentFolder = Join-Path $WorkDir "agent-workspace"
    New-TextFile (Join-Path $AgentFolder "README.md") "hello virtual"
    Invoke-Logged -Name "13-agent-track" -Exe $LoomBin -CommandArgs @("track", $AgentFolder)
    Invoke-Logged -Name "14-agent-open" -Exe $LoomBin -CommandArgs @("workspace", "open", $AgentFolder, "--session", "agent-smoke")
    Expect-Contains (Join-Path $EvidenceDir "14-agent-open.stdout.log") "Adapter: agent virtual"
    Invoke-Logged -Name "15-agent-read" -Exe $LoomBin -CommandArgs @("workspace", "read", $AgentFolder, "--session", "agent-smoke", "README.md")
    Expect-Contains (Join-Path $EvidenceDir "15-agent-read.stdout.log") "hello virtual"
    Invoke-Logged -Name "16-agent-virtual-exec" -Exe $LoomBin -CommandArgs @("workspace", "exec", $AgentFolder, "--session", "agent-smoke", "--", "cat", "README.md")
    Expect-Contains (Join-Path $EvidenceDir "16-agent-virtual-exec.stdout.log") "Mode: virtual"
    Expect-Contains (Join-Path $EvidenceDir "16-agent-virtual-exec.stdout.log") "hello virtual"
    Invoke-Logged -Name "17-agent-write-overlay" -Exe $LoomBin -CommandArgs @("workspace", "write", $AgentFolder, "--session", "agent-smoke", "notes\todo.txt", "--text", "agent overlay")
    Expect-Contains (Join-Path $EvidenceDir "17-agent-write-overlay.stdout.log") "Wrote overlay: notes/todo.txt"
    Invoke-Logged -Name "18-agent-diff" -Exe $LoomBin -CommandArgs @("workspace", "diff", $AgentFolder, "--session", "agent-smoke")
    Expect-Contains (Join-Path $EvidenceDir "18-agent-diff.stdout.log") "Changes: 1 created"
    Invoke-Logged -Name "19-agent-checkpoint" -Exe $LoomBin -CommandArgs @("workspace", "checkpoint", $AgentFolder, "--session", "agent-smoke", "-m", "agent overlay checkpoint")
    Expect-Contains (Join-Path $EvidenceDir "19-agent-checkpoint.stdout.log") "Boundary: sandbox-merge"
    Expect-Contains (Join-Path $EvidenceDir "19-agent-checkpoint.stdout.log") "Overlay files: 1"
    Invoke-Logged -Name "20-agent-open-discard" -Exe $LoomBin -CommandArgs @("workspace", "open", $AgentFolder, "--session", "agent-discard")
    Invoke-Logged -Name "21-agent-discard-write" -Exe $LoomBin -CommandArgs @("workspace", "write", $AgentFolder, "--session", "agent-discard", "scratch.txt", "--text", "discard me")
    Invoke-Logged -Name "22-agent-discard" -Exe $LoomBin -CommandArgs @("workspace", "discard", $AgentFolder, "--session", "agent-discard")
    Expect-Contains (Join-Path $EvidenceDir "22-agent-discard.stdout.log") "State: discarded"
    Expect-Contains (Join-Path $EvidenceDir "22-agent-discard.stdout.log") "Discarded overlay files: 1"

    $MaterializedFolder = Join-Path $WorkDir "materialized-workspace"
    New-TextFile (Join-Path $MaterializedFolder "README.md") "before materialized"
    Invoke-Logged -Name "23-materialized-track" -Exe $LoomBin -CommandArgs @("track", $MaterializedFolder)
    Invoke-Logged -Name "24-materialized-open" -Exe $LoomBin -CommandArgs @("workspace", "open", $MaterializedFolder, "--session", "materialized-safe")
    $safeCommand = @("cmd", "/C", "echo after materialized>README.md && mkdir src 2>NUL & echo new materialized file>src\new.txt")
    Invoke-Logged -Name "25-materialized-safe-run" -Exe $LoomBin -CommandArgs (@("workspace", "materialize-run", $MaterializedFolder, "--session", "materialized-safe", "--") + $safeCommand)
    Expect-Contains (Join-Path $EvidenceDir "25-materialized-safe-run.stdout.log") "Mode: materialized-sandbox"
    Expect-Contains (Join-Path $EvidenceDir "25-materialized-safe-run.stdout.log") "Captured: 2 changed"
    Invoke-Logged -Name "26-materialized-safe-diff" -Exe $LoomBin -CommandArgs @("workspace", "diff", $MaterializedFolder, "--session", "materialized-safe")
    Expect-Contains (Join-Path $EvidenceDir "26-materialized-safe-diff.stdout.log") "Changes: 1 created, 1 modified"
    Invoke-Logged -Name "27-materialized-open-unsafe" -Exe $LoomBin -CommandArgs @("workspace", "open", $MaterializedFolder, "--session", "materialized-unsafe")
    $LeakPath = Join-Path $MaterializedFolder "host-leak.txt"
    $EscapedLeakPath = $LeakPath -replace "'", "''"
    $unsafeCommand = @("powershell", "-NoProfile", "-Command", "Set-Content -LiteralPath '$EscapedLeakPath' -Value 'host mutation'")
    Invoke-Logged -Name "28-materialized-host-mutation-refusal" -Exe $LoomBin -CommandArgs (@("workspace", "materialize-run", $MaterializedFolder, "--session", "materialized-unsafe", "--") + $unsafeCommand) -ExpectFailure
    Expect-Contains (Join-Path $EvidenceDir "28-materialized-host-mutation-refusal.stderr.log") "mutated the real shared folder outside capture"
    Invoke-Logged -Name "29-materialized-unsafe-diff" -Exe $LoomBin -CommandArgs @("workspace", "diff", $MaterializedFolder, "--session", "materialized-unsafe")
    Expect-Contains (Join-Path $EvidenceDir "29-materialized-unsafe-diff.stdout.log") "Changes: 0 created, 0 modified"

    $FsFolder = Join-Path $WorkDir "fs-alpha"
    $NativeMount = Join-Path $WorkDir "native-view"
    $LocalDevMount = Join-Path $WorkDir "local-dev-view"
    New-TextFile (Join-Path $FsFolder "README.md") "hello fs"
    Invoke-Logged -Name "30-fs-track" -Exe $LoomBin -CommandArgs @("track", $FsFolder)
    Invoke-Logged -Name "31-fs-native-status" -Exe $LoomBin -CommandArgs @("fs", "status", $FsFolder)
    Expect-Contains (Join-Path $EvidenceDir "31-fs-native-status.stdout.log") "Can mount: no"
    Expect-Contains (Join-Path $EvidenceDir "31-fs-native-status.stdout.log") "Hydrate-on-open: no"
    Invoke-Logged -Name "32-fs-native-mount-refusal" -Exe $LoomBin -CommandArgs @("fs", "mount", $FsFolder, "--mount", $NativeMount) -ExpectFailure
    Expect-Contains (Join-Path $EvidenceDir "32-fs-native-mount-refusal.stderr.log") "adapter is unsupported for mount"
    Invoke-Logged -Name "33-fs-native-status-after-refusal" -Exe $LoomBin -CommandArgs @("fs", "status", $FsFolder)
    Expect-Contains (Join-Path $EvidenceDir "33-fs-native-status-after-refusal.stdout.log") "Mount records: none"
    Invoke-Logged -Name "34-fs-local-dev-mount" -Exe $LoomBin -CommandArgs @("fs", "mount", $FsFolder, "--adapter", "local-dev", "--mount", $LocalDevMount)
    Expect-Contains (Join-Path $EvidenceDir "34-fs-local-dev-mount.stdout.log") "Adapter: local-dev"
    Expect-Contains (Join-Path $EvidenceDir "34-fs-local-dev-mount.stdout.log") "Native OS integration: no"
    Expect-Contains (Join-Path $EvidenceDir "34-fs-local-dev-mount.stdout.log") "Hydrate-on-open: no"
    Expect-Contains (Join-Path $EvidenceDir "34-fs-local-dev-mount.stdout.log") "projection=simulated-metadata-only"
    Expect-Absent $LocalDevMount
    Invoke-Logged -Name "35-fs-local-dev-status-mounted" -Exe $LoomBin -CommandArgs @("fs", "status", $FsFolder, "--adapter", "local-dev", "--mount", $LocalDevMount)
    Expect-Contains (Join-Path $EvidenceDir "35-fs-local-dev-status-mounted.stdout.log") "state=mounted"
    Invoke-Logged -Name "36-fs-local-dev-unmount" -Exe $LoomBin -CommandArgs @("fs", "unmount", $FsFolder, "--adapter", "local-dev", "--mount", $LocalDevMount)
    Expect-Contains (Join-Path $EvidenceDir "36-fs-local-dev-unmount.stdout.log") "Unmount: recorded"
    Invoke-Logged -Name "37-fs-local-dev-status-unmounted" -Exe $LoomBin -CommandArgs @("fs", "status", $FsFolder, "--adapter", "local-dev", "--mount", $LocalDevMount)
    Expect-Contains (Join-Path $EvidenceDir "37-fs-local-dev-status-unmounted.stdout.log") "state=unmounted"

    @"
bindhub/Loom workspace adapter alpha smoke passed.

Workdir: $WorkDir
API: $ApiUrl

Proofs:
- Human sparse folders: bindhub login/share/sparse clone/status/hydrate/warm/keep/free-space success/refusal.
- Agent virtual workspaces: session open, virtual read/exec, overlay write, diff, checkpoint, and discard.
- Materialized fallback: real command changes captured into overlay; unsafe host shared-folder mutation refused.
- Filesystem adapter alpha: native adapters report unsupported/no hydrate-on-open and record no mount; local-dev records metadata-only mount/status/unmount and creates no projected folder.

Native OS hydrate-on-open, placeholder files, kernel callback hydration, chunk transport, and production filesystem drivers are intentionally not claimed here.

Evidence logs are in this directory. Session tokens and Bindhub URLs are redacted.
"@ | Set-Content -Path (Join-Path $EvidenceDir "SUMMARY.txt")

    Write-Host "workspace adapter smoke passed"
    Write-Host "evidence=$EvidenceDir"
    Write-Host ("summary=" + (Join-Path $EvidenceDir "SUMMARY.txt"))
} finally {
    if ($ApiProcess -and -not $ApiProcess.HasExited) {
        Stop-Process -Id $ApiProcess.Id -Force -ErrorAction SilentlyContinue
        $ApiProcess.WaitForExit()
    }
    if ($Cleanup -and (Test-Path $WorkDir)) {
        Remove-SmokePath $WorkDir
    }
}
