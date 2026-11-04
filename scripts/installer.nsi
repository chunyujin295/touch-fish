Unicode true

; TouchFish 安装包脚本（NSIS 3）。
; 生成：dist\TouchFish-<版本>-setup.exe
; 安装到 %LOCALAPPDATA%\Programs\TouchFish，不需要管理员权限；
; 卸载信息写在 HKCU，控制面板「应用和功能」里能看到。

!include "MUI2.nsh"


!ifndef VERSION
  !define VERSION "0.0.0"
!endif

!define APPNAME "TouchFish"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\TouchFish"

; 路径都相对于本脚本所在目录（scripts\），图标也放在这里

Name "${APPNAME}"
OutFile "..\dist\${APPNAME}-${VERSION}-setup.exe"
InstallDir "$LOCALAPPDATA\Programs\${APPNAME}"
InstallDirRegKey HKCU "Software\${APPNAME}" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma

!define MUI_ICON "..\assets\app\icon.ico"
!define MUI_UNICON "..\assets\app\icon.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\touchfish.exe"
!define MUI_FINISHPAGE_RUN_TEXT "立即启动 TouchFish"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"

Function .onInit
  ; 正在运行的实例会占住 exe，先请它退出（阅读进度是实时落盘的，不会丢）
  nsExec::Exec 'taskkill /IM touchfish.exe /F'
  Pop $0
FunctionEnd

Section "主程序（必需）" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"
  File "..\target\release\touchfish.exe"
  File "..\README.md"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  CreateDirectory "$SMPROGRAMS\${APPNAME}"
  CreateShortcut "$SMPROGRAMS\${APPNAME}\${APPNAME}.lnk" "$INSTDIR\touchfish.exe"
  CreateShortcut "$SMPROGRAMS\${APPNAME}\卸载 ${APPNAME}.lnk" "$INSTDIR\uninstall.exe"
  CreateShortcut "$DESKTOP\${APPNAME}.lnk" "$INSTDIR\touchfish.exe"

  WriteRegStr HKCU "Software\${APPNAME}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayName" "${APPNAME}"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayIcon" "$INSTDIR\touchfish.exe"
  WriteRegStr HKCU "${UNINSTKEY}" "Publisher" "chunyujin295"
  WriteRegStr HKCU "${UNINSTKEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTKEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINSTKEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoRepair" 1
  WriteRegDWORD HKCU "${UNINSTKEY}" "EstimatedSize" 3000
SectionEnd

Section "Uninstall"
  nsExec::Exec 'taskkill /IM touchfish.exe /F'
  Pop $0

  Delete "$INSTDIR\touchfish.exe"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\${APPNAME}\${APPNAME}.lnk"
  Delete "$SMPROGRAMS\${APPNAME}\卸载 ${APPNAME}.lnk"
  RMDir "$SMPROGRAMS\${APPNAME}"
  Delete "$DESKTOP\${APPNAME}.lnk"

  DeleteRegKey HKCU "${UNINSTKEY}"
  DeleteRegKey HKCU "Software\${APPNAME}"

  ; 数据目录单独问一句：卸载重装的人通常想留着书库。
  ; 静默卸载（/S）不弹窗，默认保留数据。
  IfSilent un_done
  IfFileExists "$APPDATA\TouchFish\*.*" 0 un_done
  MessageBox MB_YESNO|MB_ICONQUESTION "是否连同书库、阅读进度和设置一起删除？$\r$\n$\r$\n位置：$APPDATA\TouchFish$\r$\n选择「否」则保留，下次安装后可直接接着读。" IDNO un_done
  RMDir /r "$APPDATA\TouchFish"
  un_done:
SectionEnd
