; Uzumaki for Windows: the installer (Inno Setup 6.3 or newer).
;
; Built by packaging\windows\build.ps1 (the windows workflow runs it):
;   iscc /DAppVersion=0.2.3 /DFileVersion=0.2.3.1 /DRelease=0.2.3 /DSourceDir=<staged files> /DOutputDir=<out> uzumaki.iss
;
; What it does, all for the current user and without admin rights:
;   - installs into %LOCALAPPDATA%\Programs\Uzumaki (or the folder picked; /DIR= when silent):
;     uzumaki.exe, uzu.exe and bridge\ (the playback engine: castLabs Electron, with its Widevine
;     VMP signature, and the bridge's scripts);
;   - a Start menu shortcut that carries Uzumaki's app id (taskbar grouping, the media overlay,
;     notifications), and a desktop shortcut when asked;
;   - adds the folder to the user's PATH, so `uzu` works in a terminal (a task, on by default);
;   - asks a running Uzumaki to quit first (uzu quit), when updating and when uninstalling;
;   - the uninstaller removes the programs, the shortcuts, the PATH entry, the start-at-login
;     entry and the app id's registration. It KEEPS the user's settings, the Apple Music sign-in
;     and the caches (%APPDATA%\Uzumaki, %LOCALAPPDATA%\Uzumaki) unless the user says to delete
;     them (a question, default No; when silent only with /REMOVEDATA).
;
; Silent use (the CI smoke test, winget later):
;   Uzumaki-...-setup.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART [/DIR="C:\x"] [/TASKS="addtopath,desktopicon"]
;   unins000.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART [/REMOVEDATA]
;
; SIGNING. Nothing is Authenticode-signed (SmartScreen warns). If that changes: sign uzumaki.exe
; and uzu.exe before this script runs (build.ps1, "Sign-Files"), and give iscc a sign tool for the
; installer and its uninstaller:   iscc "/Ssigner=<command> $f" ...   plus the two lines below.
; bridge\node_modules\electron\dist\electron.exe must NOT be changed after its VMP signature
; (electron.exe.sig) was made: Authenticode first, VMP last (castLabs' rule on Windows).
;SignTool=signer
;SignedUninstaller=yes

#ifndef AppVersion
  #error Pass /DAppVersion=<version> (see packaging\windows\build.ps1)
#endif
#ifndef FileVersion
  #define FileVersion AppVersion + ".0"
#endif
#ifndef Release
  #define Release AppVersion
#endif
#ifndef SourceDir
  #define SourceDir "..\..\target\windows-dist\Uzumaki"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\target\windows-release"
#endif

; The same id as the app's (app/src/main.rs APP_ID): the taskbar, the media overlay and the
; notifications go by it.
#define AppUserModelId "io.github.hanami117.uzumaki"

[Setup]
; Never change this id: it is how an update finds the installed copy.
AppId={{8669C4B9-FEBE-4B60-A8B1-13C227E304C6}
AppName=Uzumaki
AppVersion={#AppVersion}
AppVerName=Uzumaki {#AppVersion}
AppPublisher=Uzumaki
AppPublisherURL=https://getuzumaki.app
AppSupportURL=https://github.com/hanami117/uzumaki/issues
AppUpdatesURL=https://github.com/hanami117/uzumaki/releases
AppCopyright=Copyright (C) 2026 Thomas Lonigro. GPL-3.0-or-later
VersionInfoVersion={#FileVersion}
VersionInfoProductName=Uzumaki
VersionInfoDescription=Uzumaki Setup
; Per user, no admin: {autopf} is %LOCALAPPDATA%\Programs here.
PrivilegesRequired=lowest
DefaultDirName={autopf}\Uzumaki
DisableProgramGroupPage=yes
DisableDirPage=auto
UsePreviousAppDir=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
OutputDir={#OutputDir}
OutputBaseFilename=Uzumaki-{#Release}-windows-x64-setup
SetupIconFile=uzumaki.ico
UninstallDisplayIcon={app}\uzumaki.exe
UninstallDisplayName=Uzumaki
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
LZMAUseSeparateProcess=yes
InfoBeforeFile=installer-note.txt
; The PATH task changes the user's environment: tell running programs.
ChangesEnvironment=yes
; A running Uzumaki is asked to quit by the code below; this is the fallback for files in use.
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "addtopath"; Description: "Add uzu, Uzumaki's command line, to your PATH"; GroupDescription: "Command line:"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\uzumaki.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\uzu.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\README-WINDOWS.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\THIRD_PARTY_NOTICES.txt"; DestDir: "{app}"; Flags: ignoreversion
; The playback engine, exactly as staged: electron.exe and electron.exe.sig belong together.
Source: "{#SourceDir}\bridge\*"; DestDir: "{app}\bridge"; Flags: ignoreversion recursesubdirs createallsubdirs

[InstallDelete]
; An update replaces the engine whole: no file of an older Electron may stay beside the new one.
Type: filesandordirs; Name: "{app}\bridge"

[Icons]
Name: "{autoprograms}\Uzumaki"; Filename: "{app}\uzumaki.exe"; AppUserModelID: "{#AppUserModelId}"; Comment: "Apple Music player"
Name: "{autodesktop}\Uzumaki"; Filename: "{app}\uzumaki.exe"; AppUserModelID: "{#AppUserModelId}"; Comment: "Apple Music player"; Tasks: desktopicon

[Run]
Filename: "{app}\uzumaki.exe"; Description: "Start Uzumaki"; Flags: nowait postinstall skipifsilent

[Code]
const
  EnvKey = 'Environment';
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';
  ApprovedKey = 'Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run';
  AppIdKey = 'Software\Classes\AppUserModelId\{#AppUserModelId}';

{ The user's PATH without Dir (compared without case, a trailing backslash ignored). }
function PathWithout(const Path, Dir: String): String;
var
  Rest, Item, Want: String;
  P: Integer;
begin
  Result := '';
  Want := Lowercase(RemoveBackslashUnlessRoot(Dir));
  Rest := Path;
  while Rest <> '' do
  begin
    P := Pos(';', Rest);
    if P = 0 then
    begin
      Item := Rest;
      Rest := '';
    end
    else
    begin
      Item := Copy(Rest, 1, P - 1);
      Rest := Copy(Rest, P + 1, Length(Rest));
    end;
    if (Trim(Item) <> '') and (Lowercase(RemoveBackslashUnlessRoot(Trim(Item))) <> Want) then
    begin
      if Result <> '' then
        Result := Result + ';';
      Result := Result + Item;
    end;
  end;
end;

{ Dir is one of the entries of Path (compared without case, a trailing backslash ignored). }
function IsOnPath(const Path, Dir: String): Boolean;
var
  All, Want: String;
begin
  All := ';' + Lowercase(Path) + ';';
  Want := Lowercase(RemoveBackslashUnlessRoot(Dir));
  Result := (Pos(';' + Want + ';', All) > 0) or (Pos(';' + Want + '\;', All) > 0);
end;

{ Adds Dir to the user's PATH (once, at the end) or takes it out. Only HKCU, never the system's.
  A PATH that needs no change is left exactly as it is. }
procedure SetOnPath(const Dir: String; Add: Boolean);
var
  Old, New: String;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, EnvKey, 'Path', Old) then
    Old := '';
  if IsOnPath(Old, Dir) = Add then
    exit;
  if Add then
  begin
    New := Old;
    if (New <> '') and (New[Length(New)] <> ';') then
      New := New + ';';
    New := New + Dir;
  end
  else
    New := PathWithout(Old, Dir);
  if New = '' then
    RegDeleteValue(HKEY_CURRENT_USER, EnvKey, 'Path')
  else
    RegWriteExpandStringValue(HKEY_CURRENT_USER, EnvKey, 'Path', New);
end;

{ Asks a running Uzumaki to quit (the installed uzu does it, and waits until the app is gone, at
  most about 10 seconds). With no Uzumaki running, uzu says so at once and starts nothing. }
procedure QuitUzumaki(const Dir: String);
var
  Uzu: String;
  Code: Integer;
begin
  Uzu := AddBackslash(Dir) + 'uzu.exe';
  if not FileExists(Uzu) then
    exit;
  if not Exec(Uzu, 'quit', '', SW_HIDE, ewWaitUntilTerminated, Code) then
    exit;
  { The playback engine ends a moment after the app; its files are free then. }
  Sleep(1500);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  QuitUzumaki(ExpandConstant('{app}'));
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    SetOnPath(ExpandConstant('{app}'), WizardIsTaskSelected('addtopath'));
end;

{ /REMOVEDATA on the uninstaller's command line. }
function RemoveDataAsked(): Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to ParamCount do
    if CompareText(ParamStr(I), '/REMOVEDATA') = 0 then
      Result := True;
end;

function InitializeUninstall(): Boolean;
begin
  Result := True;
  QuitUzumaki(ExpandConstant('{app}'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Remove: Boolean;
begin
  if CurUninstallStep = usUninstall then
  begin
    SetOnPath(ExpandConstant('{app}'), False);
    { What the app itself wrote for the system: start at login, and the name and icon of its
      notifications. }
    RegDeleteValue(HKEY_CURRENT_USER, RunKey, 'Uzumaki');
    RegDeleteValue(HKEY_CURRENT_USER, ApprovedKey, 'Uzumaki');
    RegDeleteKeyIncludingSubkeys(HKEY_CURRENT_USER, AppIdKey);
  end;
  if CurUninstallStep = usPostUninstall then
  begin
    { The user's own data stays unless they say otherwise. }
    Remove := RemoveDataAsked();
    if (not Remove) and (not UninstallSilent()) then
      Remove := MsgBox('Also delete your Uzumaki settings, your Apple Music sign-in in Uzumaki and its caches from this PC?' + #13#10 + #13#10 +
        'Your Apple Music library and playlists are not touched. Choose No to keep everything for the next time you install Uzumaki.',
        mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES;
    if Remove then
    begin
      DelTree(ExpandConstant('{userappdata}\Uzumaki'), True, True, True);
      DelTree(ExpandConstant('{localappdata}\Uzumaki'), True, True, True);
    end;
  end;
end;
