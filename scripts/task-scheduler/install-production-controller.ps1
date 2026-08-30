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

foreach ($path in @($AgentMeshExe, $ManifestPath, $ToolchainPin, $InputJson)) {
    if (-not (Test-Path -LiteralPath $path)) {
        throw "Missing required path: $path"
    }
}

function Convert-Iso8601DurationToMinutes([string]$Duration) {
    if ($Duration -match '^PT(\d+)M$') {
        return [int]$Matches[1]
    }
    throw "Unsupported Schedule format: $Duration (expected PT<n>M)"
}

$intervalMinutes = Convert-Iso8601DurationToMinutes -Duration $Schedule

$quotedArgs = @(
    'app',
    'run',
    '--manifest', ('"' + $ManifestPath.Replace('"', '""') + '"'),
    '--toolchain-pin', ('"' + $ToolchainPin.Replace('"', '""') + '"'),
    '--input', ('"' + $InputJson.Replace('"', '""') + '"'),
    '--sidecar-dir', ('"' + $SidecarDir.Replace('"', '""') + '"')
) -join ' '

$action = New-ScheduledTaskAction -Execute ('"' + $AgentMeshExe.Replace('"', '""') + '"') -Argument $quotedArgs
$trigger = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(1) `
    -RepetitionInterval (New-TimeSpan -Minutes $intervalMinutes) `
    -RepetitionDuration ([TimeSpan]::MaxValue)
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -MultipleInstances IgnoreNew

Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Settings $settings -Force | Out-Null
Write-Output (@{ task = $TaskName; status = "installed"; schedule = $Schedule; interval_minutes = $intervalMinutes } | ConvertTo-Json -Compress)
