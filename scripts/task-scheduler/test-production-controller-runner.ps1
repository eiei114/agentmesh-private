$ErrorActionPreference = "Stop"

$runner = Join-Path $PSScriptRoot "run-production-controller.ps1"
$installer = Join-Path $PSScriptRoot "install-production-controller.ps1"
$root = Join-Path ([System.IO.Path]::GetTempPath()) ("agentmesh-runner-test-" + [Guid]::NewGuid().ToString("N"))
$oldLocalAppData = $env:LOCALAPPDATA

function Get-TestSha256([string]$Path) {
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    $stream = [System.IO.File]::OpenRead((Resolve-Path -LiteralPath $Path).Path)
    try {
        return -join ($sha256.ComputeHash($stream) | ForEach-Object { $_.ToString("x2") })
    } finally {
        $stream.Dispose()
        $sha256.Dispose()
    }
}

try {
    $localAppData = Join-Path $root "local-app-data"
    $sidecar = Join-Path $root "sidecars"
    New-Item -ItemType Directory -Path $localAppData, $sidecar -Force | Out-Null
    $env:LOCALAPPDATA = $localAppData
    $env:AGENTMESH_TEST_CAPTURE = Join-Path $root "captured-inputs.jsonl"

    $appRoot = Join-Path $root "source-app"
    $schemaRoot = Join-Path $appRoot "schemas"
    New-Item -ItemType Directory -Path $schemaRoot -Force | Out-Null
    $manifest = Join-Path $appRoot "agentmesh-app.toml"
    $ledgerAppRoot = Join-Path $root "source-ledger-app"
    $ledgerSchemaRoot = Join-Path $ledgerAppRoot "schemas"
    New-Item -ItemType Directory -Path $ledgerSchemaRoot -Force | Out-Null
    $ledgerManifest = Join-Path $ledgerAppRoot "agentmesh-app.toml"
    $pin = Join-Path $root "pin.toml"
    $template = Join-Path $root "input.json"
    $fakeAgentMesh = Join-Path $root "fake-agentmesh.ps1"
    Set-Content -LiteralPath $manifest -Value "test" -Encoding UTF8
    Set-Content -LiteralPath (Join-Path $schemaRoot "input.schema.json") -Value "{}" -Encoding UTF8
    Set-Content -LiteralPath $ledgerManifest -Value "test-ledger" -Encoding UTF8
    Set-Content -LiteralPath (Join-Path $ledgerSchemaRoot "input.schema.json") -Value "{}" -Encoding UTF8
    $toolchainCache = Join-Path $root "source-toolchain-cache"
    $pinTag = "v0.2.0-dev.test"
    $pinTarget = "x86_64-pc-windows-msvc"
    $pinCommit = "d" * 40
    $pinnedToolchain = Join-Path (Join-Path $toolchainCache $pinTag) $pinTarget
    $pinnedBin = Join-Path $pinnedToolchain "bin"
    New-Item -ItemType Directory -Path $pinnedBin -Force | Out-Null
    $observerBinary = Join-Path $pinnedBin "agentmesh-production-controller-observer.exe"
    Set-Content -LiteralPath $observerBinary -Value "fixture-observer" -Encoding UTF8
    $observerSha = Get-TestSha256 -Path $observerBinary
    $ledgerBinary = Join-Path $pinnedBin "agentmesh-local-control-ledger.exe"
    Set-Content -LiteralPath $ledgerBinary -Value "fixture-ledger" -Encoding UTF8
    $ledgerSha = Get-TestSha256 -Path $ledgerBinary
    $releaseManifest = Join-Path $pinnedToolchain "release-manifest.json"
    @{
        schema_version = "production-controller-observer-input.v0"
        operation = "run_once"
        controller_id = ("c" * 128)
        authority_mode = "observer"
        ledger_path = (Join-Path $root "control.db")
        cli_path = $fakeAgentMesh
        now = "2000-01-01T00:00:00Z"
        scope_key = "runner-test"
    } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $template -Encoding UTF8

    @'
param([Parameter(ValueFromRemainingArguments = $true)][string[]]$Remaining)
$inputIndex = [Array]::IndexOf($Remaining, "--input")
if ($inputIndex -lt 0 -or $inputIndex + 1 -ge $Remaining.Count) {
    throw "--input not provided"
}
$line = (Get-Content -LiteralPath $Remaining[$inputIndex + 1] -Raw -Encoding UTF8).Trim()
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::AppendAllText($env:AGENTMESH_TEST_CAPTURE, $line + "`n", $utf8NoBom)
$runInput = $line | ConvertFrom-Json
if ($runInput.operation -eq "record_rollback") {
    if ($env:AGENTMESH_TEST_ROLLBACK_RESULT -eq "process-failure") {
        Write-Output '{"outcome":"error","payload":{}}'
        $global:LASTEXITCODE = 9
        return
    }
    if ($env:AGENTMESH_TEST_ROLLBACK_RESULT -eq "malformed") {
        Write-Output '{"outcome":"ok","payload":{"data":{"recorded":false}}}'
        $global:LASTEXITCODE = 0
        return
    }
    $ledgerPayload = @{
        schema_version = "local-control-ledger-output.v0"
        app_version = "local-control-ledger.v0"
        ledger_schema_version = "2"
        operation = "record_rollback"
        valid = $true
        exit_reason = "ok"
        issue_count = 0
        issues = @()
        data = @{ event_id = $runInput.event_id; recorded = $true }
    }
    Write-Output (@{ outcome = "ok"; payload = $ledgerPayload } | ConvertTo-Json -Depth 12 -Compress)
    $global:LASTEXITCODE = 0
    return
}
if ($env:AGENTMESH_TEST_RESULT -eq "malformed-duplicate") {
    Write-Output '{"outcome":"ok","payload":{"exit_reason":"duplicate_suppressed","mutation_performed":false}}'
    exit 0
}
function New-LedgerCompact([string]$Operation, [hashtable]$Data) {
    return @{
        schema_version = "local-control-ledger-output.v0"
        app_version = "local-control-ledger.v0"
        ledger_schema_version = "2"
        operation = $Operation
        valid = $true
        exit_reason = "ok"
        issue_count = 0
        issues = @()
        data = $Data
    }
}
$payload = @{
    schema_version = "production-controller-observer-output.v0"
    app_version = "test.v0"
    operation = "run_once"
    valid = $true
    exit_reason = "observer_success_no_mutation"
    mutation_performed = $false
    issue_count = 0
    issues = @()
    cli = @{
        schema_version = "multica-cli-adapter-output.v0"
        operation = "query"
        valid = $true
        exit_reason = "query_ok"
        exit_code = 0
        stdout_sha256 = ("sha256:" + ("a" * 64))
        stdout_byte_count = 2
        stdout_truncated = $false
        stderr_byte_count = 0
        json_parse_ok = $true
        json_top_level_kind = "object"
        timed_out = $false
    }
    ledger = @{
        decision = New-LedgerCompact "record_decision" @{ recorded = $true }
        watermark = New-LedgerCompact "set_watermark" @{ watermark_key = "last_observer_run" }
        authority = New-LedgerCompact "get_authority_mode" @{ authority_mode = "shadow" }
        idempotency = New-LedgerCompact "claim_idempotency" @{ claimed = $true; duplicate = $false }
    }
}
if ($env:AGENTMESH_TEST_RESULT -eq "invalid") {
    $payload.valid = $false
    $payload.exit_reason = "cli_nonzero_exit"
    $payload.issue_count = 1
    $payload.issues = @(@{ code = "cli_nonzero_exit" })
} elseif ($env:AGENTMESH_TEST_RESULT -eq "duplicate") {
    $payload.valid = $false
    $payload.exit_reason = "duplicate_suppressed"
    $payload.issue_count = 1
    $payload.issues = @(@{ code = "duplicate_suppressed" })
    $payload.cli = $null
    $payload.ledger = @{ idempotency = New-LedgerCompact "claim_idempotency" @{ claimed = $false; duplicate = $true } }
} elseif ($env:AGENTMESH_TEST_RESULT -eq "contradictory") {
    $payload.cli.timed_out = $true
}
Write-Output (@{ outcome = "ok"; payload = $payload } | ConvertTo-Json -Depth 16 -Compress)
$global:LASTEXITCODE = 0
'@ | Set-Content -LiteralPath $fakeAgentMesh -Encoding UTF8

    $pinnedHost = Join-Path $pinnedBin "agentmesh.ps1"
    Copy-Item -LiteralPath $fakeAgentMesh -Destination $pinnedHost
    $hostSha = Get-TestSha256 -Path $pinnedHost
    @{
        schema_version = "agentmesh-release-manifest.v0"
        tag = $pinTag
        commit_sha = $pinCommit
        target = $pinTarget
        protocol_version = "2026-07-15"
        binaries = @{
            "agentmesh" = @{
                relative_path = "bin/agentmesh.ps1"
                sha256 = $hostSha
            }
            "agentmesh-production-controller-observer" = @{
                relative_path = "bin/agentmesh-production-controller-observer.exe"
                sha256 = $observerSha
            }
            "agentmesh-local-control-ledger" = @{
                relative_path = "bin/agentmesh-local-control-ledger.exe"
                sha256 = $ledgerSha
            }
        }
    } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $releaseManifest -Encoding UTF8
    $releaseManifestSha = Get-TestSha256 -Path $releaseManifest
    @(
        'schema_version = "agentmesh-toolchain-pin.v0"'
        "tag = `"$pinTag`""
        "commit_sha = `"$pinCommit`""
        "target = `"$pinTarget`""
        "release_manifest_sha256 = `"$releaseManifestSha`""
    ) | Set-Content -LiteralPath $pin -Encoding UTF8

    $baseArgs = @(
        "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
        "-File", $runner,
        "-AgentMeshExe", $fakeAgentMesh,
        "-ManifestPath", $manifest,
        "-ToolchainPin", $pin,
        "-ToolchainCache", $toolchainCache,
        "-InputJson", $template,
        "-SidecarDir", $sidecar,
        "-ScheduleAnchorUtc", "2026-08-30T00:00:00Z",
        "-IntervalMinutes", "15"
    )
    foreach ($current in @(
        "2026-08-30T00:01:00Z",
        "2026-08-30T00:14:59Z",
        "2026-08-30T00:15:00Z"
    )) {
        & powershell.exe @baseArgs "-CurrentTimeUtc" $current | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "runner failed for $current with exit $LASTEXITCODE"
        }
    }

    $captured = @(Get-Content -LiteralPath $env:AGENTMESH_TEST_CAPTURE -Encoding UTF8 | ForEach-Object { $_ | ConvertFrom-Json })
    if ($captured.Count -ne 3) { throw "expected three captured inputs" }
    if ($captured[0].occurrence_id -ne $captured[1].occurrence_id) {
        throw "same schedule occurrence did not retain occurrence_id"
    }
    if ($captured[0].lease_id -ne $captured[1].lease_id) {
        throw "same schedule occurrence did not retain lease_id"
    }
    if ($captured[1].occurrence_id -eq $captured[2].occurrence_id) {
        throw "next schedule occurrence reused occurrence_id"
    }
    foreach ($item in $captured) {
        if ($item.occurrence_id -notmatch '^[0-9a-f]{32}$') {
            throw "occurrence_id is not fixed-length lowercase SHA-256 prefix"
        }
        if ($item.lease_id -notmatch '^lease-[0-9a-f]{32}$' -or $item.lease_id.Length -gt 128) {
            throw "lease_id is not fixed-length and ledger-safe"
        }
        if ($item.controller_id.Length -ne 128) {
            throw "max-length controller_id was not preserved"
        }
    }
    $remainingInputs = @(Get-ChildItem -LiteralPath (Join-Path $localAppData "AgentMesh\scheduled-inputs") -Filter "*.json" -ErrorAction SilentlyContinue)
    if ($remainingInputs.Count -ne 0) { throw "ephemeral scheduled inputs were not removed" }

    $savedErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & powershell.exe @baseArgs "-CurrentTimeUtc" "2026-08-30T00:15:00Z" "-SimulateInputWriteFailure" 2>&1 | Out-Null
    $writeFailureExitCode = $LASTEXITCODE
    $ErrorActionPreference = $savedErrorActionPreference
    if ($writeFailureExitCode -eq 0) { throw "simulated scheduled input write failure reported success" }
    $inputsAfterWriteFailure = @(Get-ChildItem -LiteralPath (Join-Path $localAppData "AgentMesh\scheduled-inputs") -Filter "*.json" -ErrorAction SilentlyContinue)
    if ($inputsAfterWriteFailure.Count -ne 0) { throw "partial scheduled input survived write failure" }

    $env:AGENTMESH_TEST_RESULT = "duplicate"
    & powershell.exe @baseArgs "-CurrentTimeUtc" "2026-08-30T00:15:01Z" | Out-Null
    if ($LASTEXITCODE -eq 0) { throw "duplicate claim without completion receipt reported success" }

    $env:AGENTMESH_TEST_RESULT = "invalid"
    & powershell.exe @baseArgs "-CurrentTimeUtc" "2026-08-30T00:30:00Z" | Out-Null
    if ($LASTEXITCODE -eq 0) { throw "invalid observer compact result reported success" }

    $env:AGENTMESH_TEST_RESULT = "malformed-duplicate"
    & powershell.exe @baseArgs "-CurrentTimeUtc" "2026-08-30T00:30:01Z" | Out-Null
    if ($LASTEXITCODE -eq 0) { throw "malformed duplicate compact result reported success" }

    $env:AGENTMESH_TEST_RESULT = "contradictory"
    & powershell.exe @baseArgs "-CurrentTimeUtc" "2026-08-30T00:30:02Z" | Out-Null
    if ($LASTEXITCODE -eq 0) { throw "contradictory success compact result reported success" }

    $prepareArgs = @{
        AgentMeshExe = $fakeAgentMesh
        ManifestPath = $manifest
        LedgerManifestPath = $ledgerManifest
        ToolchainPin = $pin
        ToolchainCache = $toolchainCache
        InputJson = $template
        SidecarDir = $sidecar
        TaskName = "AgentMesh-Runner-Test"
        Schedule = "PT15M"
        PrepareOnly = $true
    }
    $prepared = (& $installer @prepareArgs | Select-Object -Last 1) | ConvertFrom-Json
    if ($prepared.status -ne "prepared") { throw "installer did not prepare durable assets" }
    if (-not $prepared.asset_dir.StartsWith((Join-Path $localAppData "AgentMesh\scheduler-assets"))) {
        throw "durable assets escaped owner-local scheduler root"
    }
    foreach ($path in @($prepared.agentmesh_exe, $prepared.runner, $prepared.rollback_script, $prepared.rollback_parser, $prepared.uninstall_script, $prepared.manifest, $prepared.ledger_manifest, $prepared.toolchain_pin, $prepared.input_template)) {
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            throw "prepared asset missing: $path"
        }
    }
    if (-not (Test-Path -LiteralPath $prepared.toolchain_cache -PathType Container)) {
        throw "prepared toolchain cache missing"
    }
    $durableReleaseManifest = Join-Path (Join-Path (Join-Path $prepared.toolchain_cache $pinTag) $pinTarget) "release-manifest.json"
    if (-not (Test-Path -LiteralPath $durableReleaseManifest -PathType Leaf)) {
        throw "prepared pinned release manifest missing"
    }
    $preparedAgain = (& $installer @prepareArgs | Select-Object -Last 1) | ConvertFrom-Json
    if ($preparedAgain.asset_hash -ne $prepared.asset_hash) {
        throw "same scheduler assets did not reuse immutable content hash"
    }

    $assetCountBeforeInvalidSchedules = @(Get-ChildItem -LiteralPath (Join-Path $localAppData "AgentMesh\scheduler-assets") -Directory).Count
    foreach ($invalidSchedule in @("PT0M", "PT1441M")) {
        $rejected = $false
        $invalidArgs = $prepareArgs.Clone()
        $invalidArgs.Schedule = $invalidSchedule
        try {
            & $installer @invalidArgs | Out-Null
        } catch {
            $rejected = $true
        }
        if (-not $rejected) { throw "invalid schedule was accepted: $invalidSchedule" }
    }
    $assetCountAfterInvalidSchedules = @(Get-ChildItem -LiteralPath (Join-Path $localAppData "AgentMesh\scheduler-assets") -Directory).Count
    if ($assetCountAfterInvalidSchedules -ne $assetCountBeforeInvalidSchedules) {
        throw "invalid schedule staged scheduler assets"
    }

    $tamperedHost = Join-Path $root "tampered-agentmesh.ps1"
    Copy-Item -LiteralPath $fakeAgentMesh -Destination $tamperedHost
    Add-Content -LiteralPath $tamperedHost -Value "# tampered"
    $tamperedArgs = $prepareArgs.Clone()
    $tamperedArgs.AgentMeshExe = $tamperedHost
    $tamperedRejected = $false
    try {
        & $installer @tamperedArgs | Out-Null
    } catch {
        $tamperedRejected = $true
    }
    if (-not $tamperedRejected) { throw "host not pinned by release manifest was accepted" }

    # Durable runner remains operable after source extraction/cache deletion.
    Remove-Item -LiteralPath $appRoot, $ledgerAppRoot, $toolchainCache -Recurse -Force
    Remove-Item -LiteralPath $fakeAgentMesh, $pin, $template -Force
    $env:AGENTMESH_TEST_RESULT = "success"
    & powershell.exe `
        -NoProfile -NonInteractive -ExecutionPolicy Bypass `
        -File $prepared.runner `
        -AgentMeshExe $prepared.agentmesh_exe `
        -ManifestPath $prepared.manifest `
        -ToolchainPin $prepared.toolchain_pin `
        -ToolchainCache $prepared.toolchain_cache `
        -InputJson $prepared.input_template `
        -SidecarDir $sidecar `
        -ScheduleAnchorUtc "2026-08-30T00:00:00Z" `
        -IntervalMinutes 15 `
        -CurrentTimeUtc "2026-08-30T00:45:00Z" | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "durable runner failed after source/cache deletion with exit $LASTEXITCODE"
    }

    $rollbackHarness = Join-Path $root "invoke-rollback-with-task-mocks.ps1"
    @'
param(
    [Parameter(Mandatory = $true)][string]$RollbackScript,
    [Parameter(Mandatory = $true)][string]$TaskName,
    [Parameter(Mandatory = $true)][string]$CorrelationId,
    [Parameter(Mandatory = $true)][string]$AgentMeshExe,
    [Parameter(Mandatory = $true)][string]$LedgerManifestPath,
    [Parameter(Mandatory = $true)][string]$ToolchainPin,
    [Parameter(Mandatory = $true)][string]$ToolchainCache,
    [Parameter(Mandatory = $true)][string]$SidecarDir,
    [Parameter(Mandatory = $true)][string]$LedgerPath,
    [Parameter(Mandatory = $true)][string]$ControllerId,
    [Parameter(Mandatory = $true)][string]$Now
)
function global:Get-ScheduledTask {
    [CmdletBinding()]
    param([string]$TaskName)
    return [pscustomobject]@{ TaskName = $TaskName }
}
function global:Disable-ScheduledTask {
    [CmdletBinding()]
    param([string]$TaskName)
    return [pscustomobject]@{ TaskName = $TaskName; State = "Disabled" }
}
& $RollbackScript `
    -TaskName $TaskName `
    -CorrelationId $CorrelationId `
    -AgentMeshExe $AgentMeshExe `
    -LedgerManifestPath $LedgerManifestPath `
    -ToolchainPin $ToolchainPin `
    -ToolchainCache $ToolchainCache `
    -SidecarDir $SidecarDir `
    -LedgerPath $LedgerPath `
    -ControllerId $ControllerId `
    -Now $Now
exit $LASTEXITCODE
'@ | Set-Content -LiteralPath $rollbackHarness -Encoding UTF8
    $rollbackArgs = @(
        "-TaskName", "AgentMesh-Runner-Test",
        "-CorrelationId", "corr-1",
        "-AgentMeshExe", $prepared.agentmesh_exe,
        "-LedgerManifestPath", $prepared.ledger_manifest,
        "-ToolchainPin", $prepared.toolchain_pin,
        "-ToolchainCache", $prepared.toolchain_cache,
        "-SidecarDir", $sidecar,
        "-LedgerPath", (Join-Path $root "control.db"),
        "-ControllerId", "runner-test",
        "-Now", "2026-08-30T00:46:00Z"
    )
    $env:AGENTMESH_TEST_ROLLBACK_RESULT = "success"
    $rollbackOutput = & powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $rollbackHarness -RollbackScript $prepared.rollback_script @rollbackArgs
    if ($LASTEXITCODE -ne 0) { throw "durable rollback receipt failed with exit $LASTEXITCODE" }
    $rollbackResult = ($rollbackOutput | Select-Object -Last 1) | ConvertFrom-Json
    if ($rollbackResult.rollback_recorded -ne $true -or $rollbackResult.status -ne "disabled") {
        throw "durable rollback did not report disabled + recorded"
    }

    foreach ($failureMode in @("malformed", "process-failure")) {
        $env:AGENTMESH_TEST_ROLLBACK_RESULT = $failureMode
        & powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $rollbackHarness -RollbackScript $prepared.rollback_script @rollbackArgs | Out-Null
        if ($LASTEXITCODE -eq 0) { throw "rollback failure mode reported success: $failureMode" }
    }
    $remainingRollbackInputs = @(Get-ChildItem -LiteralPath (Join-Path $localAppData "AgentMesh\rollback-inputs") -Filter "*.json" -ErrorAction SilentlyContinue)
    if ($remainingRollbackInputs.Count -ne 0) { throw "ephemeral rollback inputs were not removed" }

    Write-Output "scheduler runner tests passed"
} finally {
    $env:LOCALAPPDATA = $oldLocalAppData
    Remove-Item Env:AGENTMESH_TEST_CAPTURE -ErrorAction SilentlyContinue
    Remove-Item Env:AGENTMESH_TEST_RESULT -ErrorAction SilentlyContinue
    Remove-Item Env:AGENTMESH_TEST_ROLLBACK_RESULT -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}
