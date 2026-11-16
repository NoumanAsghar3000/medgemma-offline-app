# Drives the REAL installer wizard (real mg-probe.exe, real Windows) with keystrokes, takes screenshots, and asserts on the
# state lines the installer writes to its own log ("WIZARD ..."). Evidence step: needs an interactive desktop session.
param([Parameter(Mandatory)][string]$Setup, [Parameter(Mandatory)][string]$ModelDir, [Parameter(Mandatory)][string]$OutDir)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
New-Item -ItemType Directory -Force $OutDir | Out-Null
$log = Join-Path $OutDir 'wizard.log'; Remove-Item $log -ErrorAction SilentlyContinue
function Shot($name) {
  $b = [System.Windows.Forms.SystemInformation]::VirtualScreen
  $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
  [System.Drawing.Graphics]::FromImage($bmp).CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
  $bmp.Save((Join-Path $OutDir "$name.png")); $bmp.Dispose()
}
function Keys($k) { [System.Windows.Forms.SendKeys]::SendWait($k); Start-Sleep -Milliseconds 1800 }
function Esc($s) { ($s.ToCharArray() | ForEach-Object { if ('+^%~(){}[]'.Contains([string]$_)) { "{$_}" } else { "$_" } }) -join '' }

$p = Start-Process $Setup -ArgumentList "/LOG=`"$log`"", "/DIR=`"$env:RUNNER_TEMP\wizard-target`"" -PassThru
Start-Sleep -Seconds 12
$sh = New-Object -ComObject WScript.Shell; [void]$sh.AppActivate($p.Id); Start-Sleep 1
Shot '1-destination'
Keys '%n'                       # Next -> processor page
Shot '2-processor-cpu'
Keys '{DOWN}'; Keys ' '         # focus + select the GPU radio
Shot '3-processor-gpu'
Keys '{UP}'; Keys ' '           # back to CPU
Keys '%n'                       # Next -> model folder page
Keys '^a'; Keys (Esc $ModelDir); Start-Sleep 4
Shot '4-model-page'
try { $p.Kill() } catch {}

$lines = if (Test-Path $log) { Get-Content $log | Where-Object { $_ -match 'WIZARD' } } else { @() }
$lines | ForEach-Object { $_ -replace '^\S+ \S+\s+', '' } | Tee-Object (Join-Path $OutDir 'wizard-state.txt')
$fail = @()
if (-not ($lines -match 'compute: gpu_selected=0 .* next_enabled=1')) { $fail += 'CPU selected -> Next must be enabled' }
$gpuLine = $lines | Where-Object { $_ -match 'gpu_selected=1' } | Select-Object -First 1
if (-not $gpuLine) { $fail += 'GPU selection was never logged' }
elseif ($gpuLine -match 'gpu_ok=0') {
  if ($gpuLine -notmatch 'next_enabled=0') { $fail += 'GPU not available -> Next must be DISABLED' }
  if ($gpuLine -notmatch 'GPU is not available currently in this system') { $fail += 'red warning text missing' }
} elseif ($gpuLine -notmatch 'next_enabled=1') { $fail += 'GPU available -> Next must be enabled' }
if (-not ($lines -match 'model: ok=1')) { $fail += 'model page did not accept the valid model folder' }
if ($fail) { $fail | ForEach-Object { Write-Error $_ -ErrorAction Continue }; exit 1 }
Write-Host 'WIZARD ASSERTIONS PASSED'
