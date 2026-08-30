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

function Test-RequiredFile([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Missing required file: $Path"
    }
}

function Format-ScheduledTaskArgument([string]$Value) {
    if ($Value -match '[\s"]') {
        return ('"' + $Value.Replace('"', '""') + '"')
    }
    return $Value
}

. (Join-Path $PSScriptRoot "rollback-ledger-parse.ps1")

Test-RequiredFile -Path $AgentMeshExe
Test-RequiredFile -Path $LedgerManifestPath
Test-RequiredFile -Path $ToolchainPin

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

$argumentParts = @(
    'app',
    'run',
    '--manifest', $LedgerManifestPath,
    '--toolchain-pin', $ToolchainPin,
    '--input', $rollbackInputFile
) | ForEach-Object { Format-ScheduledTaskArgument $_ }

$argumentLine = $argumentParts -join ' '
$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $AgentMeshExe
$psi.Arguments = $argumentLine
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.UseShellExecute = $false
$ledgerProcess = [System.Diagnostics.Process]::Start($psi)
$ledgerStdout = $ledgerProcess.StandardOutput.ReadToEnd() + $ledgerProcess.StandardError.ReadToEnd()
$ledgerProcess.WaitForExit() | Out-Null
$ledgerExitCode = $ledgerProcess.ExitCode
$rollbackRecorded = Test-RollbackLedgerRecorded -Stdout $ledgerStdout -ExitCode $ledgerExitCode

Write-Output (@{
    task = $TaskName
    status = $status
    reason_code = $ReasonCode
    correlation_id = $CorrelationId
    rollback_recorded = $rollbackRecorded
    ledger_exit_code = $ledgerExitCode
} | ConvertTo-Json -Compress)
