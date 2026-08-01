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
$bundleVersion = $version -replace "-.*$", ""
$baseUrl = "https://github.com/$Repository/releases/download/$TagName"

Remove-Item -Recurse -Force $OutputDir -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

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

  $source = Get-ChildItem -LiteralPath $folder -Recurse -File |
    Where-Object { $_.Name -eq $SourceName } |
    Select-Object -First 1

  if (!$source) {
    throw "Missing artifact file: $ArtifactFolder/$SourceName"
  }

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
  $platforms[$Key] = [ordered]@{
    signature = $signature
    url = "$baseUrl/$DestinationName"
  }
}

Add-Platform "darwin-aarch64" "zenith-relay-macos-arm64" "Zenith Relay.app.tar.gz" "Zenith.Relay_aarch64.app.tar.gz"
$platforms["darwin-aarch64-app"] = $platforms["darwin-aarch64"]

Add-Platform "darwin-x86_64" "zenith-relay-macos-intel" "Zenith Relay.app.tar.gz" "Zenith.Relay_x64.app.tar.gz"
$platforms["darwin-x86_64-app"] = $platforms["darwin-x86_64"]

Add-Platform "linux-aarch64" "zenith-relay-linux-arm64" "Zenith Relay_${bundleVersion}_aarch64.AppImage" "Zenith.Relay_${version}_aarch64.AppImage"
$platforms["linux-aarch64-appimage"] = $platforms["linux-aarch64"]
Add-Platform "linux-aarch64-deb" "zenith-relay-linux-arm64" "Zenith Relay_${bundleVersion}_arm64.deb" "Zenith.Relay_${version}_arm64.deb"
Add-Platform "linux-aarch64-rpm" "zenith-relay-linux-arm64" "Zenith Relay-${bundleVersion}-1.aarch64.rpm" "Zenith.Relay-${version}-1.aarch64.rpm"

Add-Platform "linux-x86_64" "zenith-relay-linux-x64" "Zenith Relay_${bundleVersion}_amd64.AppImage" "Zenith.Relay_${version}_amd64.AppImage"
$platforms["linux-x86_64-appimage"] = $platforms["linux-x86_64"]
Add-Platform "linux-x86_64-deb" "zenith-relay-linux-x64" "Zenith Relay_${bundleVersion}_amd64.deb" "Zenith.Relay_${version}_amd64.deb"
Add-Platform "linux-x86_64-rpm" "zenith-relay-linux-x64" "Zenith Relay-${bundleVersion}-1.x86_64.rpm" "Zenith.Relay-${version}-1.x86_64.rpm"

Add-Platform "windows-aarch64" "zenith-relay-windows-arm64" "Zenith Relay_${bundleVersion}_arm64_en-US.msi" "Zenith.Relay_${version}_arm64_en-US.msi"
$platforms["windows-aarch64-msi"] = $platforms["windows-aarch64"]
Add-Platform "windows-aarch64-nsis" "zenith-relay-windows-arm64" "Zenith Relay_${bundleVersion}_arm64-setup.exe" "Zenith.Relay_${version}_arm64-setup.exe"
Add-Platform "windows-aarch64-portable" "zenith-relay-windows-arm64" "zenith-relay-windows-arm64.exe" "Zenith.Relay_${version}_arm64.exe"

Add-Platform "windows-x86_64" "zenith-relay-windows-x64" "Zenith Relay_${bundleVersion}_x64_en-US.msi" "Zenith.Relay_${version}_x64_en-US.msi"
$platforms["windows-x86_64-msi"] = $platforms["windows-x86_64"]
Add-Platform "windows-x86_64-nsis" "zenith-relay-windows-x64" "Zenith Relay_${bundleVersion}_x64-setup.exe" "Zenith.Relay_${version}_x64-setup.exe"
Add-Platform "windows-x86_64-portable" "zenith-relay-windows-x64" "zenith-relay-windows-x64.exe" "Zenith.Relay_${version}_x64.exe"

$latest = [ordered]@{
  version = $version
  notes = $(if ($Notes.Trim()) { $Notes.Trim() } else { "Zenith Relay $version" })
  pub_date = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ss.fffZ")
  platforms = $platforms
}

$latest | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath (Join-Path $OutputDir "latest.json") -Encoding UTF8
