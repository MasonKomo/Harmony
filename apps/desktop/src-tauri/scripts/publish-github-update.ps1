param(
  [Parameter(Mandatory = $true)]
  [string]$ReleaseTag,

  [Parameter(Mandatory = $false)]
  [string]$Repo = "MasonKomo/Harmony",

  [Parameter(Mandatory = $false)]
  [string]$ReleaseNotes = "Harmony desktop release",

  [Parameter(Mandatory = $false)]
  [switch]$SkipReleaseCreate,

  [Parameter(Mandatory = $false)]
  [switch]$DryRun
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Assert-Command([string]$Name) {
  if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
    throw "Missing required command: $Name"
  }
}

function Resolve-GhCommand {
  $gh = Get-Command "gh" -ErrorAction SilentlyContinue
  if ($gh) {
    return "gh"
  }

  $fallback = "C:\Program Files\GitHub CLI\gh.exe"
  if (Test-Path $fallback) {
    return $fallback
  }

  throw "Missing required command: gh (GitHub CLI). Install it or add it to PATH."
}

function Assert-LatestManifest([string]$Path) {
  if (-not (Test-Path $Path)) {
    throw "latest.json was not created: $Path"
  }

  $bytes = [System.IO.File]::ReadAllBytes($Path)
  $hasUtf8Bom = $bytes.Length -ge 3 -and $bytes[0] -eq 239 -and $bytes[1] -eq 187 -and $bytes[2] -eq 191
  if ($hasUtf8Bom) {
    throw "latest.json contains a UTF-8 BOM. Tauri updater expects UTF-8 JSON without BOM."
  }

  $jsonText = [System.Text.Encoding]::UTF8.GetString($bytes)
  try {
    $manifest = $jsonText | ConvertFrom-Json
  }
  catch {
    throw "latest.json is not valid JSON: $($_.Exception.Message)"
  }

  if ([string]::IsNullOrWhiteSpace([string]$manifest.version)) {
    throw "latest.json is missing required field: version"
  }
  if (-not $manifest.platforms) {
    throw "latest.json is missing required field: platforms"
  }

  $windowsPlatform = $manifest.platforms.PSObject.Properties["windows-x86_64"]
  if (-not $windowsPlatform) {
    throw "latest.json is missing required field: platforms.windows-x86_64"
  }

  $windowsManifest = $windowsPlatform.Value
  if ([string]::IsNullOrWhiteSpace([string]$windowsManifest.signature)) {
    throw "latest.json is missing required field: platforms.windows-x86_64.signature"
  }
  if ([string]::IsNullOrWhiteSpace([string]$windowsManifest.url)) {
    throw "latest.json is missing required field: platforms.windows-x86_64.url"
  }
}

$ghCommand = $null
if (-not $DryRun) {
  $ghCommand = Resolve-GhCommand
}

$tauriConfigPath = Join-Path $PSScriptRoot "..\tauri.conf.json"
$tauriConfig = Get-Content -Raw -Path $tauriConfigPath | ConvertFrom-Json

$appVersion = [string]$tauriConfig.version
$bundleDir = Join-Path $PSScriptRoot "..\target\release\bundle\nsis"

if (-not (Test-Path $bundleDir)) {
  throw "NSIS bundle directory not found: $bundleDir"
}

$setupExe = Get-ChildItem -Path $bundleDir -File -Filter "*-setup.exe" |
  Sort-Object LastWriteTime -Descending |
  Select-Object -First 1

if (-not $setupExe) {
  throw "No NSIS setup executable found in $bundleDir"
}

$sigPath = "$($setupExe.FullName).sig"
if (-not (Test-Path $sigPath)) {
  throw "Missing signature file: $sigPath"
}

$signature = (Get-Content -Raw -Path $sigPath).Trim()
if ([string]::IsNullOrWhiteSpace($signature)) {
  throw "Signature file is empty: $sigPath"
}

$assetUrl = "https://github.com/$Repo/releases/download/$ReleaseTag/$($setupExe.Name)"
$latestJsonPath = Join-Path $bundleDir "latest.json"
$latest = @{
  version = $appVersion
  notes = $ReleaseNotes
  pub_date = (Get-Date).ToUniversalTime().ToString("o")
  platforms = @{
    "windows-x86_64" = @{
      signature = $signature
      url = $assetUrl
    }
  }
}

$latestJson = $latest | ConvertTo-Json -Depth 10
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText($latestJsonPath, $latestJson, $utf8NoBom)

Assert-LatestManifest -Path $latestJsonPath

if ($DryRun) {
  Write-Host "Dry run complete. Manifest generated at:"
  Write-Host "  $latestJsonPath"
  Write-Host "Manifest validated (UTF-8 without BOM + required fields)."
  Write-Host "No GitHub release actions were performed."
  exit 0
}

if (-not $SkipReleaseCreate) {
  $releaseExists = $false
  try {
    & $ghCommand release view $ReleaseTag --repo $Repo --json tagName *> $null
    $releaseExists = $LASTEXITCODE -eq 0
  }
  catch {
    $releaseExists = $false
  }

  if (-not $releaseExists) {
    & $ghCommand release create $ReleaseTag --repo $Repo --title "Harmony $appVersion" --notes $ReleaseNotes
  }
}

& $ghCommand release upload $ReleaseTag `
  "$($setupExe.FullName)" `
  "$sigPath" `
  "$latestJsonPath" `
  --repo $Repo `
  --clobber

Write-Host ""
Write-Host "Published update assets for ${ReleaseTag}:"
Write-Host "  - $($setupExe.Name)"
Write-Host "  - $([System.IO.Path]::GetFileName($sigPath))"
Write-Host "  - latest.json"
Write-Host ""
Write-Host "Updater endpoint:"
Write-Host "  https://github.com/$Repo/releases/latest/download/latest.json"
