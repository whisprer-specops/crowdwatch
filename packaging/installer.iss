; Copyright (C) 2026 whispr.dev
; SPDX-License-Identifier: GPL-3.0-only
;
; The Windows installer, compiled with Inno Setup 6 by packaging/package.py, which passes every value below
; with /D options. It shows the licence and needs "I accept", lets the user choose the folder, and copies the
; files there. It writes no registry entries, changes no PATH, needs no administrator rights, and leaves no
; uninstaller: removing the program means deleting its folder.

#ifndef AppName
  #error Build this with packaging/package.py, which supplies the values.
#endif
#ifndef AppFolder
  #define AppFolder AppName
#endif

[Setup]
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=whispr.dev
AppPublisherURL={#AppPublisherURL}
AppSupportURL={#AppPublisherURL}
VersionInfoVersion={#AppVersion}
VersionInfoCompany=whispr.dev
VersionInfoCopyright=Copyright (C) 2026 whispr.dev
VersionInfoDescription={#AppName} {#AppVersion} installer
DefaultDirName={localappdata}\Programs\{#AppFolder}
DisableDirPage=no
UsePreviousAppDir=no
AlwaysShowDirOnReadyPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
Uninstallable=no
LicenseFile={#TermsFile}
InfoAfterFile={#AfterFile}
OutputDir={#OutputDir}
OutputBaseFilename={#OutputBase}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0

[Files]
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
