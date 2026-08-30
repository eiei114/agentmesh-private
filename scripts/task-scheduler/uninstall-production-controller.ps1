# Uninstall a Production Controller Observer scheduled task.

param(
    [Parameter(Mandatory = $true)]
    [string]$TaskName
)

$ErrorActionPreference = "Stop"

if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
    Write-Output (@{ task = $TaskName; status = "uninstalled" } | ConvertTo-Json -Compress)
} else {
    Write-Output (@{ task = $TaskName; status = "not_found" } | ConvertTo-Json -Compress)
}
