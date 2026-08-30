# Run one scheduled AgentMesh production controller occurrence.
# Materializes a private, unique input so idempotency suppresses duplicate
# delivery of one occurrence without suppressing later schedule intervals.

param(
    [Parameter(Mandatory = $true)]
    [string]$AgentMeshExe,

    [Parameter(Mandatory = $true)]
    [string]$ManifestPath,

    [Parameter(Mandatory = $true)]
    [string]$ToolchainPin,

    [Parameter(Mandatory = $true)]
    [string]$ToolchainCache,

    [Parameter(Mandatory = $true)]
    [string]$InputJson,

    [Parameter(Mandatory = $true)]
    [string]$SidecarDir,

    [Parameter(Mandatory = $true)]
    [string]$ScheduleAnchorUtc,

    [Parameter(Mandatory = $true)]
    [ValidateRange(1, 1440)]
    [int]$IntervalMinutes,

    # Deterministic test/recovery override. The installer never passes this.
    [string]$CurrentTimeUtc = "",

    # Regression-only failure injection. The installer never passes this.
    [switch]$SimulateInputWriteFailure
)

$ErrorActionPreference = "Stop"

function Test-LocalLedgerCompact([object]$Value, [string]$Operation) {
    if ($Value -isnot [System.Management.Automation.PSCustomObject]) { return $false }
    $required = @(
        "schema_version", "app_version", "ledger_schema_version", "operation",
        "valid", "exit_reason", "issue_count", "issues", "data"
    )
    foreach ($field in $required) {
        if (-not ($Value.PSObject.Properties.Name -contains $field)) { return $false }
    }
    return (
        $Value.schema_version -eq "local-control-ledger-output.v0" -and
        $Value.app_version -is [string] -and
        -not [string]::IsNullOrWhiteSpace($Value.app_version) -and
        $Value.ledger_schema_version -eq "2" -and
        $Value.operation -eq $Operation -and
        $Value.valid -is [bool] -and
        $Value.valid -eq $true -and
        $Value.exit_reason -eq "ok" -and
        $Value.issue_count -eq 0 -and
        $Value.issues -is [System.Array] -and
        $Value.issues.Count -eq 0 -and
        $Value.data -is [System.Management.Automation.PSCustomObject]
    )
}

foreach ($requiredFile in @($AgentMeshExe, $ManifestPath, $ToolchainPin, $InputJson)) {
    if (-not (Test-Path -LiteralPath $requiredFile -PathType Leaf)) {
        throw "Missing required file: $requiredFile"
    }
}
if (-not (Test-Path -LiteralPath $SidecarDir -PathType Container)) {
    throw "Missing required directory: $SidecarDir"
}
if (-not (Test-Path -LiteralPath $ToolchainCache -PathType Container)) {
    throw "Missing required directory: $ToolchainCache"
}

$inputTemplate = Get-Content -LiteralPath $InputJson -Raw -Encoding UTF8 | ConvertFrom-Json
if ($inputTemplate.schema_version -ne "production-controller-observer-input.v0") {
    throw "Input template must use production-controller-observer-input.v0"
}
if ($inputTemplate.operation -ne "run_once" -or -not $inputTemplate.controller_id) {
    throw "Input template must define run_once and controller_id"
}

try {
    $anchor = [DateTimeOffset]::Parse(
        $ScheduleAnchorUtc,
        [System.Globalization.CultureInfo]::InvariantCulture,
        [System.Globalization.DateTimeStyles]::RoundtripKind
    ).ToUniversalTime()
    $now = if ($CurrentTimeUtc) {
        [DateTimeOffset]::Parse(
            $CurrentTimeUtc,
            [System.Globalization.CultureInfo]::InvariantCulture,
            [System.Globalization.DateTimeStyles]::RoundtripKind
        ).ToUniversalTime()
    } else {
        [DateTimeOffset]::UtcNow
    }
} catch {
    throw "ScheduleAnchorUtc and CurrentTimeUtc must be RFC 3339 timestamps"
}

$elapsedSeconds = [Math]::Max(0, ($now - $anchor).TotalSeconds)
$occurrenceIndex = [Math]::Floor($elapsedSeconds / ($IntervalMinutes * 60))
$occurrenceAt = $anchor.AddMinutes($occurrenceIndex * $IntervalMinutes)
$occurrenceSeed = "$($inputTemplate.controller_id)`n$($occurrenceAt.ToString('o'))"
$sha256 = [System.Security.Cryptography.SHA256]::Create()
try {
    $occurrenceHash = -join ($sha256.ComputeHash(
        [System.Text.Encoding]::UTF8.GetBytes($occurrenceSeed)
    ) | ForEach-Object { $_.ToString("x2") })
} finally {
    $sha256.Dispose()
}
$occurrenceId = $occurrenceHash.Substring(0, 32)
$inputTemplate.now = $now.ToString("o")
if ($inputTemplate.PSObject.Properties.Name -contains "occurrence_id") {
    $inputTemplate.occurrence_id = $occurrenceId
} else {
    $inputTemplate | Add-Member -NotePropertyName occurrence_id -NotePropertyValue $occurrenceId
}
$leaseId = "lease-$occurrenceId"
if ($inputTemplate.PSObject.Properties.Name -contains "lease_id") {
    $inputTemplate.lease_id = $leaseId
} else {
    $inputTemplate | Add-Member -NotePropertyName lease_id -NotePropertyValue $leaseId
}

