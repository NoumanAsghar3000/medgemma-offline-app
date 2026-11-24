# Authenticode-signs one file with the PFX prepared by the 'Prepare code signing' CI step.
# Used directly by the workflow AND as Inno Setup's SignTool (so the installer and the uninstaller get signed).
# Needs env: SIGNTOOL (path to signtool.exe), CERT_PATH (.pfx), WINDOWS_CERTIFICATE_PASSWORD (may be empty for a password-less PFX).
# Works with a self-signed certificate (see tools/new_selfsigned_cert.ps1): the timestamp keeps the signature valid after the certificate expires.
param([Parameter(Mandatory = $true, Position = 0)][string]$File)
$ErrorActionPreference = 'Stop'
$File = $File.Trim('"')

# Fail with a precise message instead of a confusing signtool error three levels down (this script runs inside Inno Setup's compiler).
if (-not $env:SIGNTOOL -or -not (Test-Path -LiteralPath $env:SIGNTOOL)) { Write-Host "sign.ps1: SIGNTOOL is not set or does not exist: '$env:SIGNTOOL'"; exit 1 }
if (-not $env:CERT_PATH -or -not (Test-Path -LiteralPath $env:CERT_PATH)) { Write-Host "sign.ps1: CERT_PATH is not set or does not exist: '$env:CERT_PATH'"; exit 1 }
if (-not (Test-Path -LiteralPath $File)) { Write-Host "sign.ps1: the file to sign does not exist: $File"; exit 1 }

# The password argument is only passed when there is one. An EMPTY string argument is silently dropped by Windows PowerShell 5.1 (Inno Setup
# starts this script with powershell.exe), which would shift every following signtool argument and fail the signing with a cryptic message.
$pw = @()
if ($env:WINDOWS_CERTIFICATE_PASSWORD) { $pw = @('/p', $env:WINDOWS_CERTIFICATE_PASSWORD) }

# several timestamp servers, two rounds: a timestamp keeps the signature valid after the certificate expires
$servers = @('http://timestamp.digicert.com', 'http://timestamp.sectigo.com', 'http://timestamp.globalsign.com/tsa/r6advanced1')
foreach ($round in 1..2) {
  foreach ($ts in $servers) {
    & $env:SIGNTOOL sign /fd SHA256 /f $env:CERT_PATH @pw /tr $ts /td SHA256 /d 'MedGemma Offline' $File
    if ($LASTEXITCODE -eq 0) {
      # belt and braces: the file must really carry a signature now (a self-signed chain reports UnknownError, which is fine)
      $sig = Get-AuthenticodeSignature -LiteralPath $File -ErrorAction SilentlyContinue
      if ($sig -and -not $sig.SignerCertificate) { Write-Host "sign.ps1: signtool reported success but $File has no signature"; exit 1 }
      exit 0
    }
    Write-Host "signtool failed against $ts (exit $LASTEXITCODE), trying the next timestamp server"
    Start-Sleep -Seconds 3
  }
}
Write-Host "Could not sign $File"
exit 1
