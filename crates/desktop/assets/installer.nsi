Unicode true
!include "MUI2.nsh"

!ifndef VERSION
  !error "VERSION is required"
!endif
!ifndef STAGE
  !error "STAGE is required"
!endif
!ifndef OUTFILE
  !error "OUTFILE is required"
!endif

Name "Oxide"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\Oxide"
InstallDirRegKey HKCU "Software\dev.oxide.desktop" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma

!define MUI_ICON "${STAGE}\oxide.ico"
!define MUI_UNICON "${STAGE}\oxide.ico"
!define MUI_FINISHPAGE_RUN "$INSTDIR\oxide-desktop.exe"

!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "Install"
  SetOutPath "$INSTDIR"
  File "${STAGE}\oxide-desktop.exe"
  File "${STAGE}\oxide.ico"
  File "${STAGE}\README.txt"
  File /nonfatal /r "${STAGE}\plugins"
  WriteUninstaller "$INSTDIR\Uninstall Oxide.exe"

  CreateShortCut "$SMPROGRAMS\Oxide.lnk" "$INSTDIR\oxide-desktop.exe" "" "$INSTDIR\oxide.ico"
  CreateShortCut "$DESKTOP\Oxide.lnk" "$INSTDIR\oxide-desktop.exe" "" "$INSTDIR\oxide.ico"

  WriteRegStr HKCU "Software\dev.oxide.desktop" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\dev.oxide.desktop" "DisplayName" "Oxide"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\dev.oxide.desktop" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\dev.oxide.desktop" "DisplayIcon" "$INSTDIR\oxide.ico"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\dev.oxide.desktop" "UninstallString" '"$INSTDIR\Uninstall Oxide.exe"'
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\dev.oxide.desktop" "QuietUninstallString" '"$INSTDIR\Uninstall Oxide.exe" /S'
SectionEnd

Section "Uninstall"
  Delete "$SMPROGRAMS\Oxide.lnk"
  Delete "$DESKTOP\Oxide.lnk"
  RMDir /r "$INSTDIR"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\dev.oxide.desktop"
  DeleteRegKey HKCU "Software\dev.oxide.desktop"
SectionEnd