$runInput = $null
try {
    $inputDir = Join-Path $env:LOCALAPPDATA "AgentMesh\scheduled-inputs"
    New-Item -ItemType Directory -Path $inputDir -Force | Out-Null
    $runInput = Join-Path $inputDir "observer-$occurrenceId-$PID.json"
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    if ($SimulateInputWriteFailure) {
        [System.IO.File]::WriteAllText($runInput, "partial", $utf8NoBom)
        throw "simulated scheduled input write failure"
    }
    [System.IO.File]::WriteAllText(
        $runInput,
        ($inputTemplate | ConvertTo-Json -Depth 16 -Compress),
        $utf8NoBom
    )

    $arguments = @(
        "app",
        "run",
        "--manifest", $ManifestPath,
        "--toolchain-pin", $ToolchainPin,
        "--toolchain-cache", $ToolchainCache,
        "--input", $runInput,
        "--sidecar-dir", $SidecarDir
    )
    $output = & $AgentMeshExe @arguments 2>&1
    $exitCode = $LASTEXITCODE
    $output | ForEach-Object { Write-Output $_ }
    if ($exitCode -ne 0) {
        exit $exitCode
    }

    $compactLine = @($output) |
        ForEach-Object { $_.ToString().Trim() } |
        Where-Object { $_ } |
        Select-Object -Last 1
    try {
        $envelope = $compactLine | ConvertFrom-Json -ErrorAction Stop
    } catch {
        Write-Output '{"runner_error":"invalid_compact_envelope"}'
        exit 20
    }
    $payload = $envelope.payload
    $requiredPayloadFields = @(
        "schema_version", "app_version", "operation", "valid", "exit_reason",
        "mutation_performed", "issue_count", "issues", "cli", "ledger"
    )
    $payloadIsObject = $payload -is [System.Management.Automation.PSCustomObject]
    $requiredFieldsPresent = $payloadIsObject
    if ($payloadIsObject) {
        foreach ($field in $requiredPayloadFields) {
            if (-not ($payload.PSObject.Properties.Name -contains $field)) {
                $requiredFieldsPresent = $false
                break
            }
        }
    }
    $issueCountIsInteger = $payload.issue_count -is [int] -or $payload.issue_count -is [long]
    $issueCountMatches = $payload.issues -is [System.Array] -and $payload.issue_count -eq $payload.issues.Count
    $commonValid = (
        $envelope.outcome -eq "ok" -and
        $requiredFieldsPresent -and
        $payload.schema_version -eq "production-controller-observer-output.v0" -and
        $payload.app_version -is [string] -and
        -not [string]::IsNullOrWhiteSpace($payload.app_version) -and
        $payload.operation -eq "run_once" -and
        $payload.valid -is [bool] -and
        $payload.exit_reason -is [string] -and
        $payload.mutation_performed -is [bool] -and
        $payload.mutation_performed -eq $false -and
        $issueCountIsInteger -and
        $payload.issue_count -ge 0 -and
        $issueCountMatches -and
        $payload.ledger -is [System.Management.Automation.PSCustomObject]
    )
    $cli = $payload.cli
    $cliExitCodeIsInteger = $cli.exit_code -is [int] -or $cli.exit_code -is [long]
    $cliStdoutCountIsInteger = $cli.stdout_byte_count -is [int] -or $cli.stdout_byte_count -is [long]
    $cliStderrCountIsInteger = $cli.stderr_byte_count -is [int] -or $cli.stderr_byte_count -is [long]
    $cliSuccess = (
        $cli -is [System.Management.Automation.PSCustomObject] -and
        $cli.schema_version -eq "multica-cli-adapter-output.v0" -and
        $cli.operation -eq "query" -and
        $cli.valid -is [bool] -and $cli.valid -eq $true -and
        $cli.exit_reason -eq "query_ok" -and
        $cliExitCodeIsInteger -and $cli.exit_code -eq 0 -and
        $cli.stdout_sha256 -is [string] -and $cli.stdout_sha256 -match '^sha256:[0-9a-f]{64}$' -and
        $cliStdoutCountIsInteger -and $cli.stdout_byte_count -ge 0 -and
        $cli.stdout_truncated -is [bool] -and $cli.stdout_truncated -eq $false -and
        $cliStderrCountIsInteger -and $cli.stderr_byte_count -ge 0 -and
        $cli.json_parse_ok -is [bool] -and $cli.json_parse_ok -eq $true -and
        $cli.json_top_level_kind -eq "object" -and
        $cli.timed_out -is [bool] -and $cli.timed_out -eq $false
    )
    $ledger = $payload.ledger
    $decisionOk = Test-LocalLedgerCompact -Value $ledger.decision -Operation "record_decision"
    $watermarkOk = Test-LocalLedgerCompact -Value $ledger.watermark -Operation "set_watermark"
    $authorityOk = Test-LocalLedgerCompact -Value $ledger.authority -Operation "get_authority_mode"
    $idempotencyOk = Test-LocalLedgerCompact -Value $ledger.idempotency -Operation "claim_idempotency"
    $ledgerSuccess = (
        $decisionOk -and $ledger.decision.data.recorded -eq $true -and
        $watermarkOk -and $ledger.watermark.data.watermark_key -eq "last_observer_run" -and
        $authorityOk -and @("shadow", "observer") -contains $ledger.authority.data.authority_mode -and
        $idempotencyOk -and $ledger.idempotency.data.claimed -eq $true -and
        $ledger.idempotency.data.duplicate -eq $false
    )
    $observerSuccess = (
        $commonValid -and
        $payload.valid -eq $true -and
        $payload.exit_reason -eq "observer_success_no_mutation" -and
        $cliSuccess -and
        $ledgerSuccess
    )
    if (-not $observerSuccess) {
        Write-Output (@{
            runner_error = "observer_compact_failure"
            exit_reason = [string]$payload.exit_reason
        } | ConvertTo-Json -Compress)
        exit 21
    }
    exit 0
} finally {
    if ($runInput) {
        Remove-Item -LiteralPath $runInput -Force -ErrorAction SilentlyContinue
    }
}
