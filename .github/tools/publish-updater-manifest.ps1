param(
  [Parameter(Mandatory = $true)]
  [string]$TagName,

  [Parameter(Mandatory = $true)]
  [string]$Repository,

  [Parameter(Mandatory = $true)]
  [string]$ArtifactsDir,

  [Parameter(Mandatory = $true)]
  [string]$OutputDir
)

$ErrorActionPreference = "Stop"

$version = $TagName -replace "^v", ""
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

Add-Platform "darwin-aarch64" "zenith-codex-macos-arm64" "Zenith Codex.app.tar.gz" "Zenith.Codex_aarch64.app.tar.gz"
$platforms["darwin-aarch64-app"] = $platforms["darwin-aarch64"]

Add-Platform "darwin-x86_64" "zenith-codex-macos-intel" "Zenith Codex.app.tar.gz" "Zenith.Codex_x64.app.tar.gz"
$platforms["darwin-x86_64-app"] = $platforms["darwin-x86_64"]

Add-Platform "linux-aarch64" "zenith-codex-linux-arm64" "Zenith Codex_${version}_aarch64.AppImage" "Zenith.Codex_${version}_aarch64.AppImage"
$platforms["linux-aarch64-appimage"] = $platforms["linux-aarch64"]
Add-Platform "linux-aarch64-deb" "zenith-codex-linux-arm64" "Zenith Codex_${version}_arm64.deb" "Zenith.Codex_${version}_arm64.deb"
Add-Platform "linux-aarch64-rpm" "zenith-codex-linux-arm64" "Zenith Codex-${version}-1.aarch64.rpm" "Zenith.Codex-${version}-1.aarch64.rpm"

Add-Platform "linux-x86_64" "zenith-codex-linux-x64" "Zenith Codex_${version}_amd64.AppImage" "Zenith.Codex_${version}_amd64.AppImage"
$platforms["linux-x86_64-appimage"] = $platforms["linux-x86_64"]
Add-Platform "linux-x86_64-deb" "zenith-codex-linux-x64" "Zenith Codex_${version}_amd64.deb" "Zenith.Codex_${version}_amd64.deb"
Add-Platform "linux-x86_64-rpm" "zenith-codex-linux-x64" "Zenith Codex-${version}-1.x86_64.rpm" "Zenith.Codex-${version}-1.x86_64.rpm"

Add-Platform "windows-aarch64" "zenith-codex-windows-arm64" "Zenith Codex_${version}_arm64_en-US.msi" "Zenith.Codex_${version}_arm64_en-US.msi"
$platforms["windows-aarch64-msi"] = $platforms["windows-aarch64"]
Add-Platform "windows-aarch64-nsis" "zenith-codex-windows-arm64" "Zenith Codex_${version}_arm64-setup.exe" "Zenith.Codex_${version}_arm64-setup.exe"

Add-Platform "windows-x86_64" "zenith-codex-windows-x64" "Zenith Codex_${version}_x64_en-US.msi" "Zenith.Codex_${version}_x64_en-US.msi"
$platforms["windows-x86_64-msi"] = $platforms["windows-x86_64"]
Add-Platform "windows-x86_64-nsis" "zenith-codex-windows-x64" "Zenith Codex_${version}_x64-setup.exe" "Zenith.Codex_${version}_x64-setup.exe"

$latest = [ordered]@{
  version = $version
  notes = "Zenith Codex $version"
  pub_date = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ss.fffZ")
  platforms = $platforms
}

$latest | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath (Join-Path $OutputDir "latest.json") -Encoding UTF8
