; Definitions and the uninstall manifest are supplied by tools/installers.py.
Unicode true
RequestExecutionLevel user
ManifestDPIAware true
SetCompressor /SOLID lzma
Name "Light"
OutFile "${OUTPUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\Light"
InstallDirRegKey HKCU "Software\Light\Installer" "InstallLocation"
BrandingText "Light — a modular IDE for any language"
VIProductVersion "${APP_VERSION}.0"
VIAddVersionKey /LANG=1033 "ProductName" "Light"
VIAddVersionKey /LANG=1033 "FileDescription" "Light installer"
VIAddVersionKey /LANG=1033 "FileVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=1033 "ProductVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=1033 "LegalCopyright" "Light contributors"

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "x64.nsh"
!define MUI_ICON "${APP_ICON}"
!define MUI_UNICON "${APP_ICON}"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\light.exe"
!define MUI_LANGDLL_REGISTRY_ROOT HKCU
!define MUI_LANGDLL_REGISTRY_KEY "Software\Light\Installer"
!define MUI_LANGDLL_REGISTRY_VALUENAME "Language"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_UNPAGE_FINISH
!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "Russian"

LangString LightCore ${LANG_ENGLISH} "Light IDE (required)"
LangString LightCore ${LANG_RUSSIAN} "Light IDE (обязательно)"
LangString LightDesktop ${LANG_ENGLISH} "Desktop shortcut"
LangString LightDesktop ${LANG_RUSSIAN} "Ярлык на рабочем столе"
LangString LightClose ${LANG_ENGLISH} "Close Light before continuing, then click Retry."
LangString LightClose ${LANG_RUSSIAN} "Закройте Light и нажмите «Повторить»."
LangString LightUpgradeFailed ${LANG_ENGLISH} "The previous installation could not be removed. Close Light and try again."
LangString LightUpgradeFailed ${LANG_RUSSIAN} "Не удалось удалить предыдущую установку. Закройте Light и попробуйте ещё раз."

!macro CheckClosed tag
  check_${tag}:
    IfFileExists "$INSTDIR\light.exe" 0 done_${tag}
    System::Call 'kernel32::CreateFileW(w "$INSTDIR\light.exe", i 0x40000000, i 7, p 0, i 3, i 0, p 0) p.r0'
    StrCmp $0 -1 0 close_${tag}
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "$(LightClose)" /SD IDCANCEL IDRETRY check_${tag}
    SetErrorLevel 1
    Abort
  close_${tag}:
    System::Call 'kernel32::CloseHandle(p r0)'
  done_${tag}:
!macroend

Function .onInit
  SetShellVarContext current
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "Light requires 64-bit Windows." /SD IDOK
    SetErrorLevel 1
    Abort
  ${EndIf}
  ${Unless} ${Silent}
    !insertmacro MUI_LANGDLL_DISPLAY
  ${EndUnless}
FunctionEnd

Section "$(LightCore)" Core
  SectionIn RO
  !insertmacro CheckClosed "install"
  ; Remove only files recorded by the old installer before replacing the payload.
  IfFileExists "$INSTDIR\Uninstall.exe" 0 no_upgrade
    ExecWait '"$INSTDIR\Uninstall.exe" /S _?=$INSTDIR' $0
    ${If} $0 != 0
      MessageBox MB_ICONSTOP "$(LightUpgradeFailed)" /SD IDOK
      SetErrorLevel 1
      Abort
    ${EndIf}
  no_upgrade:
  SetOutPath "$INSTDIR"
  SetOverwrite on
  File /r "${PAYLOAD_DIR}\*"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateDirectory "$SMPROGRAMS\Light"
  CreateShortcut "$SMPROGRAMS\Light\Light.lnk" "$INSTDIR\light.exe"
  CreateShortcut "$SMPROGRAMS\Light\Uninstall Light.lnk" "$INSTDIR\Uninstall.exe"
  WriteRegStr HKCU "Software\Light\Installer" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "Light"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "Light contributors"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\light.exe"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '$\"$INSTDIR\Uninstall.exe$\"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '$\"$INSTDIR\Uninstall.exe$\" /S'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" ${PAYLOAD_KIB}
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
SectionEnd

Section /o "$(LightDesktop)" Desktop
  CreateShortcut "$DESKTOP\Light.lnk" "$INSTDIR\light.exe"
SectionEnd

Function un.onInit
  SetShellVarContext current
  !insertmacro MUI_UNGETLANGUAGE
FunctionEnd

Section "Uninstall"
  !insertmacro CheckClosed "uninstall"
  ; No recursive deletion: keep files created by the user, projects and settings.
  !include "${UNINSTALL_MANIFEST}"
  ReadRegStr $0 HKCU "${UNINSTALL_KEY}" "InstallLocation"
  ${If} $0 == $INSTDIR
    Delete "$SMPROGRAMS\Light\Light.lnk"
    Delete "$SMPROGRAMS\Light\Uninstall Light.lnk"
    RMDir "$SMPROGRAMS\Light"
    Delete "$DESKTOP\Light.lnk"
    DeleteRegKey HKCU "${UNINSTALL_KEY}"
    DeleteRegKey HKCU "Software\Light\Installer"
    DeleteRegKey /ifempty HKCU "Software\Light"
  ${EndIf}
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
SectionEnd
