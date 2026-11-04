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
  ; Offer Light through Open With / Default Apps without changing UserChoice.
  WriteRegStr HKCU "Software\Classes\Applications\light.exe" "FriendlyAppName" "Light Quick View"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\shell\open\command" "" '$\"$INSTDIR\light.exe$\" --view $\"%1$\"'
  WriteRegStr HKCU "Software\Classes\Light.Source" "" "Source code (Light)"
  WriteRegStr HKCU "Software\Classes\Light.Source\DefaultIcon" "" '$\"$INSTDIR\light.exe$\",0'
  WriteRegStr HKCU "Software\Classes\Light.Source\shell\open\command" "" '$\"$INSTDIR\light.exe$\" --view $\"%1$\"'
  WriteRegStr HKCU "Software\Light\Capabilities" "ApplicationName" "Light"
  WriteRegStr HKCU "Software\Light\Capabilities" "ApplicationDescription" "Light code viewer and IDE"
  WriteRegStr HKCU "Software\RegisteredApplications" "Light" "Software\Light\Capabilities"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".txt" ""
  WriteRegStr HKCU "Software\Classes\.txt\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".txt" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".rs" ""
  WriteRegStr HKCU "Software\Classes\.rs\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".rs" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".js" ""
  WriteRegStr HKCU "Software\Classes\.js\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".js" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".mjs" ""
  WriteRegStr HKCU "Software\Classes\.mjs\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".mjs" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".cjs" ""
  WriteRegStr HKCU "Software\Classes\.cjs\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".cjs" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".ts" ""
  WriteRegStr HKCU "Software\Classes\.ts\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".ts" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".tsx" ""
  WriteRegStr HKCU "Software\Classes\.tsx\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".tsx" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".jsx" ""
  WriteRegStr HKCU "Software\Classes\.jsx\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".jsx" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".py" ""
  WriteRegStr HKCU "Software\Classes\.py\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".py" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".c" ""
  WriteRegStr HKCU "Software\Classes\.c\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".c" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".h" ""
  WriteRegStr HKCU "Software\Classes\.h\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".h" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".cpp" ""
  WriteRegStr HKCU "Software\Classes\.cpp\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".cpp" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".hpp" ""
  WriteRegStr HKCU "Software\Classes\.hpp\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".hpp" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".lisp" ""
  WriteRegStr HKCU "Software\Classes\.lisp\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".lisp" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".lsp" ""
  WriteRegStr HKCU "Software\Classes\.lsp\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".lsp" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".cl" ""
  WriteRegStr HKCU "Software\Classes\.cl\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".cl" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".el" ""
  WriteRegStr HKCU "Software\Classes\.el\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".el" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".als" ""
  WriteRegStr HKCU "Software\Classes\.als\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".als" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".html" ""
  WriteRegStr HKCU "Software\Classes\.html\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".html" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".htm" ""
  WriteRegStr HKCU "Software\Classes\.htm\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".htm" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".css" ""
  WriteRegStr HKCU "Software\Classes\.css\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".css" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".md" ""
  WriteRegStr HKCU "Software\Classes\.md\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".md" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".rst" ""
  WriteRegStr HKCU "Software\Classes\.rst\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".rst" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".json" ""
  WriteRegStr HKCU "Software\Classes\.json\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".json" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".toml" ""
  WriteRegStr HKCU "Software\Classes\.toml\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".toml" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".yaml" ""
  WriteRegStr HKCU "Software\Classes\.yaml\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".yaml" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".yml" ""
  WriteRegStr HKCU "Software\Classes\.yml\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".yml" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".ini" ""
  WriteRegStr HKCU "Software\Classes\.ini\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".ini" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".env" ""
  WriteRegStr HKCU "Software\Classes\.env\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".env" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".sh" ""
  WriteRegStr HKCU "Software\Classes\.sh\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".sh" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".bash" ""
  WriteRegStr HKCU "Software\Classes\.bash\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".bash" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".asm" ""
  WriteRegStr HKCU "Software\Classes\.asm\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".asm" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".s" ""
  WriteRegStr HKCU "Software\Classes\.s\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".s" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".make" ""
  WriteRegStr HKCU "Software\Classes\.make\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".make" "Light.Source"
  WriteRegStr HKCU "Software\Classes\Applications\light.exe\SupportedTypes" ".mk" ""
  WriteRegStr HKCU "Software\Classes\.mk\OpenWithProgids" "Light.Source" ""
  WriteRegStr HKCU "Software\Light\Capabilities\FileAssociations" ".mk" "Light.Source"

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
    DeleteRegKey HKCU "Software\Classes\Applications\light.exe"
    DeleteRegKey HKCU "Software\Classes\Light.Source"
    DeleteRegKey HKCU "Software\Light\Capabilities"
    DeleteRegValue HKCU "Software\RegisteredApplications" "Light"
    DeleteRegValue HKCU "Software\Classes\.txt\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.rs\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.js\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.mjs\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.cjs\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.ts\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.tsx\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.jsx\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.py\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.c\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.h\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.cpp\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.hpp\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.lisp\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.lsp\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.cl\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.el\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.als\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.html\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.htm\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.css\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.md\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.rst\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.json\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.toml\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.yaml\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.yml\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.ini\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.env\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.sh\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.bash\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.asm\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.s\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.make\OpenWithProgids" "Light.Source"
    DeleteRegValue HKCU "Software\Classes\.mk\OpenWithProgids" "Light.Source"

    DeleteRegKey /ifempty HKCU "Software\Light"
  ${EndIf}
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
SectionEnd
