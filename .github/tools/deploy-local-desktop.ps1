param(
    [string]$Source = (Join-Path $PSScriptRoot "..\..\src-tauri\target\release\zenith-relay.exe"),
    [string]$Destination = (Join-Path ([Environment]::GetFolderPath("Desktop")) "zenith-relay.exe"),
    [ValidateRange(0, 60)]
    [int]$DelaySeconds = 0,
    [ValidateRange(1, 60)]
    [int]$StartupTimeoutSeconds = 15,
    [switch]$RequireGateway
)

$ErrorActionPreference = "Stop"
if ($DelaySeconds -gt 0) { Start-Sleep -Seconds $DelaySeconds }
$sourcePath = (Resolve-Path -LiteralPath $Source).Path
$desktopPath = [System.IO.Path]::GetFullPath([Environment]::GetFolderPath("Desktop"))
$expectedSource = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\..\src-tauri\target\release\zenith-relay.exe"))
$destinationPath = [System.IO.Path]::GetFullPath($Destination)
$expectedDestination = [System.IO.Path]::GetFullPath((Join-Path $desktopPath "zenith-relay.exe"))
$legacyPaths = @(
    [System.IO.Path]::GetFullPath((Join-Path $desktopPath "Zenith Relay.exe")),
    [System.IO.Path]::GetFullPath((Join-Path $desktopPath "Zenith Codex.exe")),
    [System.IO.Path]::GetFullPath((Join-Path $desktopPath "Zenith.codex.exe")),
    [System.IO.Path]::GetFullPath((Join-Path $desktopPath "zenith-codex.exe"))
)
$managedPaths = @($destinationPath) + $legacyPaths

if (-not [StringComparer]::OrdinalIgnoreCase.Equals($sourcePath, $expectedSource) -or
    -not [StringComparer]::OrdinalIgnoreCase.Equals($destinationPath, $expectedDestination)) {
    throw "Unexpected deployment path: $sourcePath -> $destinationPath"
}

function Get-ManagedRelayProcess {
    $processes = Get-CimInstance Win32_Process -ErrorAction SilentlyContinue
    foreach ($process in $processes) {
        if ([string]::IsNullOrWhiteSpace($process.ExecutablePath)) { continue }
        try {
            $processPath = [System.IO.Path]::GetFullPath($process.ExecutablePath)
        } catch {
            continue
        }
        if ($managedPaths -contains $processPath) {
            $process
        }
    }
}

function Wait-ForManagedRelayToStop {
    for ($attempt = 0; $attempt -lt 100; $attempt++) {
        $remaining = @(Get-ManagedRelayProcess)
        if ($remaining.Count -eq 0) { return }
        Start-Sleep -Milliseconds 100
    }
    $ids = @(Get-ManagedRelayProcess | Select-Object -ExpandProperty ProcessId)
    throw "A previous Zenith Relay process did not stop: $($ids -join ', ')"
}

function Get-ManagedGatewayListener {
    $processIds = @(Get-ManagedRelayProcess | Select-Object -ExpandProperty ProcessId)
    if ($processIds.Count -eq 0) { return @() }
    @(Get-NetTCPConnection -State Listen -LocalAddress 127.0.0.1 -LocalPort 14998 -ErrorAction SilentlyContinue |
        Where-Object { $processIds -contains $_.OwningProcess })
}

$sourceHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $sourcePath).Hash
$productionHashPath = "$sourcePath.production.sha256"
if (-not (Test-Path -LiteralPath $productionHashPath)) {
    throw "Release build marker is missing: $productionHashPath"
}
$productionHash = (Get-Content -Raw -LiteralPath $productionHashPath).Trim()
if (-not [StringComparer]::OrdinalIgnoreCase.Equals($sourceHash, $productionHash)) {
    throw "Release executable does not match its production build marker"
}

@(Get-ManagedRelayProcess) | ForEach-Object {
    Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue
}
Wait-ForManagedRelayToStop

# Remove old desktop filenames only after their processes have exited. Otherwise
# an old single-instance process can keep the new launch from starting and
# clients briefly see connection refused on the local gateway port.
foreach ($legacyPath in $legacyPaths) {
    if (Test-Path -LiteralPath $legacyPath) {
        Remove-Item -LiteralPath $legacyPath -Force
    }
}

if (Test-Path -LiteralPath $destinationPath) {
    Remove-Item -LiteralPath $destinationPath -Force
}
Copy-Item -LiteralPath $sourcePath -Destination $destinationPath

$destinationHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $destinationPath).Hash
if ($destinationHash -ne $sourceHash) {
    throw "Deployed executable hash does not match the release build"
}

$process = Start-Process -FilePath $destinationPath -WorkingDirectory $desktopPath -PassThru
$deadline = [DateTime]::UtcNow.AddSeconds($StartupTimeoutSeconds)
$running = @()
$listener = @()
while ([DateTime]::UtcNow -lt $deadline) {
    $running = @(Get-ManagedRelayProcess)
    if ($running.Count -eq 0) {
        if ($process.HasExited) { break }
    } else {
        $listener = @(Get-ManagedGatewayListener)
        if ($listener.Count -gt 0) { break }
    }
    Start-Sleep -Milliseconds 200
}

if ($running.Count -eq 0) {
    $exitCode = if ($process.HasExited) { $process.ExitCode } else { "unknown" }
    throw "Zenith Relay exited before startup completed (exit code: $exitCode)"
}

$gatewayReady = @(Get-ManagedGatewayListener).Count -gt 0
if ($RequireGateway -and -not $gatewayReady) {
    throw "Zenith Relay is running, but the local gateway did not start on 127.0.0.1:14998"
}

[pscustomobject]@{
    Path = $destinationPath
    Sha256 = $destinationHash
    Pid = @($running | Select-Object -ExpandProperty ProcessId)
    Running = $true
    GatewayReady = $gatewayReady
    Localhost14998 = $gatewayReady
}
