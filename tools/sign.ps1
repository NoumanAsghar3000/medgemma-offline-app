# Authenticode-signs one file with the PFX prepared by the 'Prepare code signing' CI step.
# Used directly by the workflow (pwsh 7) AND as Inno Setup's SignTool, where Inno starts it with powershell.exe (Windows PowerShell 5.1)
# from a pwsh 7 step, to sign the installer and the uninstaller.
# Needs env: SIGNTOOL (path to signtool.exe), CERT_PATH (.pfx), WINDOWS_CERTIFICATE_PASSWORD (may be empty for a password-less PFX).
# Works with a self-signed certificate (see tools/new_selfsigned_cert.ps1): the timestamp keeps the signature valid after the certificate expires.
#
# WHY THIS SCRIPT USES NO MODULE CMDLETS (Get-AuthenticodeSignature, Import-Module, ...):
# powershell.exe started from a pwsh 7 step inherits pwsh's PSModulePath. Windows PowerShell 5.1 then finds PowerShell 7's copy of
# Microsoft.PowerShell.Security first and fails with "The 'Get-AuthenticodeSignature' command was found in the module
# 'Microsoft.PowerShell.Security', but the module could not be loaded." Only language features, .NET and built-in core cmdlets are used here,
# and the inherited module path is cleaned for Windows PowerShell as well.
param([Parameter(Mandatory = $true, Position = 0)][string]$File)
$ErrorActionPreference = 'Stop'

# PSModulePath without PowerShell 7's own folders (...\PowerShell\7\Modules, ...\PowerShell\Modules). 'WindowsPowerShell' entries stay.
function Get-CleanModulePath([string]$Path) {
  if (-not $Path) { return $Path }
  $keep = New-Object System.Collections.Generic.List[string]
  foreach ($entry in $Path.Split(';')) {
    if ($entry -and ($entry -notmatch '\\PowerShell\\')) { $keep.Add($entry) }
  }
  return [string]::Join(';', $keep.ToArray())
}
if ($PSVersionTable.PSEdition -eq 'Desktop') { $env:PSModulePath = Get-CleanModulePath $env:PSModulePath }

$File = $File.Trim('"')

# Fail with a precise message instead of a confusing signtool error three levels down (this script runs inside Inno Setup's compiler).
if (-not $env:SIGNTOOL -or -not [System.IO.File]::Exists($env:SIGNTOOL)) { Write-Host "sign.ps1: SIGNTOOL is not set or does not exist: '$env:SIGNTOOL'"; exit 1 }
if (-not $env:CERT_PATH -or -not [System.IO.File]::Exists($env:CERT_PATH)) { Write-Host "sign.ps1: CERT_PATH is not set or does not exist: '$env:CERT_PATH'"; exit 1 }
if (-not [System.IO.File]::Exists($File)) { Write-Host "sign.ps1: the file to sign does not exist: $File"; exit 1 }

# The password argument is only passed when there is one. An EMPTY string argument is silently dropped by Windows PowerShell 5.1,
# which would shift every following signtool argument and fail the signing with a cryptic message.
$pw = @()
if ($env:WINDOWS_CERTIFICATE_PASSWORD) { $pw = @('/p', $env:WINDOWS_CERTIFICATE_PASSWORD) }

# several timestamp servers, two rounds: a timestamp keeps the signature valid after the certificate expires
$servers = @('http://timestamp.digicert.com', 'http://timestamp.sectigo.com', 'http://timestamp.globalsign.com/tsa/r6advanced1')
foreach ($round in 1..2) {
  foreach ($ts in $servers) {
    & $env:SIGNTOOL sign /fd SHA256 /f $env:CERT_PATH @pw /tr $ts /td SHA256 /d 'MedGemma Offline' $File
    if ($LASTEXITCODE -eq 0) {
      # signtool's exit code 0 is the success criterion (it also means the timestamp was obtained). The line below is only a log aid.
      try { Write-Host ('sign.ps1: signed ' + $File + ' as ' + [System.Security.Cryptography.X509Certificates.X509Certificate]::CreateFromSignedFile($File).Subject) }
      catch { Write-Host "sign.ps1: signed $File" }
      exit 0
    }
    Write-Host "signtool failed against $ts (exit $LASTEXITCODE), trying the next timestamp server"
    [System.Threading.Thread]::Sleep(3000)
  }
}
Write-Host "Could not sign $File"
exit 1
