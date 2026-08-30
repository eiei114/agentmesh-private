function Test-RollbackLedgerRecorded(
    [string]$Stdout,
    [int]$ExitCode,
    [string]$ExpectedEventId = ""
) {
    if ($ExitCode -ne 0) {
        return $false
    }
    try {
        try {
            $envelope = $Stdout.Trim() | ConvertFrom-Json -ErrorAction Stop
        } catch {
            $compactLine = @($Stdout -split '\r?\n') |
                ForEach-Object { $_.Trim() } |
                Where-Object { $_ } |
                Select-Object -Last 1
            $envelope = $compactLine | ConvertFrom-Json -ErrorAction Stop
        }
        if ($envelope.outcome -ne "ok") {
            return $false
        }
        $payload = $envelope.payload
        return (
            $payload.schema_version -eq "local-control-ledger-output.v0" -and
            $payload.operation -eq "record_rollback" -and
            $payload.valid -eq $true -and
            $payload.exit_reason -eq "ok" -and
            $payload.data.recorded -eq $true -and
            ([string]::IsNullOrEmpty($ExpectedEventId) -or $payload.data.event_id -eq $ExpectedEventId)
        )
    } catch {
        return $false
    }
    return $false
}
