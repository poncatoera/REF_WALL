; Wall for Windows: the installer, built by tools/make-installer.ps1 with
; Inno Setup 6 (https://jrsoftware.org/isinfo.php). Don't compile this file
; by hand: the script passes the package folder and version.
;
; What it does:
; - Installs for the current user only, no admin rights: the app goes to
;   %LOCALAPPDATA%\Programs\Wall.
; - Start menu shortcut; desktop shortcut if ticked; "Open in Wall" on the
;   right-click menu of folders whose name ends in .wall (Wall libraries).
; - Running a newer Wall-Setup.exe over an older Wall updates it in place.
;   Wall-Setup.exe /SILENT updates without questions.
; - Uninstall from Settings > Apps.
;
; Libraries (your boards, pictures and videos, by default Wall\Library.wall
; in your user folder) and settings (%APPDATA%\Wall) are never in the app
; folder, so installing, updating and uninstalling never touch them.

#ifndef Package
  #error Pass /DPackage=<folder with wall.exe>
#endif
#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef NumericVersion
  #define NumericVersion "0.0.0.0"
#endif
#ifndef OutDir
  #define OutDir "."
#endif

[Setup]
; Never change AppId: it's how a new installer finds the installed Wall.
AppId={{6F0B4E29-6C1D-4B57-9E0A-3B7E2C9D51A4}
AppName=Wall
AppVersion={#AppVersion}
AppVerName=Wall {#AppVersion}
AppPublisher=Wall
VersionInfoVersion={#NumericVersion}
VersionInfoProductName=Wall
VersionInfoDescription=Wall installer
PrivilegesRequired=lowest
DefaultDirName={autopf}\Wall
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableReadyPage=yes
DisableWelcomePage=no
UsePreviousTasks=yes
UninstallDisplayName=Wall
UninstallDisplayIcon={app}\wall.exe
SetupIconFile=..\..\apps\desktop\assets\wall.ico
WizardStyle=modern
WizardSizePercent=100
Compression=lzma2/ultra64
SolidCompression=yes
OutputDir={#OutDir}
OutputBaseFilename=Wall-Setup
; We check for a running Wall ourselves (see the Code section): closing it
; from here could skip its last save.
CloseApplications=no
ChangesAssociations=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0

[Messages]
WelcomeLabel2=This installs Wall {#AppVersion} on your computer.%n%nIf Wall is already installed, it is updated. Your libraries, boards, pictures and videos are not touched.
FinishedLabel=Wall is installed. Open it from the Start menu.
ConfirmUninstall=Remove Wall from this computer?%n%nYour libraries (boards, pictures and videos) and settings stay where they are.

[Tasks]
Name: desktopicon; Description: "Put Wall on the desktop"; Flags: unchecked
Name: openwith; Description: "Add ""Open in Wall"" to the right-click menu of Wall libraries (.wall folders)"

[InstallDelete]
; Program files from an older version that this one may not ship any more.
; Only program files: nothing else is ever stored in the app folder.
Type: files; Name: "{app}\*.dll"
Type: files; Name: "{app}\wall.exe"
Type: files; Name: "{app}\ffmpeg.exe"
Type: files; Name: "{app}\ffprobe.exe"
; Left by the first installer, which this one replaces.
Type: files; Name: "{app}\uninstall.ps1"
Type: files; Name: "{app}\files.txt"
Type: files; Name: "{app}\version.txt"

[Files]
Source: "{#Package}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Wall"; Filename: "{app}\wall.exe"; WorkingDir: "{app}"; Comment: "Boards of pictures, videos and notes"
Name: "{autodesktop}\Wall"; Filename: "{app}\wall.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Registry]
; "Open in Wall" on folders named *.wall. A library is a folder, so this is
; Wall's version of double-clicking a document.
Root: HKCU; Subkey: "Software\Classes\Directory\shell\Wall"; ValueType: string; ValueName: ""; ValueData: "Open in Wall"; Flags: uninsdeletekey; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\Directory\shell\Wall"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\wall.exe"",0"; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\Directory\shell\Wall"; ValueType: string; ValueName: "AppliesTo"; ValueData: "System.FileName:""*.wall"""; Tasks: openwith
Root: HKCU; Subkey: "Software\Classes\Directory\shell\Wall\command"; ValueType: string; ValueName: ""; ValueData: """{app}\wall.exe"" ""%1"""; Tasks: openwith
; The first installer's entry in Settings > Apps; this one has its own.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Uninstall\Wall"; ValueType: none; Flags: deletekey

[Run]
Filename: "{app}\wall.exe"; Description: "Open Wall"; WorkingDir: "{app}"; Flags: nowait postinstall skipifsilent

[Code]
// A path as a WQL string: backslashes and quotes escaped.
function Escaped(S: String): String;
begin
  Result := S;
  StringChangeEx(Result, '\', '\\', True);
  StringChangeEx(Result, '''', '\''', True);
end;

// True while a wall.exe from the app folder is running.
function WallRunning(): Boolean;
var
  Locator, Service, Found: Variant;
begin
  Result := False;
  try
    Locator := CreateOleObject('WbemScripting.SWbemLocator');
    Service := Locator.ConnectServer('.', 'root\CIMV2');
    Found := Service.ExecQuery('SELECT ProcessId FROM Win32_Process WHERE Name = ''wall.exe'' AND ExecutablePath = ''' + Escaped(ExpandConstant('{app}\wall.exe')) + '''');
    Result := Found.Count > 0;
  except
    // No WMI: carry on; Windows reports locked files if Wall is open.
  end;
end;

// Wall saves when it closes, so ask instead of closing it from here.
function WaitForWallClosed(Silent: Boolean): Boolean;
begin
  Result := True;
  while WallRunning() do
  begin
    if Silent then
    begin
      Result := False;
      Exit;
    end;
    if MsgBox('Wall is open. Close it (your boards are saved when it closes), then click OK.', mbInformation, MB_OKCANCEL) <> IDOK then
    begin
      Result := False;
      Exit;
    end;
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  if not WaitForWallClosed(WizardSilent()) then
    Result := 'Wall is open. Close it and run the installer again.';
end;

function InitializeUninstall(): Boolean;
begin
  Result := WaitForWallClosed(UninstallSilent());
end;
