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
    if (-not (Test-Path -LiteralPath $path)) {
        throw "Missing required path: $path"
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

& $AgentMeshExe app run `
    --manifest $LedgerManifestPath `
    --toolchain-pin $ToolchainPin `
    --input $rollbackInputFile | Out-Null
$rollbackRecorded = $LASTEXITCODE -eq 0

Write-Output (@{
    task = $TaskName
    status = $status
    reason_code = $ReasonCode
    correlation_id = $CorrelationId
    rollback_recorded = $rollbackRecorded
    ledger_exit_code = $LASTEXITCODE
} | ConvertTo-Json -Compress)
