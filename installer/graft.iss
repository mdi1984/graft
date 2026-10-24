; Per-user installer for Graft (Inno Setup 6). No admin rights needed:
; installs to %LOCALAPPDATA%\Programs\Graft with a Start menu entry.
; Built by tools\package.ps1, which passes the version and the paths:
;   iscc /DVersion=1.2.3 /DSource=<folder with graft.exe> /DOutput=<dist> installer\graft.iss

#ifndef Version
  #error Pass the version: /DVersion=1.2.3
#endif
#ifndef Source
  #define Source "..\target\release"
#endif
#ifndef Output
  #define Output "..\dist"
#endif

[Setup]
; Never change the AppId: it is how upgrades and uninstall find Graft.
AppId={{F50FBE64-D67E-4FC3-A67B-CF64F3A8535F}
AppName=Graft
AppVersion={#Version}
AppVerName=Graft {#Version}
AppPublisher=mdi1984
AppPublisherURL=https://github.com/mdi1984/graft
AppSupportURL=https://github.com/mdi1984/graft/issues
AppUpdatesURL=https://github.com/mdi1984/graft/releases
VersionInfoVersion={#Version}
PrivilegesRequired=lowest
DefaultDirName={localappdata}\Programs\Graft
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableReadyPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
SetupIconFile=..\crates\graft\assets\icon\graft.ico
UninstallDisplayIcon={app}\graft.exe
UninstallDisplayName=Graft
OutputDir={#Output}
OutputBaseFilename=Graft-{#Version}-x64-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Close a running Graft before replacing it.
CloseApplications=yes
RestartApplications=no

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "{#Source}\graft.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\Graft"; Filename: "{app}\graft.exe"; WorkingDir: "{app}"
Name: "{userdesktop}\Graft"; Filename: "{app}\graft.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\graft.exe"; Description: "Start Graft"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; Left behind by Graft's own updater.
Type: files; Name: "{app}\graft.exe.old"
Type: files; Name: "{app}\graft.exe.new"

[Code]
// Graft's updater runs this setup with /SILENT and:
//   /WAITPID=<id>    wait until the Graft with that process id has closed;
//   /LAUNCH="<exe>"  start Graft again at the end, updated or not.
const
  SYNCHRONIZE = $00100000;

function OpenProcess(Access: DWORD; Inherit: BOOL; Pid: DWORD): THandle;
  external 'OpenProcess@kernel32.dll stdcall';
function WaitForSingleObject(Handle: THandle; Milliseconds: DWORD): DWORD;
  external 'WaitForSingleObject@kernel32.dll stdcall';
function CloseHandle(Handle: THandle): BOOL;
  external 'CloseHandle@kernel32.dll stdcall';

function InitializeSetup(): Boolean;
var
  Pid: Integer;
  Process: THandle;
begin
  Pid := StrToIntDef(ExpandConstant('{param:WAITPID|0}'), 0);
  if Pid > 0 then
  begin
    Process := OpenProcess(SYNCHRONIZE, False, Pid);
    if Process <> 0 then
    begin
      Log(Format('Waiting for Graft (process %d) to close', [Pid]));
      WaitForSingleObject(Process, 60000);
      CloseHandle(Process);
    end;
  end;
  Result := True;
end;

procedure DeinitializeSetup();
var
  Exe: String;
  Code: Integer;
begin
  Exe := ExpandConstant('{param:LAUNCH|}');
  if (Exe <> '') and FileExists(Exe) then
  begin
    if ExecAsOriginalUser(Exe, '', ExtractFileDir(Exe), SW_SHOWNORMAL, ewNoWait, Code) then
      Log('Started Graft again: ' + Exe)
    else
      Log('Could not start Graft again: ' + SysErrorMessage(Code));
  end;
end;
