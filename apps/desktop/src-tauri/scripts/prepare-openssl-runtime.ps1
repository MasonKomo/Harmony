$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$dllNames = @(
  "libssl-3-x64.dll",
  "libcrypto-3-x64.dll"
)

$outputDir = Join-Path $PSScriptRoot "..\resources\openssl-windows"
New-Item -ItemType Directory -Path $outputDir -Force | Out-Null

foreach ($dllName in $dllNames) {
  $resolvedPath = $null

  $whereResult = & where.exe $dllName 2>$null
  if ($LASTEXITCODE -eq 0 -and $whereResult) {
    $resolvedPath = ($whereResult | Select-Object -First 1).Trim()
  }

  if (-not $resolvedPath) {
    $systemPath = Join-Path $env:WINDIR "System32\$dllName"
    if (Test-Path $systemPath) {
      $resolvedPath = $systemPath
    }
  }

  if (-not $resolvedPath) {
    throw "Could not locate $dllName. Make sure OpenSSL runtime DLLs are installed on the build machine."
  }

  Copy-Item -Path $resolvedPath -Destination (Join-Path $outputDir $dllName) -Force
}

Write-Host "Prepared OpenSSL runtime DLLs in $outputDir"
