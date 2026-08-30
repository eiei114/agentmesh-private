# Roll back Production Controller scheduling by disabling the task and recording rollback metadata.
# Resume of legacy Multica Autopilots remains a separate human-owned step.

param(
    [Parameter(Mandatory = $true)]
    [string]$TaskName,

    [Parameter(Mandatory = $true)]
    [string]$CorrelationId,

    [Parameter(Mandatory = $true)]
    [string]$AgentMeshExe,

    [Parameter(Mandatory = $true)]
    [string]$LedgerManifestPath,

    [Parameter(Mandatory = $true)]
    [string]$ToolchainPin,

    [Parameter(Mandatory = $true)]
    [string]$ToolchainCache,

    [Parameter(Mandatory = $true)]
    [string]$SidecarDir,

    [Parameter(Mandatory = $true)]
    [string]$LedgerPath,

    [Parameter(Mandatory = $true)]
    [string]$ControllerId,

    [Parameter(Mandatory = $true)]
    [string]$Now,

    [string]$ReasonCode = "manual_rollback"
)

$ErrorActionPreference = "Stop"

function Test-RequiredFile([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Missing required file: $Path"
    }
}

function Test-RequiredDirectory([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
        throw "Missing required directory: $Path"
    }
}

if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
    Disable-ScheduledTask -TaskName $TaskName | Out-Null
    $status = "disabled"
} else {
    $status = "not_found"
}

$rollbackInputFile = $null
$ledgerExitCode = -1
$rollbackRecorded = $false
try {
    . (Join-Path $PSScriptRoot "rollback-ledger-parse.ps1")
    Test-RequiredFile -Path $AgentMeshExe
    Test-RequiredFile -Path $LedgerManifestPath
    Test-RequiredFile -Path $ToolchainPin
    Test-RequiredDirectory -Path $ToolchainCache
    Test-RequiredDirectory -Path $SidecarDir

    $eventId = "rollback-$CorrelationId"
    $rollbackInput = @{
        schema_version = "local-control-ledger-input.v0"
        operation = "record_rollback"
        ledger_path = $LedgerPath
        controller_id = $ControllerId
        event_id = $eventId
        reason_code = $ReasonCode
        correlation_id = $CorrelationId
        recorded_at = $Now
    } | ConvertTo-Json -Compress

    $rollbackInputDir = Join-Path $env:LOCALAPPDATA "AgentMesh\rollback-inputs"
    New-Item -ItemType Directory -Path $rollbackInputDir -Force | Out-Null
    $rollbackInputFile = Join-Path $rollbackInputDir ("rollback-$PID-$([Guid]::NewGuid().ToString('N')).json")
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($rollbackInputFile, $rollbackInput, $utf8NoBom)

    $arguments = @(
        "app",
        "run",
        "--manifest", $LedgerManifestPath,
        "--toolchain-pin", $ToolchainPin,
        "--toolchain-cache", $ToolchainCache,
        "--input", $rollbackInputFile,
        "--sidecar-dir", $SidecarDir,
        "--mode", "production"
    )
    $ledgerOutput = & $AgentMeshExe @arguments 2>&1
    $ledgerExitCode = $LASTEXITCODE
    $ledgerStdout = (@($ledgerOutput) | ForEach-Object { $_.ToString() }) -join "`n"
    $rollbackRecorded = Test-RollbackLedgerRecorded `
        -Stdout $ledgerStdout `
        -ExitCode $ledgerExitCode `
        -ExpectedEventId $eventId
} catch {
    $rollbackRecorded = $false
} finally {
    if ($rollbackInputFile) {
        Remove-Item -LiteralPath $rollbackInputFile -Force -ErrorAction SilentlyContinue
    }
}

Write-Output (@{
    task = $TaskName
    status = $status
    reason_code = $ReasonCode
    correlation_id = $CorrelationId
    rollback_recorded = $rollbackRecorded
    ledger_exit_code = $ledgerExitCode
} | ConvertTo-Json -Compress)

if (-not $rollbackRecorded) {
    exit 30
}
exit 0
