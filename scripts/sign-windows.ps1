<#
  Signs one binary with Authenticode, for Tauri's bundle.windows.signCommand.

  Tauri replaces the %1 placeholder in the configured args with the binary it
  just produced (the app exe, then the NSIS installer), so this script is the
  single signing entry point for both artefacts.

  Credentials come from the environment, never from the repository:
    PHL_SIGN_PFX_BASE64 + PHL_SIGN_PFX_PASSWORD   certificate as a base64 PFX (CI)
    PHL_SIGN_PFX_PATH   + PHL_SIGN_PFX_PASSWORD   certificate on disk (local)
    PHL_SIGN_THUMBPRINT                           certificate already in the store
  Optional:
    PHL_SIGN_TIMESTAMP_URL   default http://timestamp.digicert.com
    PHL_SIGN_REQUIRED=1      fail when no credential is configured

  With no credential the script prints why and exits 0, so an unsigned local
  build still works. The release workflow sets PHL_SIGN_REQUIRED=1, so a
  misconfigured release fails loudly instead of publishing unsigned binaries.
#>
param([Parameter(Mandatory = $true, Position = 0)][string]$File)

$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $File)) { Write-Error "sign-windows: file not found: $File"; exit 1 }

function Get-SignTool {
  $candidates = @()
  if ($env:PHL_SIGNTOOL) { $candidates += $env:PHL_SIGNTOOL }
  $roots = @("${env:ProgramFiles(x86)}\Windows Kits\10\bin", "$env:ProgramFiles\Windows Kits\10\bin")
  foreach ($root in $roots) {
    if (Test-Path -LiteralPath $root) {
      $candidates += Get-ChildItem -LiteralPath $root -Directory |
        Sort-Object Name -Descending |
        ForEach-Object { Join-Path $_.FullName "x64\signtool.exe" }
    }
  }
  foreach ($c in $candidates) { if ($c -and (Test-Path -LiteralPath $c)) { return $c } }
  $onPath = Get-Command signtool.exe -ErrorAction SilentlyContinue
  if ($onPath) { return $onPath.Source }
  return $null
}

$pfxPath = $env:PHL_SIGN_PFX_PATH
$tempPfx = $null
if ($env:PHL_SIGN_PFX_BASE64) {
  $tempPfx = Join-Path $env:RUNNER_TEMP ("phl-sign-" + [guid]::NewGuid().ToString("N") + ".pfx")
  if (-not $env:RUNNER_TEMP) { $tempPfx = Join-Path ([IO.Path]::GetTempPath()) ("phl-sign-" + [guid]::NewGuid().ToString("N") + ".pfx") }
  [IO.File]::WriteAllBytes($tempPfx, [Convert]::FromBase64String($env:PHL_SIGN_PFX_BASE64))
  $pfxPath = $tempPfx
}

$credential = $null
if ($pfxPath) { $credential = "pfx" }
elseif ($env:PHL_SIGN_THUMBPRINT) { $credential = "thumbprint" }

if (-not $credential) {
  if ($env:PHL_SIGN_REQUIRED -eq "1") {
    Write-Error "sign-windows: signing is required but no certificate is configured (PHL_SIGN_PFX_BASE64 / PHL_SIGN_PFX_PATH / PHL_SIGN_THUMBPRINT)"
    exit 1
  }
  Write-Host "sign-windows: no certificate configured, leaving $File unsigned (cwd=$($PWD.Path))"
  exit 0
}

$signtool = Get-SignTool
if (-not $signtool) { Write-Error "sign-windows: signtool.exe not found; install the Windows SDK or set PHL_SIGNTOOL"; exit 1 }

$timestamp = $env:PHL_SIGN_TIMESTAMP_URL
if (-not $timestamp) { $timestamp = "http://timestamp.digicert.com" }

$args = @("sign", "/fd", "sha256", "/td", "sha256", "/tr", $timestamp)
if ($credential -eq "thumbprint") {
  $args += @("/sha1", $env:PHL_SIGN_THUMBPRINT, "/s", "My")
} else {
  $args += @("/f", $pfxPath)
  if ($env:PHL_SIGN_PFX_PASSWORD) { $args += @("/p", $env:PHL_SIGN_PFX_PASSWORD) }
}
$args += $File

Write-Host "sign-windows: signing $File (cwd=$($PWD.Path))"
& $signtool @args
$code = $LASTEXITCODE
if ($tempPfx -and (Test-Path -LiteralPath $tempPfx)) { Remove-Item -LiteralPath $tempPfx -Force }
if ($code -ne 0) { Write-Error "sign-windows: signtool exited $code"; exit $code }

& $signtool verify /pa /v $File | Out-Null
if ($LASTEXITCODE -ne 0) { Write-Error "sign-windows: verification failed for $File"; exit $LASTEXITCODE }
Write-Host "sign-windows: $File signed and verified"
