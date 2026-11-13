; Inno Setup script for RunVideo.
;
; Build with installer\build.ps1, which stages the release executable and a
; bundled FFmpeg into the layout used below, then calls ISCC with /DAppVersion.
;
; The app looks for FFmpeg in `ffmpeg\bin` next to its own executable, so the
; installed copy works without FFmpeg being on PATH.

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif

#define AppName "RunVideo"
#define AppPublisher "Runteryaa"
#define AppUrl "https://github.com/Runteryaa/RunVideo"
#define AppExeName "runvideo.exe"
#define SourceRoot AddBackslash(SourcePath) + ".."

[Setup]
AppId={{A3C9D6F1-2B47-4E8A-9C15-7F0D3B6E8A24}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppUrl}
AppSupportURL={#AppUrl}/issues
AppUpdatesURL={#AppUrl}/releases
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
; Per-user install: no UAC prompt, lands in %LOCALAPPDATA%\Programs by default.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
OutputDir={#SourceRoot}\dist
OutputBaseFilename=RunVideo-{#AppVersion}-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
UninstallDisplayIcon={app}\{#AppExeName}
LicenseFile={#SourceRoot}\LICENSE
InfoAfterFile={#SourceRoot}\THIRD-PARTY.md
VersionInfoVersion={#AppVersion}
VersionInfoCompany={#AppPublisher}
VersionInfoDescription={#AppName} Setup

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "turkish"; MessagesFile: "compiler:Languages\Turkish.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceRoot}\target\release\runvideo.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceRoot}\vendor\ffmpeg\bin\ffmpeg.exe"; DestDir: "{app}\ffmpeg\bin"; Flags: ignoreversion
Source: "{#SourceRoot}\vendor\ffmpeg\bin\ffprobe.exe"; DestDir: "{app}\ffmpeg\bin"; Flags: ignoreversion
Source: "{#SourceRoot}\vendor\ffmpeg\LICENSE-FFmpeg.txt"; DestDir: "{app}\ffmpeg"; Flags: ignoreversion skipifsourcedoesntexist
Source: "{#SourceRoot}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceRoot}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceRoot}\THIRD-PARTY.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExeName}"
Name: "{group}\{cm:UninstallProgram,{#AppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExeName}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; Logs and autosave recovery files belong to the user's projects, so nothing
; outside {app} is touched.
