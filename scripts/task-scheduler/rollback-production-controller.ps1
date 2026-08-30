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
    [string]$LedgerPath,

    [Parameter(Mandatory = $true)]
    [string]$ControllerId,

    [Parameter(Mandatory = $true)]
    [string]$Now,

    [string]$ReasonCode = "manual_rollback"
)

$ErrorActionPreference = "Stop"

foreach ($path in @($AgentMeshExe, $LedgerManifestPath, $ToolchainPin)) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Missing required file: $path"
    }
}

if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
    Disable-ScheduledTask -TaskName $TaskName | Out-Null
    $status = "disabled"
} else {
    $status = "not_found"
}

$rollbackInput = @{
    schema_version = "local-control-ledger-input.v0"
    operation = "record_rollback"
    ledger_path = $LedgerPath
    controller_id = $ControllerId
    event_id = "rollback-$CorrelationId"
    reason_code = $ReasonCode
    correlation_id = $CorrelationId
    recorded_at = $Now
} | ConvertTo-Json -Compress

$rollbackInputFile = Join-Path $env:TEMP ("agentmesh-rollback-input-$CorrelationId.json")
Set-Content -LiteralPath $rollbackInputFile -Value $rollbackInput -Encoding UTF8

$ledgerStdout = & $AgentMeshExe app run `
    --manifest $LedgerManifestPath `
    --toolchain-pin $ToolchainPin `
    --input $rollbackInputFile 2>&1 | Out-String
$ledgerExitCode = $LASTEXITCODE
$rollbackRecorded = $false
try {
    $ledgerPayload = $ledgerStdout.Trim() | ConvertFrom-Json
    if ($ledgerPayload.data.recorded -eq $true) {
        $rollbackRecorded = $true
    }
} catch {
    $rollbackRecorded = $false
}

Write-Output (@{
    task = $TaskName
    status = $status
    reason_code = $ReasonCode
    correlation_id = $CorrelationId
    rollback_recorded = $rollbackRecorded
    ledger_exit_code = $ledgerExitCode
} | ConvertTo-Json -Compress)
