param(
    [string]$Source = (Join-Path $PSScriptRoot "..\..\src-tauri\target\release\zenith-relay.exe"),
    [string]$Destination = (Join-Path ([Environment]::GetFolderPath("Desktop")) "Zenith Relay.exe")
)

$ErrorActionPreference = "Stop"
$sourcePath = (Resolve-Path -LiteralPath $Source).Path
$expectedSource = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\..\src-tauri\target\release\zenith-relay.exe"))
$destinationPath = [System.IO.Path]::GetFullPath($Destination)
$expectedDestination = [System.IO.Path]::GetFullPath((Join-Path ([Environment]::GetFolderPath("Desktop")) "Zenith Relay.exe"))

if (-not [StringComparer]::OrdinalIgnoreCase.Equals($sourcePath, $expectedSource) -or
    -not [StringComparer]::OrdinalIgnoreCase.Equals($destinationPath, $expectedDestination)) {
    throw "Unexpected deployment path: $sourcePath -> $destinationPath"
}

$sourceHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $sourcePath).Hash
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
Move-Item -LiteralPath $sourcePath -Destination $destinationPath

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
