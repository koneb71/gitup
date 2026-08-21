; Inno Setup script for the Gitup installer.
;
; Inno Setup rather than WiX: both are preinstalled on the GitHub Actions
; Windows runners, and this produces a friendlier .exe wizard for what is a
; single self-contained binary. An MSI earns its complexity when there is
; enterprise deployment to satisfy, and nothing here needs that yet.
;
; AppVersion and the source paths arrive as /D defines from
; scripts/package-windows.ps1, so nothing here has to be edited for a release.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceExe
  #define SourceExe "..\..\target\release\gitup.exe"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\target"
#endif
#ifndef OutputName
  #define OutputName "gitup-setup"
#endif

[Setup]
; Fixed for the life of the application. Upgrades and the uninstaller are
; matched by this, so changing it would strand every existing installation.
AppId={{57CFB853-D400-4F97-940D-20F9930DD361}
AppName=Gitup
AppVersion={#AppVersion}
AppVerName=Gitup {#AppVersion}
AppPublisher=Gitup contributors
AppPublisherURL=https://github.com/koneb71/gitup
AppSupportURL=https://github.com/koneb71/gitup/issues
AppUpdatesURL=https://github.com/koneb71/gitup/releases
DefaultDirName={autopf}\Gitup
DefaultGroupName=Gitup
DisableProgramGroupPage=yes
LicenseFile=..\..\LICENSE
OutputDir={#OutputDir}
OutputBaseFilename={#OutputName}
SetupIconFile=..\icon\gitup.ico
UninstallDisplayIcon={app}\gitup.exe
UninstallDisplayName=Gitup {#AppVersion}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern

; Install per-user by default so no UAC prompt is needed, while still allowing
; a machine-wide install for anyone who wants one. An installer that demands
; administrator rights to place one binary is a poor bargain.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog

; The binary is x64; refusing to run elsewhere is better than installing
; something that cannot start.
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Additional shortcuts:"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{group}\Gitup"; Filename: "{app}\gitup.exe"
Name: "{group}\{cm:UninstallProgram,Gitup}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\Gitup"; Filename: "{app}\gitup.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\gitup.exe"; Description: "{cm:LaunchProgram,Gitup}"; Flags: nowait postinstall skipifsilent

[Code]
// Gitup runs the real git binary for anything touching the network, so a
// machine without it installs fine and then cannot fetch. Saying so during
// setup beats the user discovering it at the first pull.
//
// Deliberately the smallest amount of Pascal Script that does the job: one
// registry check and one single-line message. The first version of this
// spanned several lines and relied on adjacent string literals joining, which
// is a C habit and a compile error here — and the only way to find that out is
// to run the compiler, which needs Windows. Code that cannot be checked
// locally should be code with very little to check.
function GitIsPresent(): Boolean;
begin
  Result := RegKeyExists(HKEY_LOCAL_MACHINE, 'SOFTWARE\GitForWindows');
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and (not GitIsPresent()) then
    MsgBox('Git for Windows was not found. Gitup uses it for fetch, pull, push and clone; everything local works without it. Install it from https://git-scm.com/download/win to enable the rest.', mbInformation, MB_OK);
end;
