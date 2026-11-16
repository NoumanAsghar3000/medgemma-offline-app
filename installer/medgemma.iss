; MedGemma Offline - Inno Setup 6.3+ script. Built in CI (see .github/workflows/build.yml).
; Build flags:  /DAppVersion=1.0.0   /DBundleWebView2   (ships the offline WebView2 runtime installer)
#ifndef AppVersion
  #define AppVersion "1.0.0"
#endif

[Setup]
AppId={{B7C0A3E4-5D2F-4A57-9F43-6D1E2F0C9A11}
AppName=MedGemma Offline
AppVersion={#AppVersion}
AppPublisher=Nouman
AppVerName=MedGemma Offline {#AppVersion}
UninstallDisplayName=MedGemma Offline
DefaultGroupName=MedGemma Offline
UsePreviousAppDir=yes
UsePreviousTasks=yes
; close a running instance (and its llama-server) before upgrading / uninstalling instead of leaving locked files behind
CloseApplications=yes
RestartApplications=no
DefaultDirName={sd}\MedGemmaOffline
DisableProgramGroupPage=yes
DisableDirPage=no
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
#ifdef FastCompress
Compression=lzma2/fast
SolidCompression=yes
#else
Compression=lzma2/ultra64
SolidCompression=yes
#endif
WizardStyle=modern
SetupIconFile=..\src-tauri\icons\icon.ico
UninstallDisplayIcon={app}\medgemma-offline.exe
OutputDir=..\out
#ifdef BundleWebView2
OutputBaseFilename=MedGemmaOffline-Setup-{#AppVersion}-with-WebView2
#else
OutputBaseFilename=MedGemmaOffline-Setup-{#AppVersion}
#endif

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; Flags: unchecked

[Files]
Source: "..\staging\app\medgemma-offline.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\staging\backend\cpu\*"; DestDir: "{app}\backend\cpu"; Flags: ignoreversion recursesubdirs
Source: "..\staging\backend\vulkan\*"; DestDir: "{app}\backend\vulkan"; Flags: ignoreversion recursesubdirs; Check: GpuSelected
; hardware probe + model selector, only needed while the wizard runs
Source: "..\staging\mg-probe.exe"; Flags: dontcopy
; also installed, so a supervisor can run:  "{app}\tools\mg-probe.exe" doctor --dir <model folder>
Source: "..\staging\mg-probe.exe"; DestDir: "{app}\tools"; Flags: ignoreversion
#ifdef BundleWebView2
Source: "..\staging\MicrosoftEdgeWebView2RuntimeInstallerX64.exe"; DestDir: "{tmp}"; Flags: deleteafterinstall; Check: NeedsWebView2
#endif

[InstallDelete]
; upgrade hygiene: remove the previous version's engine/tool files first so no stale DLLs survive
Type: filesandordirs; Name: "{app}\backend"
Type: filesandordirs; Name: "{app}\tools"

[Icons]
Name: "{group}\MedGemma Offline"; Filename: "{app}\medgemma-offline.exe"
Name: "{group}\Uninstall MedGemma Offline"; Filename: "{uninstallexe}"
Name: "{autodesktop}\MedGemma Offline"; Filename: "{app}\medgemma-offline.exe"; Tasks: desktopicon

[Run]
#ifdef BundleWebView2
Filename: "{tmp}\MicrosoftEdgeWebView2RuntimeInstallerX64.exe"; Parameters: "/silent /install"; StatusMsg: "Installing Microsoft WebView2 runtime (offline)..."; Check: NeedsWebView2; Flags: waituntilterminated
#endif
Filename: "{app}\medgemma-offline.exe"; Description: "Launch MedGemma Offline"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; files created AFTER installation (so Inno does not know them). Your model folder is never touched.
Type: filesandordirs; Name: "{app}\models"
Type: files; Name: "{app}\install.json"
Type: filesandordirs; Name: "{userappdata}\com.nouman.medgemma.offline"
Type: filesandordirs; Name: "{localappdata}\com.nouman.medgemma.offline"
Type: dirifempty; Name: "{app}"

[Code]
const
  MIN_RAM_TEXT = '4 GB';

var
  ComputePage: TWizardPage;
  rbCpu, rbGpu: TNewRadioButton;
  lblHw, lblWarn: TNewStaticText;
  ModelPage: TInputDirWizardPage;
  lblPlan: TNewStaticText;
  chkCopy: TNewCheckBox;
  Hw, Plan: TStringList;
  ProbeReady: Boolean;
  ComputeOk, ModelOk: Boolean;

{ ---------- helpers ---------- }
{ Inno's TStringList has no Values[] property: look up KEY=VALUE lines ourselves }
function KVal(L: TStringList; const Key: String): String;
var
  I, P: Integer;
  Line: String;
begin
  Result := '';
  for I := 0 to L.Count - 1 do
  begin
    Line := L[I];
    P := Pos('=', Line);
    if (P > 0) and (CompareText(Copy(Line, 1, P - 1), Key) = 0) then
    begin
      Result := Trim(Copy(Line, P + 1, Length(Line)));
      Exit;
    end;
  end;
end;

function Tern(Cond: Boolean; A, B: String): String;
begin
  if Cond then Result := A else Result := B;
end;

function QuoteArg(S: String): String;
begin
  { a trailing backslash would escape the closing quote -> double it }
  if (Length(S) > 0) and (S[Length(S)] = '\') then S := S + '\';
  Result := '"' + S + '"';
end;

function RunProbe(const Args: String; KV: TStringList): Boolean;
var
  Rc: Integer;
  Outf: String;
begin
  Result := False;
  if not ProbeReady then begin
    ExtractTemporaryFile('mg-probe.exe');
    ProbeReady := True;
  end;
  Outf := ExpandConstant('{tmp}\probe.out');
  DeleteFile(Outf);
  if Exec(ExpandConstant('{tmp}\mg-probe.exe'), Args + ' --out ' + QuoteArg(Outf), '', SW_HIDE, ewWaitUntilTerminated, Rc)
     and (Rc = 0) and FileExists(Outf) then
  begin
    KV.LoadFromFile(Outf);
    Result := True;
  end;
end;

function GpuSelected: Boolean;
begin
  Result := (rbGpu <> nil) and rbGpu.Checked;
end;

function ComputeName: String;
begin
  if GpuSelected then Result := 'gpu' else Result := 'cpu';
end;

function NeedsWebView2: Boolean;
var
  V: String;
begin
  Result := True;
  if RegQueryStringValue(HKLM32, 'SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'pv', V)
     and (V <> '') and (V <> '0.0.0.0') then Result := False;
  if Result and RegQueryStringValue(HKCU, 'Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}', 'pv', V)
     and (V <> '') and (V <> '0.0.0.0') then Result := False;
end;

{ ---------- page 1: CPU / GPU with LIVE hardware detection ---------- }
procedure UpdateComputePageCore;
begin
  ComputeOk := False;
  if not RunProbe('hw', Hw) then
  begin
    lblWarn.Caption := 'Hardware detection failed. Setup cannot continue.';
    WizardForm.NextButton.Enabled := False;
    Exit;
  end;

  lblHw.Caption := 'Installed RAM: ' + KVal(Hw, 'total_gb') + ' GB     Free RAM now: ' + KVal(Hw, 'free_gb') + ' GB' + #13#10 +
    'GPU: ' + Tern(KVal(Hw, 'gpu_ok') = '1', KVal(Hw, 'gpu_name') + ' (' + KVal(Hw, 'vram_gb') + ' GB VRAM)', 'none detected');

  if KVal(Hw, 'ram_ok') <> '1' then
  begin
    { hard rule: < 4 GB installed RAM -> block, red text on this same page }
    rbCpu.Enabled := False;
    rbGpu.Enabled := False;
    lblWarn.Caption := 'This computer has only ' + KVal(Hw, 'total_gb') + ' GB of installed RAM. A minimum of ' + MIN_RAM_TEXT + ' is required, so MedGemma Offline cannot be installed.';
    WizardForm.NextButton.Enabled := False;
    Exit;
  end;

  if rbGpu.Checked and (KVal(Hw, 'gpu_ok') <> '1') then
  begin
    lblWarn.Caption := 'GPU is not available currently in this system';   { red text on the same dialog, no message box }
    WizardForm.NextButton.Enabled := False;       { Next stays disabled until an available option is chosen }
  end
  else
  begin
    lblWarn.Caption := '';
    ComputeOk := True;
    WizardForm.NextButton.Enabled := True;
  end;
end;

{ Writes the wizard's decision to the Setup log (/LOG) so CI can assert on the REAL wizard's behaviour }
procedure LogState(const Page, Detail: String);
begin
  Log('WIZARD ' + Page + ': ' + Detail + ' next_enabled=' + Tern(WizardForm.NextButton.Enabled, '1', '0'));
end;

procedure UpdateComputePage;
begin
  UpdateComputePageCore;
  LogState('compute', 'gpu_selected=' + Tern(rbGpu.Checked, '1', '0') + ' gpu_ok=' + KVal(Hw, 'gpu_ok') + ' ram_ok=' + KVal(Hw, 'ram_ok') + ' warn=[' + lblWarn.Caption + ']');
end;

procedure ComputeClick(Sender: TObject);
begin
  UpdateComputePage;
end;

{ ---------- page 2: model folder + automatic variant choice ---------- }
procedure UpdateModelPageCore;
var
  D: String;
begin
  ModelOk := False;
  lblPlan.Font.Color := clRed;
  D := Trim(ModelPage.Values[0]);
  if (D = '') or (not DirExists(D)) then
  begin
    lblPlan.Caption := 'Select the folder (hard disk or USB) that contains the MedGemma model files.';
  end
  else if not RunProbe('plan --dir ' + QuoteArg(D) + ' --compute ' + ComputeName, Plan) then
  begin
    lblPlan.Caption := 'Could not analyse the folder.';
  end
  else if KVal(Plan, 'status') <> 'ok' then
  begin
    lblPlan.Caption := KVal(Plan, 'message');
  end
  else
  begin
    ModelOk := True;
    lblPlan.Font.Color := clWindowText;
    lblPlan.Caption := 'Selected automatically: ' + KVal(Plan, 'family') + ' (' + KVal(Plan, 'quant') + ')' +
      Tern(KVal(Plan, 'multimodal') = '1', ' - multimodal (images + documents)', ' - TEXT ONLY, no vision file found') + #13#10 +
      'Estimated speed on this PC: ' + KVal(Plan, 'speed') + ' (' + KVal(Plan, 'verdict') + ').' + #13#10 +
      'Memory: ' + KVal(Plan, 'gpu_gb') + ' GB in GPU, ' + KVal(Plan, 'ram_gb') + ' GB in RAM' +
      Tern(KVal(Plan, 'streamed_gb') <> '0.0', ', ' + KVal(Plan, 'streamed_gb') + ' GB re-read from the drive for every token', '') +
      ' (RAM usable now: ' + KVal(Plan, 'budget_gb') + ' GB = free RAM minus 1 GB kept for Windows).' + #13#10 +
      'Drive speed ' + KVal(Plan, 'disk_mbps') + ' MB/s, memory speed ' + KVal(Plan, 'ram_gbps') + ' GB/s.' +
      Tern(KVal(Plan, 'note') <> '', #13#10 + KVal(Plan, 'note'), '');
  end;
  if WizardForm.CurPageID = ModelPage.ID then WizardForm.NextButton.Enabled := ModelOk;
end;

procedure UpdateModelPage;
begin
  UpdateModelPageCore;
  LogState('model', 'ok=' + Tern(ModelOk, '1', '0') + ' choice=[' + KVal(Plan, 'family') + ' ' + KVal(Plan, 'quant') + '] speed=[' + KVal(Plan, 'speed') + '] verdict=[' + KVal(Plan, 'verdict') + ']');
end;

procedure ModelDirChange(Sender: TObject);
begin
  UpdateModelPage;
end;

{ ---------- wizard construction ---------- }
procedure InitializeWizard;
begin
  Hw := TStringList.Create;
  Plan := TStringList.Create;

  ComputePage := CreateCustomPage(wpSelectDir, 'Processor selection', 'Choose where the AI model will run.');
  rbCpu := TNewRadioButton.Create(ComputePage);
  rbCpu.Parent := ComputePage.Surface;
  rbCpu.Caption := 'Run backend model on CPU';
  rbCpu.Width := ComputePage.SurfaceWidth;
  rbCpu.Top := 0;
  rbGpu := TNewRadioButton.Create(ComputePage);
  rbGpu.Parent := ComputePage.Surface;
  rbGpu.Caption := 'Run backend model on GPU';
  rbGpu.Width := ComputePage.SurfaceWidth;
  rbGpu.Top := rbCpu.Top + rbCpu.Height + ScaleY(10);
  rbCpu.Checked := ExpandConstant('{param:COMPUTE|cpu}') <> 'gpu';
  rbGpu.Checked := not rbCpu.Checked;

  lblHw := TNewStaticText.Create(ComputePage);
  lblHw.Parent := ComputePage.Surface;
  lblHw.AutoSize := False;
  lblHw.WordWrap := True;
  lblHw.Width := ComputePage.SurfaceWidth;
  lblHw.Height := ScaleY(40);
  lblHw.Top := rbGpu.Top + rbGpu.Height + ScaleY(24);

  lblWarn := TNewStaticText.Create(ComputePage);
  lblWarn.Parent := ComputePage.Surface;
  lblWarn.AutoSize := False;
  lblWarn.WordWrap := True;
  lblWarn.Width := ComputePage.SurfaceWidth;
  lblWarn.Height := ScaleY(48);
  lblWarn.Top := lblHw.Top + lblHw.Height + ScaleY(8);
  lblWarn.Font.Color := clRed;
  lblWarn.Font.Style := [fsBold];

  ModelPage := CreateInputDirPage(ComputePage.ID, 'MedGemma model files',
    'Where are the MedGemma model files?',
    'Select the folder (hard disk or USB drive) that contains the MedGemma .gguf files. Setup picks the largest variant that runs at a usable speed on this PC: 27B multimodal (8-bit first), and 4B only if 27B would be too slow. Files stay where they are unless you tick the box below.',
    False, '');
  ModelPage.Add('');
  ModelPage.Values[0] := ExpandConstant('{param:MODELDIR|}');

  lblPlan := TNewStaticText.Create(ModelPage);
  lblPlan.Parent := ModelPage.Surface;
  lblPlan.AutoSize := False;
  lblPlan.WordWrap := True;
  lblPlan.Width := ModelPage.SurfaceWidth;
  lblPlan.Height := ScaleY(112);
  lblPlan.Top := ModelPage.Edits[0].Top + ModelPage.Edits[0].Height + ScaleY(14);
  lblPlan.Font.Style := [fsBold];

  chkCopy := TNewCheckBox.Create(ModelPage);
  chkCopy.Parent := ModelPage.Surface;
  chkCopy.Width := ModelPage.SurfaceWidth;
  chkCopy.Top := lblPlan.Top + lblPlan.Height + ScaleY(6);
  chkCopy.Caption := 'Copy the selected model files into the install folder';
  chkCopy.Checked := False;

  { Attach handlers LAST: they touch lblHw/lblWarn/lblPlan, which must exist before any event can fire }
  rbCpu.OnClick := @ComputeClick;
  rbGpu.OnClick := @ComputeClick;
  ModelPage.Edits[0].OnChange := @ModelDirChange;
end;

procedure CurPageChanged(CurPageID: Integer);
begin
  WizardForm.NextButton.Enabled := True;
  if CurPageID = ComputePage.ID then UpdateComputePage
  else if CurPageID = ModelPage.ID then UpdateModelPage;
end;

function NextButtonClick(CurPageID: Integer): Boolean;
begin
  Result := True;
  if CurPageID = ComputePage.ID then Result := ComputeOk
  else if CurPageID = ModelPage.ID then Result := ModelOk;
end;

function InitializeSetup: Boolean;
var
  KV: TStringList;
  PrevVer: String;
begin
  Result := True;
  { upgrade detection: Inno upgrades in place (same AppId); we only log it. The uninstall key name is the AppId plus _is1 }
  if RegQueryStringValue(HKLM, 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{B7C0A3E4-5D2F-4A57-9F43-6D1E2F0C9A11}_is1', 'DisplayVersion', PrevVer) then
    Log('UPGRADE: previous version ' + PrevVer + ' found, installing {#AppVersion} over it')
  else
    Log('FRESH INSTALL of {#AppVersion}');
  KV := TStringList.Create;
  try
    if WizardSilent then
    begin
      if not RunProbe('hw', KV) then begin Result := False; Exit; end;
      if KVal(KV, 'ram_ok') <> '1' then begin Result := False; Exit; end;
      if (ExpandConstant('{param:COMPUTE|cpu}') = 'gpu') and (KVal(KV, 'gpu_ok') <> '1') then Result := False;
    end;
  finally
    KV.Free;
  end;
#ifndef BundleWebView2
  if Result and NeedsWebView2 then
  begin
    MsgBox('Microsoft Edge WebView2 Runtime is required but was not found on this computer.' + #13#10 +
           'Use the installer that is bundled with WebView2 (file name ends in -with-WebView2).', mbError, MB_OK);
    Result := False;
  end;
#endif
end;

{ ---------- after files are copied: write config, optionally copy model ---------- }
procedure CurStepChanged(CurStep: TSetupStep);
var
  Json: TArrayOfString;
  ModelDir, Src, Dst, Esc: String;
  Rc: Integer;
  Files: array of String;
  I: Integer;
begin
  if CurStep <> ssPostInstall then Exit;
  ModelDir := RemoveBackslashUnlessRoot(Trim(ModelPage.Values[0]));

  if chkCopy.Checked and RunProbe('plan --dir ' + QuoteArg(ModelDir) + ' --compute ' + ComputeName, Plan) and (KVal(Plan, 'status') = 'ok') then
  begin
    Dst := ExpandConstant('{app}\models');
    ForceDirectories(Dst);
    SetArrayLength(Files, 2);
    Files[0] := KVal(Plan, 'model_path');
    Files[1] := KVal(Plan, 'mmproj_path');
    for I := 0 to 1 do
    begin
      Src := Files[I];
      if Src = '' then Continue;
      WizardForm.StatusLabel.Caption := 'Copying ' + ExtractFileName(Src) + ' (this can take several minutes)...';
      WizardForm.StatusLabel.Update;
      if not Exec(ExpandConstant('{cmd}'), '/C copy /Y ' + QuoteArg(Src) + ' ' + QuoteArg(Dst + '\'), '', SW_HIDE, ewWaitUntilTerminated, Rc) or (Rc <> 0) then
      begin
        MsgBox('Copying ' + ExtractFileName(Src) + ' failed (not enough disk space?). The app will use the files in place instead.', mbError, MB_OK);
        Dst := '';
        Break;
      end;
    end;
    if Dst <> '' then ModelDir := Dst;
  end;

  { install.json seeds the app's config on first launch (the app then keeps its own copy in %APPDATA%) }
  Esc := ModelDir;
  StringChangeEx(Esc, '\', '\\', True);
  SetArrayLength(Json, 1);
  Json[0] := '{ "compute": "' + ComputeName + '", "model_dir": "' + Esc + '" }';
  SaveStringsToUTF8File(ExpandConstant('{app}\install.json'), Json, False);
end;

procedure DeinitializeSetup;
begin
  if Hw <> nil then Hw.Free;
  if Plan <> nil then Plan.Free;
end;
