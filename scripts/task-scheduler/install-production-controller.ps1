# Install a one-shot Production Controller Observer task for local Windows scheduling.
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

$action = New-ScheduledTaskAction -Execute $AgentMeshExe -Argument @(
    "app", "run",
    "--manifest", $ManifestPath,
    "--toolchain-pin", $ToolchainPin,
    "--input", $InputJson,
    "--sidecar-dir", $SidecarDir
) -Join ' '

$trigger = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(1) -RepetitionInterval (New-TimeSpan -Minutes 15) -RepetitionDuration ([TimeSpan]::MaxValue)
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -MultipleInstances IgnoreNew

Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Settings $settings -Force | Out-Null
Write-Output (@{ task = $TaskName; status = "installed"; schedule = $Schedule } | ConvertTo-Json -Compress)
