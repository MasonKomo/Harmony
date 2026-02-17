param(
  [Parameter(Mandatory = $true)]
  [string]$Version,

  [Parameter(Mandatory = $true)]
  [string]$ReleaseNotes,

  [Parameter(Mandatory = $true)]
  [string]$KeyPassword,

  [Parameter(Mandatory = $false)]
  [string]$Repo = "MasonKomo/Harmony",

  [Parameter(Mandatory = $false)]
  [string]$KeyPath = "$env:USERPROFILE\.tauri\harmony-updater.key",

  [Parameter(Mandatory = $false)]
  [switch]$SkipNpmCi,

  [Parameter(Mandatory = $false)]
  [switch]$SkipBuild,

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

function Invoke-Step([string]$Label, [scriptblock]$Script) {
  Write-Host ""
  Write-Host "==> $Label"
  & $Script
}

Assert-Command "npm"

$srcTauriDir = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$srcUiDir = (Resolve-Path (Join-Path $srcTauriDir "..\src-ui")).Path
$tauriConfigPath = Join-Path $srcTauriDir "tauri.conf.json"
$publishScriptPath = Join-Path $PSScriptRoot "publish-github-update.ps1"
$releaseTag = if ($Version.StartsWith("v")) { $Version } else { "v$Version" }

if (-not (Test-Path $tauriConfigPath)) {
  throw "Missing tauri config: $tauriConfigPath"
}
if (-not (Test-Path $publishScriptPath)) {
  throw "Missing publish script: $publishScriptPath"
}
if (-not (Test-Path $KeyPath)) {
  throw "Missing key file: $KeyPath"
}

$pubKeyPath = "$KeyPath.pub"
if (-not (Test-Path $pubKeyPath)) {
  throw "Missing public key file: $pubKeyPath"
}

$tauriConfig = Get-Content -Raw -Path $tauriConfigPath | ConvertFrom-Json
$configVersion = [string]$tauriConfig.version
if ($configVersion -ne $Version -and "v$configVersion" -ne $Version) {
  throw "Version mismatch. tauri.conf.json has '$configVersion' but script got '$Version'. Align them first."
}

$configPubKey = [string]$tauriConfig.plugins.updater.pubkey
$filePubKey = (Get-Content -Raw -Path $pubKeyPath).Trim()
if ([string]::IsNullOrWhiteSpace($configPubKey)) {
  throw "plugins.updater.pubkey in tauri.conf.json is empty."
}
if ($configPubKey.Trim() -ne $filePubKey) {
  throw "Configured updater pubkey does not match '$pubKeyPath'. Update tauri.conf.json pubkey or use matching key file."
}

$env:TAURI_SIGNING_PRIVATE_KEY_PATH = $KeyPath
$env:TAURI_SIGNING_PRIVATE_KEY = (Get-Content -Raw -Path $KeyPath).Trim()
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = $KeyPassword

Invoke-Step "Using key file $KeyPath" {
  $chars = $env:TAURI_SIGNING_PRIVATE_KEY.Length
  if ($chars -le 0) {
    throw "Private key appears empty."
  }
  Write-Host "Private key loaded into TAURI_SIGNING_PRIVATE_KEY (chars=$chars)."
}

if (-not $SkipBuild) {
  if (-not $SkipNpmCi) {
    Invoke-Step "Running npm ci in src-ui" {
      npm ci --prefix "$srcUiDir"
    }
  }

  Invoke-Step "Building signed Tauri installer" {
    npm run tauri:build --prefix "$srcUiDir"
  }
}
else {
  Write-Host ""
  Write-Host "==> Skipping build (--SkipBuild)"
}

Invoke-Step "Publishing release artifacts" {
  $params = @{
    ReleaseTag = $releaseTag
    Repo = $Repo
    ReleaseNotes = $ReleaseNotes
  }
  if ($DryRun) {
    $params["DryRun"] = $true
  }
  & "$publishScriptPath" @params
}

Write-Host ""
Write-Host "Release workflow complete."
Write-Host "Tag: $releaseTag"
Write-Host "Repo: $Repo"
Write-Host "Endpoint: https://github.com/$Repo/releases/latest/download/latest.json"
