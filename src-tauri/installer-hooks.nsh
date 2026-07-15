!define LEGACY_PRODUCT_NAME "Zenith Codex"
!define LEGACY_UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${LEGACY_PRODUCT_NAME}"
!define LEGACY_PRODUCT_KEY "Software\${MANUFACTURER}\${LEGACY_PRODUCT_NAME}"

!macro NSIS_HOOK_PREINSTALL
  ${If} $INSTDIR == "$LOCALAPPDATA\${PRODUCTNAME}"
    StrCpy $INSTDIR "$LOCALAPPDATA\Programs\${PRODUCTNAME}"
    SetOutPath "$INSTDIR"
  ${EndIf}

  ReadRegStr $R0 HKCU "${LEGACY_UNINSTALL_KEY}" "UninstallString"
  ${If} $R0 != ""
    ReadRegStr $R1 HKCU "${LEGACY_PRODUCT_KEY}" ""
    ${If} $R1 == ""
      MessageBox MB_ICONSTOP|MB_OK "The existing ${LEGACY_PRODUCT_NAME} installation location could not be verified. Uninstall it manually, then install ${PRODUCTNAME} again."
      Abort
    ${EndIf}

    ${IfNot} ${FileExists} "$R1\uninstall.exe"
      MessageBox MB_ICONSTOP|MB_OK "The existing ${LEGACY_PRODUCT_NAME} uninstaller could not be found. Uninstall it manually, then install ${PRODUCTNAME} again."
      Abort
    ${EndIf}

    ExecWait '"$R1\uninstall.exe" /S _?=$R1' $R2
    ReadRegStr $R3 HKCU "${LEGACY_UNINSTALL_KEY}" "UninstallString"
    ${If} $R2 != 0
    ${OrIf} $R3 != ""
      MessageBox MB_ICONSTOP|MB_OK "The existing ${LEGACY_PRODUCT_NAME} installation could not be removed. Close it and try again."
      Abort
    ${EndIf}

    Delete "$R1\uninstall.exe"
    RMDir "$R1"
    DeleteRegKey HKCU "${LEGACY_PRODUCT_KEY}"
    DeleteRegKey /ifempty HKCU "Software\${MANUFACTURER}"
  ${EndIf}
!macroend
