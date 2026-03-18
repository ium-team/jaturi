$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$sourceExe = Join-Path $scriptDir "jaturi.exe"

if (-not (Test-Path $sourceExe)) {
  throw "jaturi.exe not found next to install.ps1"
}

$targetDir = Join-Path $env:LOCALAPPDATA "Programs\jaturi\bin"
$targetExe = Join-Path $targetDir "jaturi.exe"

New-Item -ItemType Directory -Force -Path $targetDir | Out-Null
Copy-Item $sourceExe $targetExe -Force

$currentUserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($null -eq $currentUserPath) {
  $currentUserPath = ""
}

$pathEntries = $currentUserPath -split ";" | Where-Object { $_ -and $_.Trim() -ne "" }
$normalizedTarget = $targetDir.TrimEnd('\\')
$alreadyExists = $false

foreach ($entry in $pathEntries) {
  if ($entry.TrimEnd('\\') -ieq $normalizedTarget) {
    $alreadyExists = $true
    break
  }
}

if (-not $alreadyExists) {
  $newUserPath = if ([string]::IsNullOrWhiteSpace($currentUserPath)) {
    $targetDir
  } else {
    "$currentUserPath;$targetDir"
  }
  [Environment]::SetEnvironmentVariable("Path", $newUserPath, "User")
}

$sessionPathEntries = $env:Path -split ";" | Where-Object { $_ -and $_.Trim() -ne "" }
$sessionHasTarget = $false
foreach ($entry in $sessionPathEntries) {
  if ($entry.TrimEnd('\\') -ieq $normalizedTarget) {
    $sessionHasTarget = $true
    break
  }
}

if (-not $sessionHasTarget) {
  $env:Path = "$env:Path;$targetDir"
}

Write-Host "Installed to $targetExe"
if (-not $alreadyExists) {
  Write-Host "Added to User PATH: $targetDir"
  Write-Host "Open a new terminal for PATH changes to apply everywhere."
}
Write-Host "Run: jaturi"
