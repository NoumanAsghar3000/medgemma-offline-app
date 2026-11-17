# Asserts what a commercial Windows installer must guarantee. Phase 'installed' after (silent) install, 'uninstalled' after uninstall.
# -Compute says which engine set the install was made with: a CPU install must NOT contain the Vulkan engine (the installer
# only copies backend\vulkan when GPU is selected), a GPU install must contain both.
param([Parameter(Mandatory)][ValidateSet('installed','uninstalled')][string]$Phase, [Parameter(Mandatory)][string]$AppDir, [string]$Version = '', [switch]$DesktopIcon, [ValidateSet('cpu','gpu')][string]$Compute = 'cpu')
$ErrorActionPreference = 'Stop'
$key  = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{B7C0A3E4-5D2F-4A57-9F43-6D1E2F0C9A11}_is1'
$menu = Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs\MedGemma Offline'
$desk = Join-Path $env:PUBLIC 'Desktop\MedGemma Offline.lnk'
$data = @("$env:APPDATA\com.nouman.medgemma.offline", "$env:LOCALAPPDATA\com.nouman.medgemma.offline")
$fail = @(); function Need($ok, $msg) { if ($ok) { Write-Host "PASS  $msg" } else { Write-Host "FAIL  $msg"; $script:fail += $msg } }

if ($Phase -eq 'installed') {
  $k = Get-ItemProperty $key -ErrorAction SilentlyContinue
  Need ($null -ne $k) 'registered in Apps & features (Uninstall key exists)'
  if ($k) {
    Need ($k.DisplayName -eq 'MedGemma Offline') "DisplayName = '$($k.DisplayName)'"
    if ($Version) { Need ($k.DisplayVersion -eq $Version) "DisplayVersion = '$($k.DisplayVersion)' (expected '$Version')" }
    Need ($k.Publisher -eq 'Nouman') "Publisher = '$($k.Publisher)'"
    Need (Test-Path ($k.UninstallString.Trim('"').Split('"')[0])) 'UninstallString points to an existing uninstaller'
  }
  Need ((Get-ChildItem (Split-Path $key) | Where-Object { $_.PSChildName -like '{B7C0A3E4*' }).Count -eq 1) 'exactly one uninstall entry (no duplicates after upgrade)'
  Need (Test-Path "$menu\MedGemma Offline.lnk") 'Start Menu shortcut'
  Need (Test-Path "$menu\Uninstall MedGemma Offline.lnk") 'Start Menu uninstall shortcut'
  if ($DesktopIcon) { Need (Test-Path $desk) 'optional Desktop shortcut created (task selected)' } else { Need (-not (Test-Path $desk)) 'Desktop shortcut NOT created when the task is off' }
  foreach ($f in 'medgemma-offline.exe','unins000.exe','tools\mg-probe.exe','backend\cpu\llama-server.exe','backend\cpu\llama-server-impl.dll') { Need (Test-Path "$AppDir\$f") "installed file: $f" }
  if ($Compute -eq 'gpu') { Need (Test-Path "$AppDir\backend\vulkan\llama-server.exe") 'installed file: backend\vulkan\llama-server.exe (GPU install)' }
  else { Need (-not (Test-Path "$AppDir\backend\vulkan")) 'Vulkan engine NOT installed for a CPU install (backend\vulkan absent)' }
} else {
  Need (-not (Test-Path $key)) 'Uninstall key removed (entry gone from Apps & features)'
  Need (-not (Test-Path $menu)) 'Start Menu folder removed'
  Need (-not (Test-Path $desk)) 'Desktop shortcut removed'
  Need (-not (Test-Path $AppDir)) "install folder removed completely: $AppDir"
  if (Test-Path $AppDir) { Get-ChildItem $AppDir -Recurse -Force | ForEach-Object { Write-Host "      leftover: $($_.FullName)" } }
  foreach ($d in $data) { Need (-not (Test-Path $d)) "per-user data removed: $d" }
  Need (-not (Get-Process -Name 'medgemma-offline','llama-server' -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$AppDir*" })) 'no process left running from the install folder'
}
if ($fail) { Write-Error "install verification ($Phase) failed: $($fail -join '; ')"; exit 1 }
Write-Host "INSTALL VERIFICATION ($Phase) PASSED"
