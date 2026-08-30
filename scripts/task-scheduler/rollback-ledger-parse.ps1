function Test-RollbackLedgerRecorded([string]$Stdout, [int]$ExitCode) {
    if ($ExitCode -ne 0) {
        return $false
    }
    try {
        $envelope = $Stdout.Trim() | ConvertFrom-Json
        if ($envelope.outcome -ne "ok") {
            return $false
        }
        if ($envelope.payload.data.recorded -eq $true) {
            return $true
        }
    } catch {
        return $false
    }
    return $false
}
