; Stintmate AMS2 Connector installer (PRY-057 stage P2). Built by scripts/build-installer.ps1:
;   ISCC /DVersion=<x.y.z> /DVersionInfo=<a.b.c.d> /DStage=<dir> /DOutDir=<dir> [/DOutName=<file>]
;        [/DDev] [/DSetupIcon=<file.ico>]
; Per user (no administrator rights), fixed folder, English only, unsigned. Only code lives in
; {app}; the workspace and its data (<workspace>\connectors\ams2\) are never touched.
; The unattended mode is a contract for Stintmate: docs/installer.md.

#ifndef Version
  #error Version is not defined (build with scripts/build-installer.ps1)
#endif
#ifndef VersionInfo
  #define VersionInfo "0.0.0.0"
#endif
#ifndef Stage
  #error Stage is not defined (build with scripts/build-installer.ps1)
#endif
#ifndef OutDir
  #define OutDir "."
#endif

; A development build has its own identity (AppId, name, folder), so installing it on a test
; folder or machine never replaces or removes the real connector's installation.
#ifdef Dev
  #define AppName "Stintmate AMS2 Connector (dev)"
  #define AppGuid "{{7C1D5E94-3B2A-4F60-A8D7-2E9B0F4C6A15}"
  #define AppDir "Stintmate AMS2 Connector (dev)"
  #ifndef OutName
    #define OutName "AMS2-Connector-" + Version + "-dev-setup"
  #endif
#else
  #define AppName "Stintmate AMS2 Connector"
  #define AppGuid "{{E3A94F7B-5C18-4D2E-9B60-81F5D3C7A204}"
  #define AppDir "Stintmate AMS2 Connector"
  #ifndef OutName
    #define OutName "AMS2-Connector-" + Version + "-setup"
  #endif
#endif

