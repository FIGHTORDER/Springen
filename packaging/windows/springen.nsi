; Springen Windows installer.
;
; Built with NSIS, which cross-builds on Linux, so the installer comes out of
; the same CI run as the binaries it wraps. Both executables are statically
; linked against nothing but system DLLs, so there is no runtime to ship.

Unicode true
ManifestDPIAware true

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "x64.nsh"
!include "FileFunc.nsh"

!ifndef VERSION
  !define VERSION "0.1.0"
!endif
!ifndef SRCDIR
  !define SRCDIR "."
!endif

!define APPNAME    "Springen"
!define PUBLISHER  "Springen"
!define REGKEY     "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APPNAME}"

Name "${APPNAME} ${VERSION}"
OutFile "${OUTFILE}"
InstallDir "$PROGRAMFILES64\${APPNAME}"
InstallDirRegKey HKLM "Software\${APPNAME}" "InstallDir"
RequestExecutionLevel admin
SetCompressor /SOLID lzma

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName"     "${APPNAME}"
VIAddVersionKey "FileDescription" "${APPNAME} installer"
VIAddVersionKey "FileVersion"     "${VERSION}.0"
VIAddVersionKey "ProductVersion"  "${VERSION}.0"
VIAddVersionKey "CompanyName"     "${PUBLISHER}"
VIAddVersionKey "LegalCopyright"  "MIT licensed"

!define MUI_ICON   "${SRCDIR}\springen.ico"
!define MUI_UNICON "${SRCDIR}\springen.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\springen-app.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Open Springen"

!insertmacro MUI_PAGE_LICENSE "${SRCDIR}\LICENSE.txt"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "Springen needs 64-bit Windows."
    Abort
  ${EndIf}
  SetRegView 64
  ; Shortcuts belong to the machine, not to whoever ran the installer.
  ;
  ; This installs into Program Files and writes its uninstall entry to HKLM,
  ; so it is a per-machine install in every other respect — but `$SMPROGRAMS`
  ; and `$DESKTOP` default to the *current user's* folders, which put the Start
  ; Menu entry somewhere the next user of the machine would never find it. It
  ; also made `windows-verify` fail: CI installs silently and then looks under
  ; ProgramData, which is the all-users location and the correct one for an
  ; install of this shape.
  SetShellVarContext all
FunctionEnd

Function un.onInit
  SetRegView 64
  ; The uninstaller has to look where the installer wrote, or it leaves the
  ; Start Menu folder and the desktop shortcut behind.
  SetShellVarContext all
FunctionEnd

Section "Springen (desktop app)" SEC_APP
  SectionIn RO
  SetOutPath "$INSTDIR"
  File "${SRCDIR}\springen-app.exe"
  File "${SRCDIR}\springen.ico"
  File "${SRCDIR}\README.txt"
  File "${SRCDIR}\LICENSE.txt"

  WriteRegStr HKLM "Software\${APPNAME}" "InstallDir" "$INSTDIR"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; Add/Remove Programs
  WriteRegStr   HKLM "${REGKEY}" "DisplayName"     "${APPNAME}"
  WriteRegStr   HKLM "${REGKEY}" "DisplayVersion"  "${VERSION}"
  WriteRegStr   HKLM "${REGKEY}" "Publisher"       "${PUBLISHER}"
  WriteRegStr   HKLM "${REGKEY}" "DisplayIcon"     "$INSTDIR\springen.ico"
  WriteRegStr   HKLM "${REGKEY}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  WriteRegStr   HKLM "${REGKEY}" "InstallLocation" "$INSTDIR"
  WriteRegDWORD HKLM "${REGKEY}" "NoModify" 1
  WriteRegDWORD HKLM "${REGKEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  WriteRegDWORD HKLM "${REGKEY}" "EstimatedSize" "$0"

  CreateDirectory "$SMPROGRAMS\${APPNAME}"
  CreateShortcut "$SMPROGRAMS\${APPNAME}\${APPNAME}.lnk" "$INSTDIR\springen-app.exe" "" "$INSTDIR\springen.ico"
  CreateShortcut "$SMPROGRAMS\${APPNAME}\Uninstall ${APPNAME}.lnk" "$INSTDIR\uninstall.exe"
SectionEnd

Section "Command line tool (springen.exe)" SEC_CLI
  SetOutPath "$INSTDIR"
  File "${SRCDIR}\springen.exe"
  CreateDirectory "$SMPROGRAMS\${APPNAME}"
  CreateShortcut "$SMPROGRAMS\${APPNAME}\Springen command prompt.lnk" \
    "$WINDIR\system32\cmd.exe" "/K cd /d $\"$INSTDIR$\"" "$INSTDIR\springen.ico"
SectionEnd

; Deliberately no "add to PATH" option. Rewriting the system PATH from NSIS
; means read-modify-write through a string variable, and the standard build
; caps strings at 1024 characters -- which is exactly how installers truncate
; people's PATH. README.txt explains how to add it by hand instead. The Start
; Menu gets a command prompt that opens here, which covers most of the need.

Section /o "Desktop shortcut" SEC_DESKTOP
  CreateShortcut "$DESKTOP\${APPNAME}.lnk" "$INSTDIR\springen-app.exe" "" "$INSTDIR\springen.ico"
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_APP} \
    "The node-graph map design tool."
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_CLI} \
    "springen.exe, for baking maps from a project file without opening the app."
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_DESKTOP} \
    "Also place a shortcut on the desktop."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  SetRegView 64

  Delete "$INSTDIR\springen-app.exe"
  Delete "$INSTDIR\springen.exe"
  Delete "$INSTDIR\springen.ico"
  Delete "$INSTDIR\README.txt"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\${APPNAME}\${APPNAME}.lnk"
  Delete "$SMPROGRAMS\${APPNAME}\Uninstall ${APPNAME}.lnk"
  Delete "$SMPROGRAMS\${APPNAME}\Springen command prompt.lnk"
  RMDir  "$SMPROGRAMS\${APPNAME}"
  Delete "$DESKTOP\${APPNAME}.lnk"

  DeleteRegKey HKLM "${REGKEY}"
  DeleteRegKey HKLM "Software\${APPNAME}"
SectionEnd
