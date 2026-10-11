; Carnet's Windows installer. scripts/package.nu runs it as
;
;   makensis -DVERSION=0.1.0 -DEXE=<carnet.exe> -DOUT=<setup.exe> carnet.nsi
;
; It installs for the current user only, so it never asks for an
; administrator: into %LOCALAPPDATA%\Programs\Carnet, with a Start menu
; shortcut and an entry in Settings > Apps that uninstalls it. Running a
; newer one over an older one upgrades it in place. Uninstalling removes
; the program and leaves the notes (Documents\Carnet) and the settings
; (%APPDATA%\Carnet) where they are.

Unicode true
SetCompressor /SOLID lzma
RequestExecutionLevel user
ManifestDPIAware true

!ifndef VERSION
  !error "VERSION is not defined (makensis -DVERSION=...)"
!endif
!ifndef EXE
  !error "EXE is not defined (makensis -DEXE=path\to\carnet.exe)"
!endif
!ifndef OUT
  !define OUT "Carnet-${VERSION}-windows-x64-setup.exe"
!endif

!define APP "Carnet"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP}"

Name "${APP}"
OutFile "${OUT}"
InstallDir "$LOCALAPPDATA\Programs\${APP}"
InstallDirRegKey HKCU "Software\${APP}" "InstallDir"
BrandingText "${APP} ${VERSION}"

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${APP}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "${APP} installer"
VIAddVersionKey "LegalCopyright" ""

!include "MUI2.nsh"

!define MUI_ICON "..\..\assets\icon\carnet.ico"
!define MUI_UNICON "..\..\assets\icon\carnet.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\carnet.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Open ${APP}"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Section "Carnet" SecMain
  SectionIn RO
  SetShellVarContext current
  SetOutPath "$INSTDIR"
  ; A running Carnet holds carnet.exe open; ask rather than fail half-way.
  retry:
  ClearErrors
  Delete "$INSTDIR\carnet.exe"
  IfErrors 0 +3
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "Carnet is open. Close it, then choose Retry." IDRETRY retry
    Abort
  File "/oname=carnet.exe" "${EXE}"

  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateShortcut "$SMPROGRAMS\${APP}.lnk" "$INSTDIR\carnet.exe"

  WriteRegStr HKCU "Software\${APP}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "${APP}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "qxuken"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\carnet.exe,0"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
  ; Settings > Apps shows the size; it is in KB.
  SectionGetSize ${SecMain} $0
  WriteRegDWORD HKCU "${UNINST_KEY}" "EstimatedSize" $0
SectionEnd

Section "Uninstall"
  SetShellVarContext current
  Delete "$INSTDIR\carnet.exe"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\${APP}.lnk"
  DeleteRegKey HKCU "${UNINST_KEY}"
  DeleteRegKey HKCU "Software\${APP}"
SectionEnd
