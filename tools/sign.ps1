# Authenticode-signs one file with the PFX prepared by the 'Prepare code signing' CI step.
# Used directly by the workflow AND as Inno Setup's SignTool (so the installer and the uninstaller get signed).
# Needs env: SIGNTOOL (path to signtool.exe), CERT_PATH (.pfx), WINDOWS_CERTIFICATE_PASSWORD.
param([Parameter(Mandatory = $true, Position = 0)][string]$File)
$ErrorActionPreference = 'Stop'
$File = $File.Trim('"')

# several timestamp servers: a timestamp keeps the signature valid after the certificate expires
$servers = @('http://timestamp.digicert.com', 'http://timestamp.sectigo.com', 'http://timestamp.globalsign.com/tsa/r6advanced1')
foreach ($ts in $servers) {
  & $env:SIGNTOOL sign /fd SHA256 /f $env:CERT_PATH /p $env:WINDOWS_CERTIFICATE_PASSWORD /tr $ts /td SHA256 /d 'MedGemma Offline' $File
  if ($LASTEXITCODE -eq 0) { exit 0 }
  Write-Host "signtool failed against $ts (exit $LASTEXITCODE), trying the next timestamp server"
  Start-Sleep -Seconds 3
}
Write-Host "Could not sign $File"
exit 1