[Setup]
AppId={#AppGuid}
AppName={#AppName}
AppVersion={#Version}
AppVerName={#AppName} {#Version}
AppPublisher=Stintmate
DefaultDirName={localappdata}\Programs\{#AppDir}
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableReadyPage=yes
DefaultGroupName={#AppName}
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutDir}
OutputBaseFilename={#OutName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
#ifdef SetupIcon
SetupIconFile={#SetupIcon}
#endif
UninstallDisplayIcon={app}\ams2-connector-ui.exe
UninstallDisplayName={#AppName}
; The setup closes nothing by itself: the [Code] below does it (and never with /MANAGED).
CloseApplications=no
RestartApplications=no
VersionInfoVersion={#VersionInfo}
VersionInfoProductVersion={#VersionInfo}
VersionInfoProductName={#AppName}
VersionInfoCompany=Stintmate
VersionInfoDescription={#AppName} setup
VersionInfoCopyright=Copyright 2026 The Stintmate authors

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Files]
Source: "{#Stage}\ams2-connector-ui.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\ams2-connector-window.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\ams2-recorder.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\NOTICE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\THIRD-PARTY-NOTICES.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\README.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
; Only the uninstall entry: Stintmate starts and supervises the connector, and a shortcut that
; started it alone would run it unmanaged next to Stintmate's own instance.
Name: "{autoprograms}\Uninstall {#AppName}"; Filename: "{uninstallexe}"

[Code]
const
  UiName = 'ams2-connector-ui.exe';
  WindowName = 'ams2-connector-window.exe';
  RecorderName = 'ams2-recorder.exe';
  { exit statuses of ams2-connector-ui.exe }
  ExitWorkspaceError = 3;
  ExitManaged = 4;

var
  ProgressFile: String;   { /PROGRESS=<file>, '' when not asked }
  FailReason: String;     { the "reason" of the done line }
  Started: Boolean;       { the files started to be copied (or removed) }
  Finished: Boolean;      { the run completed }
  LastPercent: Integer;
  Warning: String;        { a note for the done line of a successful run }

{ ---- command line (ParamStr works in the setup and in the uninstaller) ---- }

function CmdValue(const Name: String): String;
var
  I: Integer;
  Prefix, P: String;
begin
  Result := '';
  Prefix := '/' + Uppercase(Name) + '=';
  for I := 1 to ParamCount do
  begin
    P := ParamStr(I);
    if Pos(Prefix, Uppercase(P)) = 1 then
    begin
      Result := Trim(Copy(P, Length(Prefix) + 1, MaxInt));
      Exit;
    end;
  end;
end;

function CmdFlag(const Name: String): Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to ParamCount do
    if CompareText(ParamStr(I), '/' + Name) = 0 then
    begin
      Result := True;
      Exit;
    end;
end;

{ A run without a person: the setup's /SILENT or /VERYSILENT, or the uninstaller's. WizardSilent
  cannot be called while uninstalling. }
function IsSilent: Boolean;
begin
  if IsUninstaller then
    Result := UninstallSilent
  else
    Result := WizardSilent;
end;

{ ---- progress file: JSON lines, UTF-8, appended and closed per line (so flushed) ---- }

function JsonString(const S: String): String;
var
  T: String;
begin
  T := S;
  StringChangeEx(T, '\', '\\', True);
  StringChangeEx(T, '"', '\"', True);
  Result := '"' + T + '"';
end;

procedure Emit(const Line: String);
begin
  if ProgressFile <> '' then
    SaveStringToFile(ProgressFile, UTF8Encode(Line + #10), True);
end;

procedure EmitFiles(const Done: Integer);
begin
  Emit('{"stage":"files","done":' + IntToStr(Done) + ',"total":100}');
end;

procedure InitProgress;
begin
  ProgressFile := CmdValue('PROGRESS');
  if ProgressFile <> '' then
    SaveStringToFile(ProgressFile, '', False);   { truncate: one run per file }
  LastPercent := -1;
end;

procedure EmitDone;
var
  Reason: String;
begin
  if Finished then
  begin
    if Warning <> '' then
      Emit('{"stage":"done","ok":true,"warning":' + JsonString(Warning) + '}')
    else
      Emit('{"stage":"done","ok":true}');
    Exit;
  end;
  Reason := FailReason;
  if Reason = '' then
    if Started then
      Reason := 'disk'          { the files could not be written (or the run was cut short) }
    else
      Reason := 'cancelled';
  Emit('{"stage":"done","ok":false,"reason":' + JsonString(Reason) + '}');
end;

{ ---- Stintmate (the hub) and the connector's processes: patterns of Stintmate's own setup ---- }

function HubLauncher: String;
begin
  Result := ExpandConstant('{localappdata}\Programs\Stintmate\hub\.venv\Scripts\stintmate.exe');
end;

{ "stintmate.exe --running" of the installed hub (read-only). Exit 10: nothing of Stintmate
  runs. 11: it runs and is idle. 12: a driving session is recorded. Any other result (no
  launcher, an older hub): -1. }
function RunningState: Integer;
var
  Code: Integer;
begin
  Result := -1;
  if not FileExists(HubLauncher) then
    Exit;
  if Exec(HubLauncher, '--running', '', SW_HIDE, ewWaitUntilTerminated, Code) then
    if (Code >= 10) and (Code <= 12) then
      Result := Code;
end;

{ Asks before closing Stintmate. A silent run closes an idle Stintmate without asking and
  gives up on a busy one (a session is being recorded). }
function ConfirmClose(const What: String): Boolean;
var
  Code: Integer;
begin
  Result := True;
  Code := RunningState;
  while Code = 12 do
  begin
    if IsSilent or (MsgBox('A driving session is being recorded. Finish it, then choose Retry.', mbError, MB_RETRYCANCEL) <> IDRETRY) then
    begin
      Result := False;
      Exit;
    end;
    Code := RunningState;
  end;
  if (Code = 11) or ((Code = -1) and CheckForMutexes('Local\Stintmate.Hub')) then
    if not IsSilent then
      Result := MsgBox('Stintmate is open. The setup will close it to ' + What + '.', mbConfirmation, MB_OKCANCEL) = IDOK;
end;

{ "stintmate.exe --quit-all" of the installed hub. Exit 0: everything closed. 1: something is
  still running. Anything else, or no launcher: nothing runs, go on. }
function QuitAllDone: Boolean;
var
  Code: Integer;
begin
  Result := True;
  if not FileExists(HubLauncher) then
    Exit;
  if Exec(HubLauncher, '--quit-all', '', SW_HIDE, ewWaitUntilTerminated, Code) then
    Result := Code <> 1;
end;

function QuitAllOrAsk(const What: String): Boolean;
begin
  Result := ConfirmClose(What);
  if not Result then
    Exit;
  Result := QuitAllDone;
  while not Result do
  begin
    if IsSilent or (MsgBox('Stintmate is still running. Quit it from its tray icon and end any radio call, then choose Retry to ' + What + '.', mbError, MB_RETRYCANCEL) <> IDRETRY) then
      Exit;
    Result := QuitAllDone;
  end;
end;

{ How many of the connector's three programs run from the install folder (by image path, so the Python
  connector and any other copy are never counted). If WMI is unavailable, the connector's
  single-instance mutexes answer instead. }
function CountRunning: Integer;
var
  Locator, Service, Items, Item: Variant;
  I: Integer;
  Dir, Path: String;
begin
  Result := 0;
  Dir := Lowercase(AddBackslash(ExpandConstant('{app}')));
  try
    Locator := CreateOleObject('WbemScripting.SWbemLocator');
    Service := Locator.ConnectServer('.', 'root\cimv2');
    Items := Service.ExecQuery('SELECT ExecutablePath FROM Win32_Process WHERE Name = ''' + UiName + ''' OR Name = ''' + WindowName + ''' OR Name = ''' + RecorderName + '''');
    for I := 0 to Items.Count - 1 do
    begin
      Item := Items.ItemIndex(I);
      if not VarIsNull(Item.ExecutablePath) then
      begin
        Path := Item.ExecutablePath;
        Path := Lowercase(Path);
        if Pos(Dir, Path) = 1 then
          Result := Result + 1;
      end;
    end;
  except
    if CheckForMutexes('Local\Ams2Connector.UI,Local\Ams2Connector.Window,Local\Ams2Connector.Recorder') then
      Result := 1;
  end;
end;

function WaitStopped(const Seconds: Integer): Boolean;
var
  I: Integer;
begin
  Result := CountRunning = 0;
  I := 0;
  while (not Result) and (I < Seconds * 4) do
  begin
    Sleep(250);
    I := I + 1;
    Result := CountRunning = 0;
  end;
end;

function WorkspaceArgs(const Workspace: String): String;
var
  W: String;
begin
  Result := '';
  W := Workspace;
  while (Length(W) > 3) and (W[Length(W)] = '\') do
    Delete(W, Length(W), 1);
  if W <> '' then
    Result := ' --workspace "' + W + '"';
end;

{ Leaves none of the connector's programs running, or returns the reason it could not.
  /MANAGED: Stintmate stopped the connector, so one still running is a failure and nothing is
  closed. Otherwise: ask the control app to exit (--quit); when Stintmate manages it (exit 4),
  close Stintmate (asking, unless the run is silent) and wait for it to stop the connector. }
function EnsureConnectorStopped(const Workspace: String): String;
var
  Code: Integer;
  Managed: Boolean;
  What: String;
begin
  Result := '';
  if CountRunning = 0 then
    Exit;
  if CmdFlag('MANAGED') then
  begin
    Result := 'connector_running';
    Exit;
  end;
  Managed := False;
  if FileExists(ExpandConstant('{app}\' + UiName)) then
    if Exec(ExpandConstant('{app}\' + UiName), '--quit' + WorkspaceArgs(Workspace), '', SW_HIDE, ewWaitUntilTerminated, Code) then
      Managed := Code = ExitManaged;
  if WaitStopped(3) then
    Exit;
  What := 'continue the installation';
  if IsUninstaller then
    What := 'remove the AMS2 connector';
  if not QuitAllOrAsk(What) then
  begin
    Result := 'connector_running';
    Exit;
  end;
  if not WaitStopped(15) then
    Result := 'connector_running';
end;

function ConnectorRunningText: String;
begin
  Result := 'The AMS2 connector is running. Close it (and Stintmate, which manages it) and run the setup again.';
end;

{ ---- the workspace the connector is registered in, remembered for the uninstaller ---- }

const
  InstallsKey = 'Software\Stintmate AMS2 Connector\Installs';   { value name: the install folder }

function RememberedWorkspace: String;
begin
  if not RegQueryStringValue(HKCU, InstallsKey, ExpandConstant('{app}'), Result) then
    Result := '';
end;

procedure RememberWorkspace(const Workspace: String);
begin
  RegWriteStringValue(HKCU, InstallsKey, ExpandConstant('{app}'), Workspace);
end;

procedure ForgetWorkspace;
begin
  RegDeleteValue(HKCU, InstallsKey, ExpandConstant('{app}'));
  RegDeleteKeyIfEmpty(HKCU, InstallsKey);
  RegDeleteKeyIfEmpty(HKCU, 'Software\Stintmate AMS2 Connector');
end;

{ Unattended runs need a workspace before anything is copied: /WORKSPACE, STINTMATE_WORKSPACE
  or the pointer file the Stintmate onboarding writes. Whether it is a usable workspace is
  decided by --register afterwards. }
function WorkspaceKnown: Boolean;
var
  W: String;
begin
  W := CmdValue('WORKSPACE');
  if W <> '' then
    Result := DirExists(W)
  else
    Result := (GetEnv('STINTMATE_WORKSPACE') <> '') or FileExists(ExpandConstant('{userappdata}\Stintmate\settings.json'));
end;

{ ---- install ---- }

function InitializeSetup: Boolean;
begin
  InitProgress;
  Result := True;
end;

procedure DeinitializeSetup;
begin
  EmitDone;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  Emit('{"stage":"prepare"}');
  if WizardSilent and not WorkspaceKnown then
  begin
    FailReason := 'no_workspace';
    Result := 'No Stintmate workspace was found (/WORKSPACE, or the one Stintmate saved).';
    Exit;
  end;
  FailReason := EnsureConnectorStopped(CmdValue('WORKSPACE'));
  if FailReason <> '' then
    Result := ConnectorRunningText;
end;

procedure CurInstallProgressChanged(CurProgress, MaxProgress: Integer);
var
  Percent: Integer;
begin
  if (MaxProgress <= 0) or (ProgressFile = '') then
    Exit;
  Percent := (CurProgress * 100) div MaxProgress;
  if Percent > 99 then
    Percent := 99;     { 100 is written once the files are all in place }
  if Percent <> LastPercent then
  begin
    LastPercent := Percent;
    EmitFiles(Percent);
  end;
end;

{ Registers the connector with Stintmate: ams2-connector-ui.exe --register [--workspace]. }
procedure RegisterConnector;
var
  Code: Integer;
  Workspace: String;
begin
  Emit('{"stage":"register"}');
  Workspace := CmdValue('WORKSPACE');
  if not Exec(ExpandConstant('{app}\' + UiName), '--register' + WorkspaceArgs(Workspace), '', SW_HIDE, ewWaitUntilTerminated, Code) then
    Code := 1;
  if Code = 0 then
  begin
    if Workspace <> '' then
      RememberWorkspace(Workspace);
    Exit;
  end;
  if (Code = ExitWorkspaceError) and not WizardSilent then
  begin
    { interactive, no workspace yet: the files stay and the control app registers itself }
    MsgBox('The AMS2 connector is installed.' + #13#10#13#10 +
      'No Stintmate workspace was found yet. Open Stintmate and finish its first run; the connector registers itself when it starts.', mbInformation, MB_OK);
    Exit;
  end;
  if Code = ExitWorkspaceError then
    FailReason := 'no_workspace'
  else
    FailReason := 'register_failed';
  if not WizardSilent then
    MsgBox('The AMS2 connector could not be registered with Stintmate (code ' + IntToStr(Code) + '). Nothing was installed. See the log in the workspace folder connectors\ams2, or run the setup again.', mbCriticalError, MB_OK);
  { Abort during the installation rolls it back: no half-installed connector is left behind }
  Abort;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssInstall then
  begin
    Started := True;
    EmitFiles(0);
  end
  else if CurStep = ssPostInstall then
  begin
    EmitFiles(100);
    RegisterConnector;
  end
  else if CurStep = ssDone then
    Finished := True;
end;

{ ---- uninstall ---- }

function InitializeUninstall: Boolean;
begin
  InitProgress;
  Emit('{"stage":"prepare"}');
  FailReason := EnsureConnectorStopped(RememberedWorkspace);
  Result := FailReason = '';
  if not Result and not UninstallSilent then
    MsgBox('The AMS2 connector is still running. Close it (and Stintmate, which manages it); nothing was uninstalled.', mbInformation, MB_OK);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Code: Integer;
begin
  if CurUninstallStep = usUninstall then
  begin
    Started := True;
    Emit('{"stage":"unregister"}');
    { removes <workspace>\connectors\ams2\connector.json only; exit 3 (no workspace any more)
      has nothing to remove }
    if not Exec(ExpandConstant('{app}\' + UiName), '--unregister' + WorkspaceArgs(RememberedWorkspace), '', SW_HIDE, ewWaitUntilTerminated, Code) then
      Code := 1;
    if (Code <> 0) and (Code <> ExitWorkspaceError) then
      Warning := 'unregister_failed';
    EmitFiles(0);
  end
  else if CurUninstallStep = usPostUninstall then
  begin
    ForgetWorkspace;
    EmitFiles(100);
  end
  else if CurUninstallStep = usDone then
    Finished := True;
end;

procedure DeinitializeUninstall;
begin
  EmitDone;
end;
