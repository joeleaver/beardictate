; beardictate for Windows: a per-user Inno Setup installer (no admin, no
; UAC prompt). See DESIGN.md "Windows install and first run".
;
; Built by scripts/package-windows.ps1, which stages the files into
; packaging\windows\stage and runs:
;   iscc /DAppVersion=0.1.0 /DStage=<stage dir> packaging\windows\beardictate.iss
; Add /DSign to sign the installer and uninstaller with the "bdsign" sign
; tool, which the caller defines (iscc "/Sbdsign=signtool sign ... $f").
;
; The installer ships no models: the app downloads them on first run into
; %LOCALAPPDATA%\beardictate\models.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef Stage
  #define Stage "stage"
#endif

[Setup]
; Never change AppId: upgrades and the uninstaller find the app by it.
AppId={{6B1D2C4E-0F57-4E8B-9C1A-B3A7D9E2F410}
AppName=beardictate
AppVersion={#AppVersion}
AppVerName=beardictate {#AppVersion}
AppPublisher=Joe Leaver
AppPublisherURL=https://github.com/joeleaver/beardictate
AppSupportURL=https://github.com/joeleaver/beardictate/issues
; Per user: %LOCALAPPDATA%\Programs\beardictate, like VS Code's user setup.
PrivilegesRequired=lowest
DefaultDirName={autopf}\beardictate
DisableDirPage=yes
DefaultGroupName=beardictate
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
OutputDir=..\..\dist
OutputBaseFilename=beardictate-{#AppVersion}-windows-x64-setup
SetupIconFile=beardictate.ico
UninstallDisplayIcon={app}\beardictate.ico
UninstallDisplayName=beardictate
LicenseFile=..\..\LICENSE
; Shown before installing: what this alpha sends (docs/telemetry.md).
InfoBeforeFile={#Stage}\ALPHA-NOTICE.txt
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; An upgrade closes the running app (Restart Manager) and starts it again.
CloseApplications=yes
RestartApplications=yes
#ifdef Sign
SignTool=bdsign
SignedUninstaller=yes
#endif

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "startatlogin"; Description: "Start beardictate when I sign in"; GroupDescription: "Startup:"
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Shortcuts:"; Flags: unchecked

[Files]
Source: "{#Stage}\beardictate.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\bd.exe"; DestDir: "{app}"; Flags: ignoreversion
; sherpa-onnx and llama.cpp are shared libraries that must sit next to the
; exes; llama.cpp's backends (ggml-cpu-*.dll, ggml-vulkan.dll) are loaded from
; there at run time.
Source: "{#Stage}\*.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "beardictate.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\THIRD-PARTY.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\ALPHA-NOTICE.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\beardictate"; Filename: "{app}\beardictate.exe"; IconFilename: "{app}\beardictate.ico"; Comment: "Dictation for fiction writers"
Name: "{autodesktop}\beardictate"; Filename: "{app}\beardictate.exe"; IconFilename: "{app}\beardictate.ico"; Tasks: desktopicon

[Registry]
; The same value the app's "Start at login" setting writes, so either can
; turn it off. Removed on uninstall either way.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "beardictate"; ValueData: """{app}\beardictate.exe"""; Tasks: startatlogin; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueName: "beardictate"; Flags: dontcreatekey uninsdeletevalue

[Run]
Filename: "{app}\beardictate.exe"; Description: "Start beardictate now"; Flags: nowait postinstall skipifsilent
; An update from the app runs this installer silently with /relaunch=1 and
; quits; start the new version.
Filename: "{app}\beardictate.exe"; Flags: nowait runasoriginaluser; Check: Relaunch

[UninstallDelete]
; Downloaded updates, and alpha telemetry waiting to be sent: nothing the
; user made, so always removed.
Type: filesandordirs; Name: "{localappdata}\beardictate\data\updates"
Type: filesandordirs; Name: "{localappdata}\beardictate\data\telemetry"

[UninstallRun]
; Stop the tray app so its files can be removed.
Filename: "{sys}\taskkill.exe"; Parameters: "/IM beardictate.exe /F"; Flags: runhidden; RunOnceId: "StopApp"

[Code]
function Relaunch: Boolean;
begin
  Result := ExpandConstant('{param:relaunch|0}') = '1';
end;

// Models (~0.7–3 GB) and settings outlive an uninstall unless the user says
// otherwise, so a reinstall doesn't download them again. A silent uninstall
// keeps them.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if (CurUninstallStep = usPostUninstall) and not UninstallSilent then
    if MsgBox('Also delete your beardictate settings, special words and downloaded models?' + #13#10 + #13#10 +
              ExpandConstant('{localappdata}\beardictate') + #13#10 + ExpandConstant('{userappdata}\beardictate'),
              mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
    begin
      DelTree(ExpandConstant('{localappdata}\beardictate'), True, True, True);
      DelTree(ExpandConstant('{userappdata}\beardictate'), True, True, True);
    end;
end;
