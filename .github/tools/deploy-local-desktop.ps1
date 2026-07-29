param(
    [string]$Source = (Join-Path $PSScriptRoot "..\..\src-tauri\target\release\zenith-relay.exe"),
    [string]$Destination = (Join-Path ([Environment]::GetFolderPath("Desktop")) "Zenith Relay.exe"),
    [ValidateRange(0, 60)]
    [int]$DelaySeconds = 0
)

$ErrorActionPreference = "Stop"
if ($DelaySeconds -gt 0) { Start-Sleep -Seconds $DelaySeconds }
$sourcePath = (Resolve-Path -LiteralPath $Source).Path
$expectedSource = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\..\src-tauri\target\release\zenith-relay.exe"))
$destinationPath = [System.IO.Path]::GetFullPath($Destination)
$expectedDestination = [System.IO.Path]::GetFullPath((Join-Path ([Environment]::GetFolderPath("Desktop")) "Zenith Relay.exe"))

if (-not [StringComparer]::OrdinalIgnoreCase.Equals($sourcePath, $expectedSource) -or
    -not [StringComparer]::OrdinalIgnoreCase.Equals($destinationPath, $expectedDestination)) {
    throw "Unexpected deployment path: $sourcePath -> $destinationPath"
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

Get-Process -Name "Zenith Relay" -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -eq $destinationPath } |
    Stop-Process -Force

for ($attempt = 0; $attempt -lt 50; $attempt++) {
    $running = Get-Process -Name "Zenith Relay" -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -eq $destinationPath }
    if (-not $running) { break }
    Start-Sleep -Milliseconds 100
}
if ($running) { throw "The previous Zenith Relay process did not stop" }

if (Test-Path -LiteralPath $destinationPath) {
    Remove-Item -LiteralPath $destinationPath -Force
}
Copy-Item -LiteralPath $sourcePath -Destination $destinationPath

$destinationHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $destinationPath).Hash
if ($destinationHash -ne $sourceHash) {
    throw "Deployed executable hash does not match the release build"
}

$process = Start-Process -FilePath $destinationPath -PassThru
Start-Sleep -Seconds 3
[pscustomobject]@{
    Path = $destinationPath
    Sha256 = $destinationHash
    Pid = $process.Id
    Running = [bool](Get-Process -Id $process.Id -ErrorAction SilentlyContinue)
    Localhost14998 = [bool](Get-NetTCPConnection -State Listen -LocalPort 14998 -ErrorAction SilentlyContinue)
}
