; MedGemma Offline - Inno Setup 6.3+ script. Built in CI (see .github/workflows/build.yml).
; Build flags:  /DAppVersion=1.0.0   /DBundleWebView2   (ships the offline WebView2 runtime installer)
;               /DSign + /Smgsign=<command>   (Authenticode-signs the installer AND the uninstaller; used in CI)
; Silent install:  setup.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART [/DIR="..."] [/TASKS=desktopicon]
;                  [/COMPUTE=cpu|gpu] [/MODELDIR="D:\models"] [/COPYMODEL=1]
;   - /COMPUTE and /MODELDIR left out on a RE-RUN keep the previous installation's choices (install.json), they are never reset.
;   - /COMPUTE=gpu without a usable GPU, an invalid value, or a /MODELDIR folder that does not exist: setup refuses (exit code 1) and
;     changes nothing. The reason is written to the setup log (/LOG=...).
#ifndef AppVersion
  #define AppVersion "1.0.0"
#endif

[Setup]
AppId={{B7C0A3E4-5D2F-4A57-9F43-6D1E2F0C9A11}
AppName=MedGemma Offline
AppVersion={#AppVersion}
AppPublisher=Nouman
AppVerName=MedGemma Offline {#AppVersion}
AppCopyright=Copyright (C) Nouman
AppComments=Fully offline medical AI assistant. No data leaves this computer.
UninstallDisplayName=MedGemma Offline
DefaultGroupName=MedGemma Offline
UsePreviousAppDir=yes
UsePreviousTasks=yes
; only one setup at a time (a second double-click must not start a parallel install)
SetupMutex=MedGemmaOfflineSetupMutex
; close a running instance (and its llama-server) before upgrading instead of leaving locked files behind
CloseApplications=yes
RestartApplications=no
; Program Files: its default permissions already keep standard users from modifying the program files.
; (An existing installation keeps its folder: UsePreviousAppDir. The script also locks the folder down explicitly, see LockDownAppDir.)
DefaultDirName={autopf}\MedGemma Offline
DisableProgramGroupPage=yes
DisableDirPage=no
PrivilegesRequired=admin
MinVersion=10.0
ShowLanguageDialog=no
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
#ifdef Sign
; the command behind 'mgsign' is passed on the ISCC command line (/Smgsign=...) by the CI workflow
SignTool=mgsign
SignedUninstaller=yes
; a timestamp server that hiccups must not fail the whole build: sign.ps1 already rotates servers, this retries the call itself
SignToolRetryCount=3
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
; The engine's outbound-network firewall rule is created in [Code] (BlockEngineNetwork) so that its result is written to the setup log.
#ifdef BundleWebView2
Filename: "{tmp}\MicrosoftEdgeWebView2RuntimeInstallerX64.exe"; Parameters: "/silent /install"; StatusMsg: "Installing Microsoft WebView2 runtime (offline)..."; Check: NeedsWebView2; Flags: waituntilterminated
#endif
Filename: "{app}\medgemma-offline.exe"; Description: "Launch MedGemma Offline"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; removes the firewall rule(s) created at install time (by name, so it is found wherever the program was installed)
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""MedGemma Offline engine (no network)"""; Flags: runhidden; RunOnceId: "RemoveEngineFirewallRule"

[UninstallDelete]
; files created AFTER installation (so Inno does not know them). Your model folder is never touched.
; (CurUninstallStepChanged additionally removes the per-user data of every Windows profile on this PC.)
Type: filesandordirs; Name: "{app}\backend"
Type: filesandordirs; Name: "{app}\tools"
Type: filesandordirs; Name: "{app}\models"
Type: files; Name: "{app}\install.json"
Type: filesandordirs; Name: "{userappdata}\com.nouman.medgemma.offline"
Type: filesandordirs; Name: "{localappdata}\com.nouman.medgemma.offline"
Type: dirifempty; Name: "{app}"

[Code]
const
  MIN_RAM_TEXT = '4 GB';
  UNINSTALL_KEY = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{B7C0A3E4-5D2F-4A57-9F43-6D1E2F0C9A11}_is1';
  FW_RULE = 'MedGemma Offline engine (no network)';
  APP_DATA_NAME = 'com.nouman.medgemma.offline';

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
  AppDirIsOurs: Boolean;
  // what the previous installation (if any) chose, read from its install.json
  PrevAppDir, PrevCompute, PrevModelDir: String;
  // effective starting choices: command line first, then the previous installation, then the defaults
  EffCompute, EffModelDir: String;
  // files to copy into the install folder (decided and size-checked in PrepareToInstall)
  CopyModelSrc, CopyMmprojSrc: String;

// ---------- helpers ----------
// Inno's TStringList has no Values[] property: look up KEY=VALUE lines ourselves
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
  // a trailing backslash would escape the closing quote -> double it
  if (Length(S) > 0) and (S[Length(S)] = '\') then S := S + '\';
  Result := '"' + S + '"';
end;

// A path typed or pasted into the folder box: trims blanks, drops surrounding quotes and a trailing backslash (but keeps "D:\").
function CleanPath(S: String): String;
begin
  S := Trim(S);
  if (Length(S) >= 2) and (S[1] = '"') and (S[Length(S)] = '"') then
    S := Trim(Copy(S, 2, Length(S) - 2));
  if S = '' then Result := '' else Result := RemoveBackslashUnlessRoot(S);
end;

// Minimal reader for "key": "value" in the one-line install.json that this script writes (handles \\ and \" escapes).
function JsonStr(const J, Key: String): String;
var
  P, Q: Integer;
  S: String;
begin
  Result := '';
  P := Pos('"' + Key + '"', J);
  if P = 0 then Exit;
  S := Copy(J, P + Length(Key) + 2, Length(J));
  P := Pos(':', S);
  if P = 0 then Exit;
  S := Copy(S, P + 1, Length(S));
  P := Pos('"', S);
  if P = 0 then Exit;
  S := Copy(S, P + 1, Length(S));
  Q := 1;
  while Q <= Length(S) do
  begin
    if S[Q] = '\' then
    begin
      if Q < Length(S) then Result := Result + S[Q + 1];
      Q := Q + 2;
    end
    else if S[Q] = '"' then
      Exit
    else
    begin
      Result := Result + S[Q];
      Q := Q + 1;
    end;
  end;
end;

function DirIsEmpty(const D: String): Boolean;
var
  FR: TFindRec;
begin
  Result := True;
  if FindFirst(AddBackslash(D) + '*', FR) then
  begin
    try
      repeat
        if (FR.Name <> '.') and (FR.Name <> '..') then
        begin
          Result := False;
          Break;
        end;
      until not FindNext(FR);
    finally
      FindClose(FR);
    end;
  end;
end;

// True when Child lies inside Parent
function IsInside(const Child, Parent: String): Boolean;
begin
  Result := (Parent <> '') and (CompareText(Copy(Child, 1, Length(AddBackslash(Parent))), AddBackslash(Parent)) = 0);
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
    KV.LoadFromFile(Outf);   // mg-probe writes UTF-8 with a BOM, so non-ASCII folder names survive
    Result := True;
  end;
end;

// Runs a program hidden and waits. Returns its exit code, or -1 when it could not be started at all.
function RunHidden(const Exe, Params: String): Integer;
var
  Rc: Integer;
begin
  Rc := -1;
  if Exec(Exe, Params, '', SW_HIDE, ewWaitUntilTerminated, Rc) then Result := Rc else Result := -1;
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

// Ends a running MedGemma (its engine is a child of it and dies with it). Used before relocating or uninstalling.
procedure StopRunningApp;
begin
  RunHidden(ExpandConstant('{sys}\taskkill.exe'), '/F /T /IM medgemma-offline.exe');
end;

// ---------- a previous installation: its choices are the defaults of a re-run ----------
procedure ReadPreviousInstall;
var
  L: TStringList;
  P, J: String;
begin
  PrevAppDir := '';
  PrevCompute := '';
  PrevModelDir := '';
  if not RegQueryStringValue(HKLM, UNINSTALL_KEY, 'Inno Setup: App Path', PrevAppDir) then Exit;
  P := AddBackslash(PrevAppDir) + 'install.json';
  if not FileExists(P) then Exit;
  L := TStringList.Create;
  try
    try
      L.LoadFromFile(P);
      J := L.Text;
      PrevCompute := Lowercase(JsonStr(J, 'compute'));
      PrevModelDir := JsonStr(J, 'model_dir');
    except
      Log('previous install.json could not be read: ' + GetExceptionMessage);
    end;
  finally
    L.Free;
  end;
  Log('PREVIOUS INSTALL: folder=' + PrevAppDir + ' compute=' + PrevCompute + ' model_dir=' + PrevModelDir);
end;

// ---------- page 1: CPU / GPU with LIVE hardware detection ----------
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
    // hard rule: < 4 GB installed RAM -> block, red text on this same page
    rbCpu.Enabled := False;
    rbGpu.Enabled := False;
    lblWarn.Caption := 'This computer has only ' + KVal(Hw, 'total_gb') + ' GB of installed RAM. A minimum of ' + MIN_RAM_TEXT + ' is required, so MedGemma Offline cannot be installed.';
    WizardForm.NextButton.Enabled := False;
    Exit;
  end;

  if rbGpu.Checked and (KVal(Hw, 'gpu_ok') <> '1') then
  begin
    // red text on the same dialog, no message box; one line so that the setup log keeps it on one line
    lblWarn.Caption := 'GPU is not available currently in this system. Choose CPU, or install a Vulkan-capable graphics driver (a GPU with at least 2 GB of video memory) and run setup again.';
    WizardForm.NextButton.Enabled := False;       // Next stays disabled until an available option is chosen
  end
  else
  begin
    lblWarn.Caption := '';
    ComputeOk := True;
    WizardForm.NextButton.Enabled := True;
  end;
end;

// Writes the wizard's decision to the Setup log (/LOG) so CI can assert on the REAL wizard's behaviour
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

// ---------- page 2: model folder + automatic variant choice ----------
procedure UpdateModelPageCore;
var
  D: String;
begin
  ModelOk := False;
  lblPlan.Font.Color := clRed;
  D := CleanPath(ModelPage.Values[0]);
  if (D = '') or (not DirExists(D)) then
  begin
    if D = '' then
      lblPlan.Caption := 'Select the folder (hard disk or USB) that contains the MedGemma model files.'
    else
      lblPlan.Caption := 'This folder was not found: ' + D + #13#10 + 'Connect the drive or select another folder.';
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

// ---------- wizard construction ----------
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
  rbCpu.Checked := EffCompute <> 'gpu';
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
  lblWarn.Height := ScaleY(64);
  lblWarn.Top := lblHw.Top + lblHw.Height + ScaleY(8);
  lblWarn.Font.Color := clRed;
  lblWarn.Font.Style := [fsBold];

  ModelPage := CreateInputDirPage(ComputePage.ID, 'MedGemma model files',
    'Where are the MedGemma model files?',
    'Select the folder (hard disk or USB drive) that contains the MedGemma .gguf files. Setup picks the largest variant that runs at a usable speed on this PC: 27B multimodal (8-bit first), and 4B only if 27B would be too slow. Files stay where they are unless you tick the box below.',
    False, '');
  ModelPage.Add('');
  ModelPage.Values[0] := EffModelDir;

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
  chkCopy.Checked := ExpandConstant('{param:COPYMODEL|0}') = '1';

  // Attach handlers LAST: they touch lblHw/lblWarn/lblPlan, which must exist before any event can fire
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
  if CurPageID = ComputePage.ID then
    Result := ComputeOk
  else if CurPageID = ModelPage.ID then
  begin
    // analyse again NOW: the USB drive may have been removed, or the files changed, since the folder was typed or browsed
    UpdateModelPage;
    Result := ModelOk;
  end;
end;

// ---------- start of setup: validates the command line of a silent install, picks the starting choices ----------
function InitializeSetup: Boolean;
var
  KV: TStringList;
  PrevVer, ParamCompute, ParamModelDir: String;
begin
  Result := True;
  // upgrade detection: Inno upgrades in place (same AppId); we log it. The uninstall key name is the AppId plus _is1
  if RegQueryStringValue(HKLM, UNINSTALL_KEY, 'DisplayVersion', PrevVer) then
    Log('UPGRADE: previous version ' + PrevVer + ' found, installing {#AppVersion} over it')
  else
    Log('FRESH INSTALL of {#AppVersion}');
  ReadPreviousInstall;

  ParamCompute := Lowercase(Trim(ExpandConstant('{param:COMPUTE|}')));
  ParamModelDir := CleanPath(ExpandConstant('{param:MODELDIR|}'));
  if (ParamCompute <> '') and (ParamCompute <> 'cpu') and (ParamCompute <> 'gpu') then
  begin
    Log('REFUSED: /COMPUTE must be cpu or gpu (got "' + ParamCompute + '"). Nothing was changed.');
    if not WizardSilent then
      MsgBox('The /COMPUTE option must be cpu or gpu.', mbError, MB_OK);
    Result := False;
    Exit;
  end;

  // starting choices: command line, else the previous installation (a re-run must not silently reset them), else CPU / nothing
  if ParamCompute <> '' then EffCompute := ParamCompute
  else if PrevCompute = 'gpu' then EffCompute := 'gpu'
  else EffCompute := 'cpu';
  if ParamModelDir <> '' then EffModelDir := ParamModelDir else EffModelDir := PrevModelDir;

  KV := TStringList.Create;
  try
    if WizardSilent then
    begin
      if not RunProbe('hw', KV) then
      begin
        Log('REFUSED: the hardware probe failed. Nothing was changed.');
        Result := False;
        Exit;
      end;
      if KVal(KV, 'ram_ok') <> '1' then
      begin
        Log('REFUSED: this computer has less than ' + MIN_RAM_TEXT + ' of RAM. Nothing was changed.');
        Result := False;
        Exit;
      end;
      if (EffCompute = 'gpu') and (KVal(KV, 'gpu_ok') <> '1') then
      begin
        if ParamCompute = 'gpu' then
        begin
          Log('REFUSED: /COMPUTE=gpu but no usable GPU (Vulkan, at least 2 GB VRAM) was detected. Nothing was changed.');
          Result := False;
          Exit;
        end;
        // only inherited from the previous installation: do not fail an upgrade, fall back to CPU
        Log('NOTE: the previous installation used the GPU but none is usable now: continuing with CPU.');
        EffCompute := 'cpu';
      end;
      if (ParamModelDir <> '') and (not DirExists(ParamModelDir)) then
      begin
        Log('REFUSED: /MODELDIR folder not found: ' + ParamModelDir + '. Nothing was changed.');
        Result := False;
        Exit;
      end;
    end;
  finally
    KV.Free;
  end;
#ifndef BundleWebView2
  if Result and NeedsWebView2 then
  begin
    Log('REFUSED: Microsoft Edge WebView2 Runtime not found (use the -with-WebView2 installer).');
    MsgBox('Microsoft Edge WebView2 Runtime is required but was not found on this computer.' + #13#10 +
           'Use the installer that is bundled with WebView2 (file name ends in -with-WebView2).', mbError, MB_OK);
    Result := False;
  end;
#endif
end;

// ---------- install-folder permissions ----------
// A folder created directly under a drive root inherits "Authenticated Users: Modify", so after an admin install ANY standard user could replace
// medgemma-offline.exe, llama-server.exe or its DLLs, or edit install.json, and have that code run under another user's account (local
// privilege escalation). The permissions of OUR folder are therefore reset to SYSTEM + Administrators full control and Users read/execute.
// A pre-existing foreign folder (for example D:\Tools) is never re-permissioned: only a folder that this installer creates (or that is empty),
// or that already holds a previous install of this app, is locked down.
procedure LockDownAppDir;
var
  Dir: String;
  Rc: Integer;
begin
  Dir := RemoveBackslashUnlessRoot(ExpandConstant('{app}'));
  if (Length(Dir) <= 3) or (not AppDirIsOurs) then
  begin
    Log('SECURITY: permissions of ' + Dir + ' were left unchanged (drive root, or a folder that already held other files)');
    if not WizardSilent then
      MsgBox('The install folder already contained other files, so its permissions were not changed. For best security install into a new folder, ' +
             'for example under C:\Program Files, so that only administrators can modify the program files.', mbInformation, MB_OK);
    Exit;
  end;
  Rc := RunHidden(ExpandConstant('{sys}\icacls.exe'),
          QuoteArg(Dir) + ' /inheritance:r /grant:r *S-1-5-18:(OI)(CI)F *S-1-5-32-544:(OI)(CI)F *S-1-5-32-545:(OI)(CI)RX /T /C /Q');
  if Rc = 0 then
    Log('SECURITY: install folder locked down (SYSTEM + Administrators full control, Users read/execute)')
  else
    Log('SECURITY WARNING: could not restrict permissions on ' + Dir + ' (icacls exit code ' + IntToStr(Rc) + ')');
end;

// ---------- "offline" enforced by the OS: the inference engine gets no network access ----------
// Windows Firewall does not filter loopback traffic, so the app still reaches its own engine on 127.0.0.1. Removed again by [UninstallRun].
procedure BlockEngineNetwork;
var
  Netsh, Base, Exe: String;
  Rc: Integer;
  Failed: Boolean;
begin
  Netsh := ExpandConstant('{sys}\netsh.exe');
  Base := 'advfirewall firewall add rule name="' + FW_RULE + '" dir=out action=block enable=yes profile=any ' +
          'description="Blocks all network access of the MedGemma inference engine. Removed when MedGemma Offline is uninstalled." program=';
  Failed := False;
  RunHidden(Netsh, 'advfirewall firewall delete rule name="' + FW_RULE + '"');   // a re-run / upgrade starts clean (no duplicate rules)

  Exe := ExpandConstant('{app}\backend\cpu\llama-server.exe');
  Rc := RunHidden(Netsh, Base + QuoteArg(Exe));
  Log('FIREWALL: outbound block for ' + Exe + ' -> netsh exit code ' + IntToStr(Rc));
  if Rc <> 0 then Failed := True;

  Exe := ExpandConstant('{app}\backend\vulkan\llama-server.exe');
  if FileExists(Exe) then
  begin
    Rc := RunHidden(Netsh, Base + QuoteArg(Exe));
    Log('FIREWALL: outbound block for ' + Exe + ' -> netsh exit code ' + IntToStr(Rc));
    if Rc <> 0 then Failed := True;
  end;

  if Failed and (not WizardSilent) then
    MsgBox('The Windows Firewall rule that blocks network access for the AI engine could not be created (is the firewall managed by another product?). ' +
           'MedGemma Offline works without any network connection; you may block llama-server.exe in your firewall yourself.', mbInformation, MB_OK);
end;

// Installing into a DIFFERENT folder than before would leave the old copy behind, unregistered and never removed (same AppId = one Apps & features
// entry). Remove the old installation first; it is left alone if the chosen model folder lives inside it.
procedure RemovePreviousInstallElsewhere(const ModelDirNow: String);
var
  Prev, Cur, Unins: String;
  I: Integer;
  Rc: Integer;
begin
  if PrevAppDir = '' then Exit;
  Prev := RemoveBackslashUnlessRoot(PrevAppDir);
  Cur := RemoveBackslashUnlessRoot(WizardDirValue);
  if CompareText(Prev, Cur) = 0 then Exit;
  Unins := AddBackslash(Prev) + 'unins000.exe';
  if not FileExists(Unins) then Exit;
  if IsInside(ModelDirNow, Prev) then
  begin
    Log('RELOCATION: the model folder is inside the old installation, so the old installation at ' + Prev + ' is left in place');
    Exit;
  end;
  Log('RELOCATION: removing the previous installation at ' + Prev);
  StopRunningApp;
  Rc := RunHidden(Unins, '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART');
  Log('RELOCATION: previous uninstaller exit code ' + IntToStr(Rc));
  // the uninstaller continues from a temporary copy of itself: wait (max 60 s) until the old uninstaller file is gone
  for I := 1 to 120 do
  begin
    if not FileExists(Unins) then Break;
    Sleep(500);
  end;
end;

// ---------- the last checks before anything is changed ----------
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  D, ModelDirNow: String;
  FreeMB, TotalMB: Cardinal;
  FreeI, NeedMB: Integer;
begin
  Result := '';
  D := AddBackslash(WizardDirValue);
  AppDirIsOurs := (not DirExists(D)) or FileExists(D + 'medgemma-offline.exe') or DirIsEmpty(D);
  ModelDirNow := CleanPath(ModelPage.Values[0]);

  CopyModelSrc := '';
  CopyMmprojSrc := '';
  if chkCopy.Checked and (ModelDirNow <> '') then
  begin
    if (not RunProbe('plan --dir ' + QuoteArg(ModelDirNow) + ' --compute ' + ComputeName, Plan)) or (KVal(Plan, 'status') <> 'ok') then
    begin
      Result := 'The model files could not be analysed, so they cannot be copied into the install folder. Untick the copy option or select another folder.';
      Exit;
    end;
    CopyModelSrc := KVal(Plan, 'model_path');
    CopyMmprojSrc := KVal(Plan, 'mmproj_path');
    // enough room for the copy (plus 1 GB for the program itself)? A half-written 20 GB file must never be left behind.
    NeedMB := StrToIntDef(KVal(Plan, 'model_mb'), 0) + StrToIntDef(KVal(Plan, 'mmproj_mb'), 0) + 1024;
    if GetSpaceOnDisk(ExtractFileDrive(D) + '\', True, FreeMB, TotalMB) then
    begin
      FreeI := Integer(FreeMB);
      if FreeI < NeedMB then
      begin
        Result := 'Not enough free disk space on ' + ExtractFileDrive(D) + ' to copy the model files: ' + IntToStr(NeedMB div 1024) + ' GB are needed, ' +
                  IntToStr(FreeI div 1024) + ' GB are free. Untick "Copy the selected model files" (the files are then used where they are) or free some space.';
        Exit;
      end;
    end;
  end;

  // only now, when nothing can fail any more: remove an old installation in another folder
  RemovePreviousInstallElsewhere(ModelDirNow);
end;

// The "Ready to install" page lists the choices, so they can be checked before anything is changed.
function UpdateReadyMemo(Space, NewLine, MemoUserInfoInfo, MemoDirInfo, MemoTypeInfo, MemoComponentsInfo, MemoGroupInfo, MemoTasksInfo: String): String;
var
  S: String;
begin
  S := MemoDirInfo + NewLine + NewLine;
  S := S + 'Processor:' + NewLine + Space + Tern(GpuSelected, 'GPU (Vulkan) - the CPU engine is installed as well', 'CPU') + NewLine + NewLine;
  S := S + 'Model folder:' + NewLine + Space + CleanPath(ModelPage.Values[0]);
  if chkCopy.Checked then
    S := S + NewLine + Space + '(the selected model files are copied into the install folder)';
  if MemoTasksInfo <> '' then
    S := S + NewLine + NewLine + MemoTasksInfo;
  Result := S;
end;

// Copies one model file into the install folder. A failed or partial copy is deleted again (never leave a half-written multi-GB file).
function CopyOne(const Src, DstDir: String): Boolean;
var
  Target: String;
begin
  Target := AddBackslash(DstDir) + ExtractFileName(Src);
  if CompareText(Src, Target) = 0 then
  begin
    Result := True;     // the model already lives there (re-run): nothing to copy, and copying a file onto itself would destroy it
    Exit;
  end;
  WizardForm.StatusLabel.Caption := 'Copying ' + ExtractFileName(Src) + ' (this can take several minutes)...';
  WizardForm.StatusLabel.Update;
  Result := FileCopy(Src, Target, False);
  if not Result then
  begin
    Log('COPY FAILED: ' + Src + ' -> ' + Target);
    DeleteFile(Target);
  end;
end;

// ---------- after files are copied: write config, optionally copy model, firewall, permissions ----------
procedure CurStepChanged(CurStep: TSetupStep);
var
  Json: TArrayOfString;
  ModelDir, Dst, Esc: String;
  Copied: Boolean;
begin
  if CurStep <> ssPostInstall then Exit;
  ModelDir := CleanPath(ModelPage.Values[0]);

  if CopyModelSrc <> '' then
  begin
    Dst := ExpandConstant('{app}\models');
    ForceDirectories(Dst);
    Copied := CopyOne(CopyModelSrc, Dst);
    if Copied and (CopyMmprojSrc <> '') then Copied := CopyOne(CopyMmprojSrc, Dst);
    if Copied then
      ModelDir := Dst
    else
    begin
      DelTree(Dst, True, True, True);
      MsgBox('Copying the model files failed (not enough disk space, or the drive was removed?). The app will use the files in place instead.', mbError, MB_OK);
    end;
  end;

  // install.json seeds the app's config on first launch (the app then keeps its own copy in %APPDATA%; a later re-run of this setup replaces it)
  Esc := ModelDir;
  StringChangeEx(Esc, '\', '\\', True);
  SetArrayLength(Json, 1);
  Json[0] := '{ "compute": "' + ComputeName + '", "model_dir": "' + Esc + '" }';
  SaveStringsToUTF8File(ExpandConstant('{app}\install.json'), Json, False);
  Log('INSTALL: compute=' + ComputeName + ' model_dir=' + ModelDir);
  BlockEngineNetwork;
  LockDownAppDir;
end;

// ---------- uninstall ----------
// Per-user data lives under each profile; [UninstallDelete] only reaches the account that runs the uninstaller.
procedure RemoveUserDataOfAllProfiles;
var
  FR: TFindRec;
  Root, Name: String;
begin
  Root := ExpandConstant('{sd}\Users\');
  if FindFirst(Root + '*', FR) then
  begin
    try
      repeat
        Name := FR.Name;
        if ((FR.Attributes and FILE_ATTRIBUTE_DIRECTORY) <> 0) and (Name <> '.') and (Name <> '..') and
           (CompareText(Name, 'Public') <> 0) and (CompareText(Name, 'Default') <> 0) and
           (CompareText(Name, 'Default User') <> 0) and (CompareText(Name, 'All Users') <> 0) then
        begin
          DelTree(Root + Name + '\AppData\Roaming\' + APP_DATA_NAME, True, True, True);
          DelTree(Root + Name + '\AppData\Local\' + APP_DATA_NAME, True, True, True);
        end;
      until not FindNext(FR);
    finally
      FindClose(FR);
    end;
  end;
end;

function InitializeUninstall: Boolean;
begin
  Result := True;
  StopRunningApp;      // a running app would keep its exe and the engine DLLs locked and leave files behind
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then RemoveUserDataOfAllProfiles;
end;

procedure DeinitializeSetup;
begin
  if Hw <> nil then Hw.Free;
  if Plan <> nil then Plan.Free;
end;
