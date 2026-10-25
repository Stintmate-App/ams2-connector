; Stintmate AMS2 Connector installer (PRY-057 stage P2). Built by scripts/build-installer.ps1:
;   ISCC /DVersion=<x.y.z> /DVersionInfo=<a.b.c.d> /DStage=<dir> /DOutDir=<dir> [/DOutName=<file>]
;        [/DDev] [/DSetupIcon=<file.ico>]
; Per user (no administrator rights), fixed folder, English only, unsigned. Only code lives in
; {app}; the workspace and its data (<workspace>\connectors\ams2\) are never touched, and the
; setup needs no workspace: it registers the connector in the per-user machine file
; %LOCALAPPDATA%\Stintmate\connectors\ams2.json (PRY-088). One flavour (release or dev) is
; installed at a time: a setup removes the other one through the "uninstall" of that file.
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
; Extracted to {tmp} by PrepareToInstall, to read the installed registration (--read-registration)
; before anything is copied.
Source: "{#Stage}\ams2-connector-ui.exe"; DestDir: "{tmp}"; Flags: dontcopy
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
  ExitNoRegistration = 2;   { --read-registration: no valid registration of this connector }
  ExitManaged = 4;          { --quit: a managing app owns the connector }

var
  ProgressFile: String;   { /PROGRESS=<file>, '' when not asked }
  FailReason: String;     { the "reason" of the done line }
  Started: Boolean;       { the files started to be copied (or removed) }
  Installed: Boolean;     { the files are installed but --register failed }
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
  if Installed and (FailReason <> '') then
  begin
    { the files are in place; only the registration failed }
    Emit('{"stage":"done","ok":false,"reason":' + JsonString(FailReason) + ',"installed":true}');
    Exit;
  end;
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

{ How many of the connector's three programs run from the folder InstallDir (by image path, so
  any other copy is never counted). If WMI is unavailable, the connector's single-instance
  mutexes answer instead (they cannot tell which copy holds them). }
function CountRunning(const InstallDir: String): Integer;
var
  Locator, Service, Items, Item: Variant;
  I: Integer;
  Dir, Path: String;
begin
  Result := 0;
  Dir := Lowercase(AddBackslash(InstallDir));
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

function WaitStopped(const InstallDir: String; const Seconds: Integer): Boolean;
var
  I: Integer;
begin
  Result := CountRunning(InstallDir) = 0;
  I := 0;
  while (not Result) and (I < Seconds * 4) do
  begin
    Sleep(250);
    I := I + 1;
    Result := CountRunning(InstallDir) = 0;
  end;
end;

