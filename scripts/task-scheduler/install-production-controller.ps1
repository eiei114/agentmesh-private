# Install a one-shot Production Controller task for local Windows scheduling.
# Does not configure live Multica credentials or run the task immediately.

param(
    [Parameter(Mandatory = $true)]
    [string]$AgentMeshExe,

    [Parameter(Mandatory = $true)]
    [string]$ManifestPath,

    [Parameter(Mandatory = $true)]
    [string]$LedgerManifestPath,

    [Parameter(Mandatory = $true)]
    [string]$ToolchainPin,

    [Parameter(Mandatory = $true)]
    [string]$ToolchainCache,

    [Parameter(Mandatory = $true)]
    [string]$InputJson,

    [Parameter(Mandatory = $true)]
    [string]$SidecarDir,

    [Parameter(Mandatory = $true)]
    [string]$TaskName,

    [string]$Schedule = "PT15M",

    # Stage immutable owner-local task assets without registering a task.
    [switch]$PrepareOnly
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
Test-RequiredFile -Path $LedgerManifestPath
Test-RequiredFile -Path $ToolchainPin
Test-RequiredFile -Path $InputJson
Test-RequiredDirectory -Path $SidecarDir
Test-RequiredDirectory -Path $ToolchainCache
$RunnerScript = Join-Path $PSScriptRoot "run-production-controller.ps1"
$RollbackScript = Join-Path $PSScriptRoot "rollback-production-controller.ps1"
$RollbackParser = Join-Path $PSScriptRoot "rollback-ledger-parse.ps1"
$UninstallScript = Join-Path $PSScriptRoot "uninstall-production-controller.ps1"
Test-RequiredFile -Path $RunnerScript
Test-RequiredFile -Path $RollbackScript
Test-RequiredFile -Path $RollbackParser
Test-RequiredFile -Path $UninstallScript

function Get-Sha256([string]$Path) {
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    $stream = [System.IO.File]::OpenRead((Resolve-Path -LiteralPath $Path).Path)
    try {
        return -join ($sha256.ComputeHash($stream) | ForEach-Object { $_.ToString("x2") })
    } finally {
        $stream.Dispose()
        $sha256.Dispose()
    }
}

function Get-StringSha256([string]$Value) {
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    try {
        return -join ($sha256.ComputeHash(
            [System.Text.Encoding]::UTF8.GetBytes($Value)
        ) | ForEach-Object { $_.ToString("x2") })
    } finally {
        $sha256.Dispose()
    }
}

function Get-TreeDescriptor([string]$Root, [string]$Prefix) {
    $resolvedRoot = (Resolve-Path -LiteralPath $Root).Path.TrimEnd('\', '/')
    return @(Get-ChildItem -LiteralPath $resolvedRoot -File -Recurse | Sort-Object FullName | ForEach-Object {
        $relative = $_.FullName.Substring($resolvedRoot.Length).TrimStart('\', '/').Replace('\', '/')
        "$Prefix/$relative=$(Get-Sha256 -Path $_.FullName)"
    })
}

function Get-TomlString([string]$Text, [string]$Key) {
    $pattern = '(?m)^\s*' + [regex]::Escape($Key) + '\s*=\s*"([^"]+)"\s*$'
    $match = [regex]::Match($Text, $pattern)
    if (-not $match.Success) {
        throw "Toolchain pin missing string field: $Key"
    }
    return $match.Groups[1].Value
}

function Convert-Iso8601DurationToMinutes([string]$Duration) {
    if ($Duration -match '^PT(\d+)M$') {
        $minutes = [int]$Matches[1]
        if ($minutes -ge 1 -and $minutes -le 1440) {
            return $minutes
        }
    }
    throw "Unsupported Schedule format: $Duration (expected PT<n>M, 1-1440 minutes)"
}

$intervalMinutes = Convert-Iso8601DurationToMinutes -Duration $Schedule
$pinText = Get-Content -LiteralPath $ToolchainPin -Raw -Encoding UTF8
$pinTag = Get-TomlString -Text $pinText -Key "tag"
$pinCommit = Get-TomlString -Text $pinText -Key "commit_sha"
$pinTarget = Get-TomlString -Text $pinText -Key "target"
$pinManifestSha = Get-TomlString -Text $pinText -Key "release_manifest_sha256"
if ($pinTag -notmatch '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$') {
    throw "Unsafe toolchain pin tag: $pinTag"
}
if ($pinTarget -notmatch '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$') {
    throw "Unsafe toolchain pin target: $pinTarget"
}
if ($pinCommit -notmatch '^[0-9a-f]{40}$') {
    throw "Invalid toolchain pin commit_sha"
}
if ($pinManifestSha -notmatch '^[0-9a-f]{64}$') {
    throw "Invalid toolchain pin release_manifest_sha256"
}
$resolvedCacheRoot = (Resolve-Path -LiteralPath $ToolchainCache).Path.TrimEnd('\', '/')
$pinnedToolchainDir = Join-Path (Join-Path $resolvedCacheRoot $pinTag) $pinTarget
Test-RequiredDirectory -Path $pinnedToolchainDir
$resolvedPinnedToolchainDir = (Resolve-Path -LiteralPath $pinnedToolchainDir).Path.TrimEnd('\', '/')
$cachePrefix = $resolvedCacheRoot + [System.IO.Path]::DirectorySeparatorChar
if (-not $resolvedPinnedToolchainDir.StartsWith($cachePrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Pinned toolchain directory escapes cache root"
}
$releaseManifestPath = Join-Path $resolvedPinnedToolchainDir "release-manifest.json"
Test-RequiredFile -Path $releaseManifestPath
if ((Get-Sha256 -Path $releaseManifestPath) -ne $pinManifestSha) {
    throw "Pinned release manifest hash does not match toolchain pin"
}
$releaseManifest = Get-Content -LiteralPath $releaseManifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
if (
    $releaseManifest.schema_version -ne "agentmesh-release-manifest.v0" -or
    $releaseManifest.tag -ne $pinTag -or
    $releaseManifest.commit_sha -ne $pinCommit -or
    $releaseManifest.target -ne $pinTarget
) {
    throw "Pinned release manifest identity does not match toolchain pin"
}
if ($releaseManifest.binaries -isnot [System.Management.Automation.PSCustomObject]) {
    throw "Pinned release manifest binaries must be an object"
}
$binaryProperties = @($releaseManifest.binaries.PSObject.Properties)
if ($binaryProperties.Count -eq 0) {
    throw "Pinned release manifest contains no binaries"
}
if (-not ($releaseManifest.binaries.PSObject.Properties.Name -contains "agentmesh-production-controller-observer")) {
    throw "Pinned release manifest lacks production controller observer"
}
if (-not ($releaseManifest.binaries.PSObject.Properties.Name -contains "agentmesh")) {
    throw "Pinned release manifest lacks AgentMesh host"
}
$pinnedPrefix = $resolvedPinnedToolchainDir + [System.IO.Path]::DirectorySeparatorChar
foreach ($property in $binaryProperties) {
    $relativePath = [string]$property.Value.relative_path
    $expectedSha = [string]$property.Value.sha256
    if (
        [string]::IsNullOrWhiteSpace($relativePath) -or
        [System.IO.Path]::IsPathRooted($relativePath) -or
        $relativePath.Contains('\') -or
        @($relativePath.Split('/')) -contains '..' -or
        $expectedSha -notmatch '^[0-9a-f]{64}$'
    ) {
        throw "Unsafe pinned binary entry: $($property.Name)"
    }
    $binaryPath = Join-Path $resolvedPinnedToolchainDir $relativePath.Replace('/', [System.IO.Path]::DirectorySeparatorChar)
    Test-RequiredFile -Path $binaryPath
    $resolvedBinaryPath = (Resolve-Path -LiteralPath $binaryPath).Path
    if (-not $resolvedBinaryPath.StartsWith($pinnedPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Pinned binary escapes toolchain directory: $($property.Name)"
    }
    if ((Get-Sha256 -Path $resolvedBinaryPath) -ne $expectedSha) {
        throw "Pinned binary hash mismatch: $($property.Name)"
    }
}
$expectedHostSha = [string]$releaseManifest.binaries.agentmesh.sha256
if ((Get-Sha256 -Path $AgentMeshExe) -ne $expectedHostSha) {
    throw "AgentMesh host hash does not match pinned release manifest"
}
$manifestRoot = Split-Path -Parent (Resolve-Path -LiteralPath $ManifestPath).Path
$ledgerManifestRoot = Split-Path -Parent (Resolve-Path -LiteralPath $LedgerManifestPath).Path
$agentMeshLeaf = if ((Split-Path -Leaf $AgentMeshExe).EndsWith(".exe", [System.StringComparison]::OrdinalIgnoreCase)) {
    "agentmesh.exe"
} else {
    Split-Path -Leaf $AgentMeshExe
}
$sourceDescriptor = @(
    "agentmesh=$(Get-Sha256 -Path $AgentMeshExe)"
    "runner=$(Get-Sha256 -Path $RunnerScript)"
    "rollback=$(Get-Sha256 -Path $RollbackScript)"
    "rollback-parser=$(Get-Sha256 -Path $RollbackParser)"
    "uninstall=$(Get-Sha256 -Path $UninstallScript)"
    "pin=$(Get-Sha256 -Path $ToolchainPin)"
    "input=$(Get-Sha256 -Path $InputJson)"
) + @(Get-TreeDescriptor -Root $manifestRoot -Prefix "app") + @(
    Get-TreeDescriptor -Root $ledgerManifestRoot -Prefix "ledger-app"
) + @(
    Get-TreeDescriptor -Root $resolvedPinnedToolchainDir -Prefix "toolchain-cache/$pinTag/$pinTarget"
)
$assetHash = Get-StringSha256 -Value ($sourceDescriptor -join "`n")
$assetBase = Join-Path $env:LOCALAPPDATA "AgentMesh\scheduler-assets"
$assetDir = Join-Path $assetBase $assetHash

if (-not (Test-Path -LiteralPath $assetDir -PathType Container)) {
    New-Item -ItemType Directory -Path $assetBase -Force | Out-Null
    $stageDir = Join-Path $assetBase (".stage-$assetHash-$PID")
    Remove-Item -LiteralPath $stageDir -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Path $stageDir -Force | Out-Null
    try {
        Copy-Item -LiteralPath $AgentMeshExe -Destination (Join-Path $stageDir $agentMeshLeaf)
        Copy-Item -LiteralPath $RunnerScript -Destination (Join-Path $stageDir "run-production-controller.ps1")
        Copy-Item -LiteralPath $RollbackScript -Destination (Join-Path $stageDir "rollback-production-controller.ps1")
        Copy-Item -LiteralPath $RollbackParser -Destination (Join-Path $stageDir "rollback-ledger-parse.ps1")
        Copy-Item -LiteralPath $UninstallScript -Destination (Join-Path $stageDir "uninstall-production-controller.ps1")
        Copy-Item -LiteralPath $ToolchainPin -Destination (Join-Path $stageDir "toolchain-pin.toml")
        Copy-Item -LiteralPath $InputJson -Destination (Join-Path $stageDir "input-template.json")
        Copy-Item -LiteralPath $manifestRoot -Destination (Join-Path $stageDir "app") -Recurse
        Copy-Item -LiteralPath $ledgerManifestRoot -Destination (Join-Path $stageDir "ledger-app") -Recurse
        $stageToolchainTag = Join-Path (Join-Path $stageDir "toolchain-cache") $pinTag
        New-Item -ItemType Directory -Path $stageToolchainTag -Force | Out-Null
        Copy-Item -LiteralPath $resolvedPinnedToolchainDir -Destination $stageToolchainTag -Recurse
        try {
            Move-Item -LiteralPath $stageDir -Destination $assetDir -ErrorAction Stop
        } catch {
            if (-not (Test-Path -LiteralPath $assetDir -PathType Container)) { throw }
            Remove-Item -LiteralPath $stageDir -Recurse -Force -ErrorAction SilentlyContinue
        }
    } finally {
        Remove-Item -LiteralPath $stageDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

$durableAgentMesh = Join-Path $assetDir $agentMeshLeaf
$durableRunner = Join-Path $assetDir "run-production-controller.ps1"
$durableRollback = Join-Path $assetDir "rollback-production-controller.ps1"
$durableRollbackParser = Join-Path $assetDir "rollback-ledger-parse.ps1"
$durableUninstall = Join-Path $assetDir "uninstall-production-controller.ps1"
$durablePin = Join-Path $assetDir "toolchain-pin.toml"
$durableInput = Join-Path $assetDir "input-template.json"
$durableManifestRoot = Join-Path $assetDir "app"
$durableManifest = Join-Path $durableManifestRoot (Split-Path -Leaf $ManifestPath)
$durableLedgerManifestRoot = Join-Path $assetDir "ledger-app"
$durableLedgerManifest = Join-Path $durableLedgerManifestRoot (Split-Path -Leaf $LedgerManifestPath)
$durableToolchainCache = Join-Path $assetDir "toolchain-cache"
$durablePinnedToolchainDir = Join-Path (Join-Path $durableToolchainCache $pinTag) $pinTarget
$durableDescriptor = @(
    "agentmesh=$(Get-Sha256 -Path $durableAgentMesh)"
    "runner=$(Get-Sha256 -Path $durableRunner)"
    "rollback=$(Get-Sha256 -Path $durableRollback)"
    "rollback-parser=$(Get-Sha256 -Path $durableRollbackParser)"
    "uninstall=$(Get-Sha256 -Path $durableUninstall)"
    "pin=$(Get-Sha256 -Path $durablePin)"
    "input=$(Get-Sha256 -Path $durableInput)"
) + @(Get-TreeDescriptor -Root $durableManifestRoot -Prefix "app") + @(
    Get-TreeDescriptor -Root $durableLedgerManifestRoot -Prefix "ledger-app"
) + @(
    Get-TreeDescriptor -Root $durablePinnedToolchainDir -Prefix "toolchain-cache/$pinTag/$pinTarget"
)
if (($durableDescriptor -join "`n") -ne ($sourceDescriptor -join "`n")) {
    throw "Durable scheduler assets failed hash verification: $assetDir"
}

$AgentMeshExe = $durableAgentMesh
$RunnerScript = $durableRunner
$ManifestPath = $durableManifest
$LedgerManifestPath = $durableLedgerManifest
$ToolchainPin = $durablePin
$ToolchainCache = $durableToolchainCache
$InputJson = $durableInput

if ($PrepareOnly) {
    Write-Output (@{
        task = $TaskName
        status = "prepared"
        asset_hash = $assetHash
        asset_dir = $assetDir
        agentmesh_exe = $AgentMeshExe
        runner = $RunnerScript
        rollback_script = $durableRollback
        rollback_parser = $durableRollbackParser
        uninstall_script = $durableUninstall
        manifest = $ManifestPath
        ledger_manifest = $LedgerManifestPath
        toolchain_pin = $ToolchainPin
        toolchain_cache = $ToolchainCache
        input_template = $InputJson
    } | ConvertTo-Json -Compress)
    return
}

$startAt = (Get-Date).AddMinutes(1)
$anchorUtc = $startAt.ToUniversalTime().ToString("o")

$argumentParts = @(
    '-NoProfile',
    '-NonInteractive',
    '-ExecutionPolicy', 'Bypass',
    '-File', $RunnerScript,
    '-AgentMeshExe', $AgentMeshExe,
    '-ManifestPath', $ManifestPath,
    '-ToolchainPin', $ToolchainPin,
    '-ToolchainCache', $ToolchainCache,
    '-InputJson', $InputJson,
    '-SidecarDir', $SidecarDir,
    '-ScheduleAnchorUtc', $anchorUtc,
    '-IntervalMinutes', $intervalMinutes
) | ForEach-Object { Format-ScheduledTaskArgument $_ }

$PowerShellExe = (Get-Process -Id $PID).Path
$action = New-ScheduledTaskAction -Execute $PowerShellExe -Argument ($argumentParts -join ' ')
$trigger = New-ScheduledTaskTrigger -Once -At $startAt `
    -RepetitionInterval (New-TimeSpan -Minutes $intervalMinutes) `
    -RepetitionDuration ([TimeSpan]::MaxValue)
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -MultipleInstances IgnoreNew

Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Settings $settings -Force | Out-Null
Write-Output (@{ task = $TaskName; status = "installed"; schedule = $Schedule; interval_minutes = $intervalMinutes; schedule_anchor_utc = $anchorUtc; asset_hash = $assetHash; asset_dir = $assetDir; agentmesh_exe = $AgentMeshExe; runner = $RunnerScript; rollback_script = $durableRollback; rollback_parser = $durableRollbackParser; uninstall_script = $durableUninstall; manifest = $ManifestPath; ledger_manifest = $LedgerManifestPath; toolchain_pin = $ToolchainPin; toolchain_cache = $ToolchainCache; input_template = $InputJson } | ConvertTo-Json -Compress)
