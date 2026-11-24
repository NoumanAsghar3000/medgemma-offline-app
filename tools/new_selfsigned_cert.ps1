<#
  Creates a SELF-SIGNED code-signing certificate for MedGemma Offline and writes the two values the build workflow expects as
  GitHub repository secrets:
      WINDOWS_CERTIFICATE            base64 text of the .pfx file          (-> OutDir\WINDOWS_CERTIFICATE.txt)
      WINDOWS_CERTIFICATE_PASSWORD   the password protecting the .pfx      (-> OutDir\WINDOWS_CERTIFICATE_PASSWORD.txt)
  Run it ONCE on a Windows PC, copy the two values into  Settings -> Secrets and variables -> Actions, then DELETE the output folder
  (or keep it somewhere private: whoever has the .pfx can sign as you). Never commit it.

  usage:  powershell -ExecutionPolicy Bypass -File tools\new_selfsigned_cert.ps1 [-OutDir .\cert] [-Subject "CN=Nouman"] [-Years 5]

  A self-signed certificate proves that the installer was not changed after it was built, but Windows cannot tell WHO built it:
  SmartScreen still shows its warning on first run (More info -> Run anyway). Do not tell users to install the certificate as a trusted root.
  The matching public certificate (.cer) is written as well, for people who want to compare the signer's thumbprint.
#>
param(
  [string]$OutDir = (Join-Path (Get-Location) 'cert'),
  [string]$Subject = 'CN=Nouman, O=MedGemma Offline',
  [ValidateRange(1, 20)][int]$Years = 5,
  [string]$Password
)
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSEdition -eq 'Core' -and -not $IsWindows) { throw 'Run this script on Windows (New-SelfSignedCertificate is a Windows cmdlet).' }

if (-not $Password) {
  $bytes = New-Object byte[] 18
  $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create(); $rng.GetBytes($bytes); $rng.Dispose()
  $Password = [Convert]::ToBase64String($bytes)
}
$secure = ConvertTo-SecureString -String $Password -AsPlainText -Force
New-Item -ItemType Directory -Force $OutDir | Out-Null

$cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject $Subject -KeyAlgorithm RSA -KeyLength 3072 -HashAlgorithm SHA256 `
  -KeyExportPolicy Exportable -KeyUsage DigitalSignature -FriendlyName 'MedGemma Offline code signing' `
  -CertStoreLocation 'Cert:\CurrentUser\My' -NotAfter (Get-Date).AddYears($Years)
try {
  $pfx = Join-Path $OutDir 'medgemma-codesign.pfx'
  Export-PfxCertificate -Cert $cert -FilePath $pfx -Password $secure | Out-Null
  Export-Certificate -Cert $cert -FilePath (Join-Path $OutDir 'medgemma-codesign.cer') | Out-Null
  Set-Content -Path (Join-Path $OutDir 'WINDOWS_CERTIFICATE.txt') -Value ([Convert]::ToBase64String([IO.File]::ReadAllBytes($pfx))) -Encoding ascii
  Set-Content -Path (Join-Path $OutDir 'WINDOWS_CERTIFICATE_PASSWORD.txt') -Value $Password -Encoding ascii
} finally {
  Remove-Item -LiteralPath ("Cert:\CurrentUser\My\" + $cert.Thumbprint) -ErrorAction SilentlyContinue    # the key only lives in the exported files now
}
"Created a code-signing certificate"
"  Subject    : $($cert.Subject)"
"  Thumbprint : $($cert.Thumbprint)"
"  Valid until: $($cert.NotAfter)"
"Files in $OutDir :"
Get-ChildItem $OutDir | ForEach-Object { "  $($_.Name)" }
""
"Next: create two repository secrets (Settings -> Secrets and variables -> Actions -> New repository secret):"
"  WINDOWS_CERTIFICATE           = the ENTIRE content of WINDOWS_CERTIFICATE.txt"
"  WINDOWS_CERTIFICATE_PASSWORD  = the content of WINDOWS_CERTIFICATE_PASSWORD.txt"
"Then delete or move the folder '$OutDir' somewhere private. Never commit it."