{ Leaves none of the connector's programs of the folder InstallDir running (this install's, or
  the other flavour's), or returns the reason it could not.
  /MANAGED: Stintmate stopped the connector, so one still running is a failure and nothing is
  closed. Otherwise: ask the control app of that folder to exit (--quit, which finds the
  workspace's key itself). A connector Stintmate manages (exit 4) is Stintmate's to stop: a
  silent run fails (it never closes Stintmate), an interactive one asks to close Stintmate and
  waits for it to stop the connector. }
function EnsureConnectorStopped(const InstallDir: String): String;
var
  Code: Integer;
  Managed: Boolean;
  What: String;
begin
  Result := '';
  if CountRunning(InstallDir) = 0 then
    Exit;
  if CmdFlag('MANAGED') then
  begin
    Result := 'connector_running';
    Exit;
  end;
  Managed := False;
  if FileExists(AddBackslash(InstallDir) + UiName) then
    if Exec(AddBackslash(InstallDir) + UiName, '--quit', '', SW_HIDE, ewWaitUntilTerminated, Code) then
      Managed := Code = ExitManaged;
  if (not Managed) and WaitStopped(InstallDir, 3) then
    Exit;
  if IsSilent then
  begin
    Result := 'connector_running';
    Exit;
  end;
  What := 'continue the installation';
  if IsUninstaller then
    What := 'remove the AMS2 connector';
  if not QuitAllOrAsk(What) then
  begin
    Result := 'connector_running';
    Exit;
  end;
  if not WaitStopped(InstallDir, 15) then
    Result := 'connector_running';
end;

function ConnectorRunningText: String;
begin
  Result := 'The AMS2 connector is running. Close it (and Stintmate, which manages it) and run the setup again.';
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

{ The value of Key in the "key=value" lines --read-registration wrote ('' when absent). }
function RegistrationValue(const Lines: TArrayOfString; const Key: String): String;
var
  I, P: Integer;
begin
  Result := '';
  for I := 0 to GetArrayLength(Lines) - 1 do
  begin
    P := Pos('=', Lines[I]);
    if (P > 0) and (CompareText(Copy(Lines[I], 1, P - 1), Key) = 0) then
    begin
      Result := Copy(Lines[I], P + 1, MaxInt);
      Exit;
    end;
  end;
end;

{ One flavour at a time. Before anything is copied: the new control app, extracted to the temp
  folder, reads the registration file (--read-registration; no workspace is involved). When it
  names another folder than the install folder, that install (the other flavour, or another copy) is stopped
  and removed through the "uninstall" command of the file, and the setup waits until its launcher
  is gone. A registration with no "uninstall" (a development checkout) is left alone and simply
  overwritten by --register. Finally this install's own programs are stopped. }
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code, I: Integer;
  Lines: TArrayOfString;
  ListFile, OtherDir, OtherLauncher, OtherUninstall, OtherArgs: String;
  Ran: Boolean;
begin
  Result := '';
  Emit('{"stage":"prepare"}');
  OtherDir := '';
  OtherLauncher := '';
  OtherUninstall := '';
  OtherArgs := '';
  ExtractTemporaryFile(UiName);
  ListFile := ExpandConstant('{tmp}\registration.txt');
  DeleteFile(ListFile);
  if not Exec(ExpandConstant('{tmp}\' + UiName), '--read-registration "' + ListFile + '"', '', SW_HIDE, ewWaitUntilTerminated, Code) then
    Code := 1;
  if Code = 0 then
  begin
    { LoadStringsFromFile reads the UTF-8 BOM the file starts with: non-ASCII paths stay intact }
    if not LoadStringsFromFile(ListFile, Lines) then
      Code := 1
    else
    begin
      OtherLauncher := RegistrationValue(Lines, 'launcher');
      OtherDir := RegistrationValue(Lines, 'dir');
      OtherUninstall := RegistrationValue(Lines, 'uninstall');
      OtherArgs := RegistrationValue(Lines, 'uninstall_args');
    end;
  end;
  if (Code <> 0) and (Code <> ExitNoRegistration) then
  begin
    FailReason := 'register_failed';
    Result := 'The connector''s registration cannot be read. Nothing was installed.';
    Exit;
  end;

  if (OtherDir <> '') and (CompareText(RemoveBackslash(OtherDir), RemoveBackslash(ExpandConstant('{app}'))) <> 0) then
  begin
    FailReason := EnsureConnectorStopped(OtherDir);
    if FailReason <> '' then
    begin
      Result := ConnectorRunningText;
      Exit;
    end;
    if (OtherUninstall <> '') and FileExists(OtherUninstall) then
    begin
      Emit('{"stage":"replace"}');
      Ran := Exec(OtherUninstall, OtherArgs, '', SW_HIDE, ewWaitUntilTerminated, Code);
      { Inno's uninstaller hands over to a copy of itself and returns at once: wait (30 s) until
        the other install's launcher is gone }
      I := 0;
      while Ran and FileExists(OtherLauncher) and (I < 120) do
      begin
        Sleep(250);
        I := I + 1;
      end;
      if (not Ran) or FileExists(OtherLauncher) then
      begin
        FailReason := 'replace_failed';
        Result := 'The other installation of the AMS2 connector (' + OtherDir + ') could not be removed. Uninstall it from Windows'' Installed apps, then run this setup again.';
        Exit;
      end;
    end;
  end;

  FailReason := EnsureConnectorStopped(ExpandConstant('{app}'));
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

{ Registers the connector in the machine file: ams2-connector-ui.exe --register --uninstall
  "<this uninstaller>" [--flavour dev]. No workspace is needed. }
procedure RegisterConnector;
var
  Code: Integer;
  Params: String;
begin
  Emit('{"stage":"register"}');
  Params := '--register --uninstall "' + ExpandConstant('{uninstallexe}') + '"';
#ifdef Dev
  Params := Params + ' --flavour dev';
#endif
  if not Exec(ExpandConstant('{app}\' + UiName), Params, '', SW_HIDE, ewWaitUntilTerminated, Code) then
    Code := 1;
  if Code = 0 then
    Exit;
  { rare: the files stay (Abort cannot undo them) and the run ends with exit status 9 and
    "installed":true }
  Installed := True;
  FailReason := 'register_failed';
  if not WizardSilent then
    MsgBox('The AMS2 connector is installed, but it could not be registered (code ' + IntToStr(Code) + '). Run the setup again.', mbCriticalError, MB_OK);
end;

function GetCustomSetupExitCode: Integer;
begin
  if Installed and (FailReason <> '') then
    Result := 9
  else
    Result := 0;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssInstall then
  begin
    Started := True;
    LastPercent := 0;     { the 0 line is written here; progress adds one line per change }
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
  FailReason := EnsureConnectorStopped(ExpandConstant('{app}'));
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
    { removes %LOCALAPPDATA%\Stintmate\connectors\ams2.json only when it names this install (a
      file of another install, or none, is left as it is: exit 0) }
    if not Exec(ExpandConstant('{app}\' + UiName), '--unregister', '', SW_HIDE, ewWaitUntilTerminated, Code) then
      Code := 1;
    if Code <> 0 then
      Warning := 'unregister_failed';
    EmitFiles(0);
  end
  else if CurUninstallStep = usPostUninstall then
  begin
    EmitFiles(100);
  end
  else if CurUninstallStep = usDone then
    Finished := True;
end;

procedure DeinitializeUninstall;
begin
  EmitDone;
end;
