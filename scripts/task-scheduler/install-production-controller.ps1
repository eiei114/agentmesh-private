# Install a one-shot Production Controller task for local Windows scheduling.
# Does not configure live Multica credentials or run the task immediately.

param(
    [Parameter(Mandatory = $true)]
    [string]$AgentMeshExe,

    [Parameter(Mandatory = $true)]
    [string]$ManifestPath,

    [Parameter(Mandatory = $true)]
    [string]$ToolchainPin,

    [Parameter(Mandatory = $true)]
    [string]$InputJson,

    [Parameter(Mandatory = $true)]
    [string]$SidecarDir,

    [Parameter(Mandatory = $true)]
    [string]$TaskName,

    [string]$Schedule = "PT15M"
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

function Format-ScheduledTaskArgument([string]$Value) {
    if ($Value -match '[\s"]') {
        return ('"' + $Value.Replace('"', '""') + '"')
    }
    return $Value
}

Test-RequiredFile -Path $AgentMeshExe
Test-RequiredFile -Path $ManifestPath
Test-RequiredFile -Path $ToolchainPin
Test-RequiredFile -Path $InputJson
Test-RequiredDirectory -Path $SidecarDir

function Convert-Iso8601DurationToMinutes([string]$Duration) {
    if ($Duration -match '^PT(\d+)M$') {
        return [int]$Matches[1]
    }
    throw "Unsupported Schedule format: $Duration (expected PT<n>M)"
}

$intervalMinutes = Convert-Iso8601DurationToMinutes -Duration $Schedule

$argumentParts = @(
    'app',
    'run',
    '--manifest', $ManifestPath,
    '--toolchain-pin', $ToolchainPin,
    '--input', $InputJson,
    '--sidecar-dir', $SidecarDir
) | ForEach-Object { Format-ScheduledTaskArgument $_ }

$action = New-ScheduledTaskAction -Execute $AgentMeshExe -Argument ($argumentParts -join ' ')
$trigger = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(1) `
    -RepetitionInterval (New-TimeSpan -Minutes $intervalMinutes) `
    -RepetitionDuration ([TimeSpan]::MaxValue)
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -MultipleInstances IgnoreNew

Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Settings $settings -Force | Out-Null
Write-Output (@{ task = $TaskName; status = "installed"; schedule = $Schedule; interval_minutes = $intervalMinutes } | ConvertTo-Json -Compress)
