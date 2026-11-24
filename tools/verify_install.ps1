# Asserts what a commercial Windows installer must guarantee. Phase 'installed' after (silent) install, 'uninstalled' after uninstall.
# -Compute says which engine set the install was made with: a CPU install must NOT contain the Vulkan engine (the installer
# only copies backend\vulkan when GPU is selected), a GPU install must contain both.
# Also asserts the hardening done by installer\medgemma.iss: the install folder is writable only by SYSTEM + Administrators, the inference
# engine has an outbound-block firewall rule (removed again on uninstall), and install.json records the chosen processor.
param([Parameter(Mandatory)][ValidateSet('installed','uninstalled')][string]$Phase, [Parameter(Mandatory)][string]$AppDir, [string]$Version = '', [switch]$DesktopIcon, [ValidateSet('cpu','gpu')][string]$Compute = 'cpu')
$ErrorActionPreference = 'Stop'
$key  = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{B7C0A3E4-5D2F-4A57-9F43-6D1E2F0C9A11}_is1'
$menu = Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs\MedGemma Offline'
$desk = Join-Path $env:PUBLIC 'Desktop\MedGemma Offline.lnk'
$data = @("$env:APPDATA\com.nouman.medgemma.offline", "$env:LOCALAPPDATA\com.nouman.medgemma.offline")
$fwName = 'MedGemma Offline engine (no network)'
$fail = @(); function Need($ok, $msg) { if ($ok) { Write-Host "PASS  $msg" } else { Write-Host "FAIL  $msg"; $script:fail += $msg } }

# Problems in the permissions of the install folder ($acl = Get-Acl result). Compared by SID, so it works on any Windows language.
# Expected: inheritance switched off, and only SYSTEM (S-1-5-18), Administrators (S-1-5-32-544) and Users (S-1-5-32-545) allowed,
# with Users limited to read/execute.
function Get-AclProblems($acl) {
  $problems = @()
  if (-not $acl.AreAccessRulesProtected) { $problems += 'permissions are still inherited from the parent folder' }
  $allowed = @('S-1-5-18', 'S-1-5-32-544', 'S-1-5-32-545')
  $writeMask = [System.Security.AccessControl.FileSystemRights]'Write, Delete, DeleteSubdirectoriesAndFiles, ChangePermissions, TakeOwnership'
  foreach ($r in $acl.Access) {
    if ("$($r.AccessControlType)" -ne 'Allow') { continue }
    $sid = $r.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value
    if ($allowed -notcontains $sid) { $problems += "unexpected principal in the ACL: $($r.IdentityReference)"; continue }
    if ($sid -eq 'S-1-5-32-545' -and (([int]$r.FileSystemRights) -band ([int]$writeMask))) { $problems += "standard users can modify the folder ($($r.FileSystemRights))" }
  }
  return $problems
}

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

  # --- the processor choice is recorded for the app ---
  $ij = Join-Path $AppDir 'install.json'
  Need (Test-Path $ij) 'install.json written'
  if (Test-Path $ij) { $j = Get-Content $ij -Raw | ConvertFrom-Json; Need ($j.compute -eq $Compute) "install.json compute = '$($j.compute)' (expected '$Compute')" }

  # --- only administrators may change the program files (otherwise a standard user could plant code that runs for other users) ---
  $aclProblems = @(Get-AclProblems (Get-Acl -LiteralPath $AppDir))
  Need ($aclProblems.Count -eq 0) ("install folder permissions: SYSTEM + Administrators full control, Users read/execute only" + $(if ($aclProblems) { ' -> ' + ($aclProblems -join '; ') } else { '' }))

  # --- "offline" enforced by the OS: the engine may not use the network ---
  $fwSvc = Get-Service -Name 'mpssvc' -ErrorAction SilentlyContinue
  if ($fwSvc -and $fwSvc.Status -eq 'Running' -and (Get-Command Get-NetFirewallRule -ErrorAction SilentlyContinue)) {
    $rules = @(Get-NetFirewallRule -DisplayName $fwName -ErrorAction SilentlyContinue)
    Need ($rules.Count -ge 1) "firewall: rule '$fwName' exists"
    if ($rules.Count -ge 1) {
      Need (@($rules | Where-Object { "$($_.Direction)" -eq 'Outbound' -and "$($_.Action)" -eq 'Block' -and "$($_.Enabled)" -eq 'True' }).Count -eq $rules.Count) 'firewall: every rule is an ENABLED outbound BLOCK'
      $progs = @($rules | ForEach-Object { (Get-NetFirewallApplicationFilter -AssociatedNetFirewallRule $_).Program })
      Need (@($progs | Where-Object { $_ -like '*\backend\cpu\llama-server.exe' }).Count -ge 1) 'firewall: the CPU engine is covered'
      if ($Compute -eq 'gpu') { Need (@($progs | Where-Object { $_ -like '*\backend\vulkan\llama-server.exe' }).Count -ge 1) 'firewall: the Vulkan engine is covered' }
      else { Need (@($progs | Where-Object { $_ -like '*\backend\vulkan\*' }).Count -eq 0) 'firewall: no rule for a Vulkan engine that is not installed' }
    }
  } else { Write-Host 'SKIP  firewall checks (the Windows Firewall service is not running on this machine)' }
} else {
  Need (-not (Test-Path $key)) 'Uninstall key removed (entry gone from Apps & features)'
  Need (-not (Test-Path $menu)) 'Start Menu folder removed'
  Need (-not (Test-Path $desk)) 'Desktop shortcut removed'
  Need (-not (Test-Path $AppDir)) "install folder removed completely: $AppDir"
  if (Test-Path $AppDir) { Get-ChildItem $AppDir -Recurse -Force | ForEach-Object { Write-Host "      leftover: $($_.FullName)" } }
  foreach ($d in $data) { Need (-not (Test-Path $d)) "per-user data removed: $d" }
  if (Get-Command Get-NetFirewallRule -ErrorAction SilentlyContinue) { Need (@(Get-NetFirewallRule -DisplayName $fwName -ErrorAction SilentlyContinue).Count -eq 0) "firewall rule '$fwName' removed" }
  Need (-not (Get-Process -Name 'medgemma-offline','llama-server' -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$AppDir*" })) 'no process left running from the install folder'
}
if ($fail) { Write-Error "install verification ($Phase) failed: $($fail -join '; ')"; exit 1 }
Write-Host "INSTALL VERIFICATION ($Phase) PASSED"
