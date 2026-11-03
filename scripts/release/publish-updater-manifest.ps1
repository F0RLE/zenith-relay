param(
  [Parameter(Mandatory = $true)]
  [string]$TagName,

  [Parameter(Mandatory = $true)]
  [string]$Repository,

  [Parameter(Mandatory = $true)]
  [string]$ArtifactsDir,

  [Parameter(Mandatory = $true)]
  [string]$OutputDir,

  [string]$Notes = ""
)

$ErrorActionPreference = "Stop"

$version = $TagName -replace "^v", ""
if ($version -notmatch "^\d+\.\d+\.\d+$") {
  throw "The stable updater manifest accepts only stable SemVer tags (for example v1.1.0); prereleases require a separate update channel."
}
$baseUrl = "https://github.com/$Repository/releases/download/$TagName"

if (Test-Path -LiteralPath $OutputDir) {
  $output = Get-Item -LiteralPath $OutputDir -Force
  if (!$output.PSIsContainer -or ($output.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
    throw "Updater output must be a regular directory: $OutputDir"
  }
  if (Get-ChildItem -LiteralPath $OutputDir -Force | Select-Object -First 1) {
    throw "Updater output must be empty; choose a new output directory: $OutputDir"
  }
} else {
  New-Item -ItemType Directory -Path $OutputDir | Out-Null
}

function Copy-Asset {
  param(
    [Parameter(Mandatory = $true)]
    [string]$ArtifactFolder,

    [Parameter(Mandatory = $true)]
    [string]$SourceName,

    [Parameter(Mandatory = $true)]
    [string]$DestinationName
  )

  $folder = Join-Path $ArtifactsDir $ArtifactFolder
  if (!(Test-Path -LiteralPath $folder)) {
    throw "Missing artifact folder: $folder"
  }

  $sources = @(Get-ChildItem -LiteralPath $folder -Recurse -File |
    Where-Object { $_.Name -eq $SourceName })

  if ($sources.Count -ne 1) {
    throw "Expected exactly one artifact file: $ArtifactFolder/$SourceName; found $($sources.Count)"
  }
  $source = $sources[0]
  if ($source.Length -eq 0) { throw "Empty artifact file: $ArtifactFolder/$SourceName" }

  $destination = Join-Path $OutputDir $DestinationName
  Copy-Item -LiteralPath $source.FullName -Destination $destination -Force
  return $destination
}

$platforms = [ordered]@{}

function Add-Platform {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Key,

    [Parameter(Mandatory = $true)]
    [string]$ArtifactFolder,

    [Parameter(Mandatory = $true)]
    [string]$SourceName,

    [Parameter(Mandatory = $true)]
    [string]$DestinationName
  )

  Copy-Asset -ArtifactFolder $ArtifactFolder -SourceName $SourceName -DestinationName $DestinationName | Out-Null
  Copy-Asset -ArtifactFolder $ArtifactFolder -SourceName "$SourceName.sig" -DestinationName "$DestinationName.sig" | Out-Null

  $signature = (Get-Content -Raw -LiteralPath (Join-Path $OutputDir "$DestinationName.sig")).Trim()
  if (!$signature) { throw "Empty updater signature: $DestinationName.sig" }
  $platforms[$Key] = [ordered]@{
    signature = $signature
    url = "$baseUrl/$DestinationName"
  }
}

Add-Platform "darwin-aarch64" "zenith-relay-macos-arm64" "zenith-relay-macos-arm64.app.tar.gz" "zenith-relay-macos-arm64.app.tar.gz"
$platforms["darwin-aarch64-app"] = $platforms["darwin-aarch64"]

Add-Platform "darwin-x86_64" "zenith-relay-macos-intel" "zenith-relay-macos-intel.app.tar.gz" "zenith-relay-macos-intel.app.tar.gz"
$platforms["darwin-x86_64-app"] = $platforms["darwin-x86_64"]

Add-Platform "linux-aarch64" "zenith-relay-linux-arm64" "zenith-relay-linux-arm64.AppImage" "zenith-relay-linux-arm64.AppImage"
$platforms["linux-aarch64-appimage"] = $platforms["linux-aarch64"]
Add-Platform "linux-aarch64-deb" "zenith-relay-linux-arm64" "zenith-relay-linux-arm64.deb" "zenith-relay-linux-arm64.deb"
Add-Platform "linux-aarch64-rpm" "zenith-relay-linux-arm64" "zenith-relay-linux-arm64.rpm" "zenith-relay-linux-arm64.rpm"

Add-Platform "linux-x86_64" "zenith-relay-linux-x64" "zenith-relay-linux-x64.AppImage" "zenith-relay-linux-x64.AppImage"
$platforms["linux-x86_64-appimage"] = $platforms["linux-x86_64"]
Add-Platform "linux-x86_64-deb" "zenith-relay-linux-x64" "zenith-relay-linux-x64.deb" "zenith-relay-linux-x64.deb"
Add-Platform "linux-x86_64-rpm" "zenith-relay-linux-x64" "zenith-relay-linux-x64.rpm" "zenith-relay-linux-x64.rpm"

Add-Platform "windows-aarch64" "zenith-relay-windows-arm64" "zenith-relay-windows-arm64-setup.exe" "zenith-relay-windows-arm64-setup.exe"
$platforms["windows-aarch64-nsis"] = $platforms["windows-aarch64"]
Add-Platform "windows-aarch64-portable" "zenith-relay-windows-arm64" "zenith-relay-windows-arm64.exe" "zenith-relay-windows-arm64.exe"

Add-Platform "windows-x86_64" "zenith-relay-windows-x64" "zenith-relay-windows-x64-setup.exe" "zenith-relay-windows-x64-setup.exe"
$platforms["windows-x86_64-nsis"] = $platforms["windows-x86_64"]
Add-Platform "windows-x86_64-portable" "zenith-relay-windows-x64" "zenith-relay-windows-x64.exe" "zenith-relay-windows-x64.exe"

$latest = [ordered]@{
  version = $version
  notes = $(if ($Notes.Trim()) { $Notes.Trim() } else { "Zenith Relay $version" })
  pub_date = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ss.fffZ")
  platforms = $platforms
}

$latest | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath (Join-Path $OutputDir "latest.json") -Encoding UTF8
