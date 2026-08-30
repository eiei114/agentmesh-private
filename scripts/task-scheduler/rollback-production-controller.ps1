# Roll back Production Controller scheduling by disabling the task and recording rollback metadata.
# Resume of legacy Multica Autopilots remains a separate human-owned step.

param(
    [Parameter(Mandatory = $true)]
    [string]$TaskName,

    [Parameter(Mandatory = $true)]
    [string]$CorrelationId,

    [string]$ReasonCode = "manual_rollback"
)

$ErrorActionPreference = "Stop"

if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
    Disable-ScheduledTask -TaskName $TaskName | Out-Null
    $status = "disabled"
} else {
    $status = "not_found"
}

Write-Output (@{
    task = $TaskName
    status = $status
    reason_code = $ReasonCode
    correlation_id = $CorrelationId
    rollback_recorded = $true
} | ConvertTo-Json -Compress)
