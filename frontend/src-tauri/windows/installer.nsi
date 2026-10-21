Unicode true
ManifestDPIAware true
; Add in `dpiAwareness` `PerMonitorV2` to manifest for Windows 10 1607+ (note this should not affect lower versions since they should be able to ignore this and pick up `dpiAware` `true` set by `ManifestDPIAware true`)
; Currently undocumented on NSIS's website but is in the Docs folder of source tree, see
; https://github.com/kichik/nsis/blob/5fc0b87b819a9eec006df4967d08e522ddd651c9/Docs/src/attributes.but#L286-L300
; https://github.com/tauri-apps/tauri/pull/10106
ManifestDPIAwareness PerMonitorV2

!if "{{compression}}" == "none"
  SetCompress off
!else
  ; Set the compression algorithm. We default to LZMA.
  SetCompressor /SOLID "{{compression}}"
!endif

; Keep above !include to stay ahead of any plugin command
; see https://github.com/tauri-apps/tauri/pull/15422#discussion_r3289239624
{{#if signed_plugins_path}}
!addplugindir "{{signed_plugins_path}}"
{{/if}}

!include MUI2.nsh
!include FileFunc.nsh
!include x64.nsh
!include WordFunc.nsh
!include "utils.nsh"
!include "FileAssociation.nsh"
!include "Win\COM.nsh"
!include "Win\Propkey.nsh"
!include "Win\RestartManager.nsh"
!include "StrFunc.nsh"
${StrCase}
${StrLoc}

{{#if installer_hooks}}
!include "{{installer_hooks}}"
{{/if}}

!define WEBVIEW2APPGUID "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"

!define MANUFACTURER "{{manufacturer}}"
!define PRODUCTNAME "{{product_name}}"
!define VERSION "{{version}}"
!define VERSIONWITHBUILD "{{version_with_build}}"
!define HOMEPAGE "{{homepage}}"
!define INSTALLMODE "{{install_mode}}"
!define LICENSE "{{license}}"
!define INSTALLERICON "{{installer_icon}}"
!define SIDEBARIMAGE "{{sidebar_image}}"
!define HEADERIMAGE "{{header_image}}"
!define UNINSTALLERICON "{{uninstaller_icon}}"
!define UNINSTALLERHEADERIMAGE "{{uninstaller_header_image}}"
!define MAINBINARYNAME "{{main_binary_name}}"
!define MAINBINARYSRCPATH "{{main_binary_path}}"
!define BUNDLEID "{{bundle_id}}"
!define COPYRIGHT "{{copyright}}"
!define OUTFILE "{{out_file}}"
!define ARCH "{{arch}}"
!define ADDITIONALPLUGINSPATH "{{additional_plugins_path}}"
!define ALLOWDOWNGRADES "{{allow_downgrades}}"
!define DISPLAYLANGUAGESELECTOR "{{display_language_selector}}"
!define INSTALLWEBVIEW2MODE "{{install_webview2_mode}}"
!define WEBVIEW2INSTALLERARGS "{{webview2_installer_args}}"
!define WEBVIEW2BOOTSTRAPPERPATH "{{webview2_bootstrapper_path}}"
!define WEBVIEW2INSTALLERPATH "{{webview2_installer_path}}"
!define MINIMUMWEBVIEW2VERSION "{{minimum_webview2_version}}"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME}"
!define MANUKEY "Software\${MANUFACTURER}"
!define MANUPRODUCTKEY "${MANUKEY}\${PRODUCTNAME}"
!define UNINSTALLERSIGNCOMMAND "{{uninstaller_sign_cmd}}"
!define ESTIMATEDSIZE "{{estimated_size}}"
!define STARTMENUFOLDER "{{start_menu_folder}}"

Var PassiveMode
Var UpdateMode
Var NoShortcutMode
Var WixMode
Var OldMainBinaryName

; Каталоги данных лаунчера на Windows (см. state.rs / config.py):
;   %APPDATA%\nivora-launcher            — настройки, аккаунты
;   %LOCALAPPDATA%\nivora-launcher       — экземпляры, миры, Java, логи, бэкенд
;   %LOCALAPPDATA%\Cache\nivora-launcher — кэш загрузок
;   trial-launcher                       — то же у старых сборок
;   %APPDATA%|%LOCALAPPDATA%\${BUNDLEID} — данные WebView2
!define NIVORA_DATA_DIR "nivora-launcher"
!define NIVORA_LEGACY_DATA_DIR "trial-launcher"
!define NIVORA_BACKEND_IMAGE "nivora-launcher-backend*"

!macro NivoraStopBackend
  nsExec::Exec 'taskkill /F /T /IM ${NIVORA_BACKEND_IMAGE}'
  Pop $0
!macroend

!macro NivoraPurgeData
  DetailPrint "$(nvPurging)"
  !insertmacro NivoraStopBackend
  SetShellVarContext current
  RMDir /r /REBOOTOK "$APPDATA\${NIVORA_DATA_DIR}"
  RMDir /r /REBOOTOK "$LOCALAPPDATA\${NIVORA_DATA_DIR}"
  RMDir /r /REBOOTOK "$LOCALAPPDATA\Cache\${NIVORA_DATA_DIR}"
  RMDir /r /REBOOTOK "$APPDATA\${NIVORA_LEGACY_DATA_DIR}"
  RMDir /r /REBOOTOK "$LOCALAPPDATA\${NIVORA_LEGACY_DATA_DIR}"
  RMDir /r /REBOOTOK "$LOCALAPPDATA\Cache\${NIVORA_LEGACY_DATA_DIR}"
  RMDir /r /REBOOTOK "$APPDATA\${BUNDLEID}"
  RMDir /r /REBOOTOK "$LOCALAPPDATA\${BUNDLEID}"
  RMDir /r "$TEMP\nivora-launcher-runtime"
  Delete "$TEMP\nivora-launcher-*.cancel"

  ; Токены Microsoft лежат в «Диспетчере учётных данных» Windows
  ; (службы trial-launcher-microsoft / nivora-launcher-microsoft).
  ; Скрипт пишется во временный файл: строка NSIS ограничена 1024 символами.
  InitPluginsDir
  FileOpen $0 "$PLUGINSDIR\nivora-credentials.ps1" w
  FileWrite $0 `$$ErrorActionPreference = 'SilentlyContinue'$\r$\n`
  FileWrite $0 `$$targets = cmdkey /list | ForEach-Object { if ($$_ -match '^\s*[^:]+:\s*(\S.*)$$') { $$Matches[1].Trim() } }$\r$\n`
  FileWrite $0 `$$targets = $$targets | Where-Object { $$_ -match '(trial|nivora)-launcher-microsoft' }$\r$\n`
  FileWrite $0 `foreach ($$target in $$targets) {$\r$\n`
  FileWrite $0 `  cmdkey "/delete:$$target" | Out-Null$\r$\n`
  FileWrite $0 `  $$short = $$target -replace '^LegacyGeneric:target=', ''$\r$\n`
  FileWrite $0 `  if ($$short -ne $$target) { cmdkey "/delete:$$short" | Out-Null }$\r$\n`
  FileWrite $0 `}$\r$\n`
  FileClose $0
  nsExec::Exec 'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\nivora-credentials.ps1"'
  Pop $0
  Delete "$PLUGINSDIR\nivora-credentials.ps1"

  DeleteRegValue HKCU "${MANUPRODUCTKEY}" "Installer Language"
  DeleteRegKey HKCU "${MANUPRODUCTKEY}"
  DeleteRegKey /ifempty HKCU "${MANUKEY}"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${PRODUCTNAME}"
  !insertmacro SetContext
!macroend

Name "${PRODUCTNAME}"
BrandingText "${COPYRIGHT}"
OutFile "${OUTFILE}"

; We don't actually use this value as default install path,
; it's just for nsis to append the product name folder in the directory selector
; https://nsis.sourceforge.io/Reference/InstallDir
!define PLACEHOLDER_INSTALL_DIR "placeholder\${PRODUCTNAME}"
InstallDir "${PLACEHOLDER_INSTALL_DIR}"

VIProductVersion "${VERSIONWITHBUILD}"
VIAddVersionKey "ProductName" "${PRODUCTNAME}"
VIAddVersionKey "FileDescription" "${PRODUCTNAME}"
VIAddVersionKey "LegalCopyright" "${COPYRIGHT}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"

# additional plugins
!addplugindir "${ADDITIONALPLUGINSPATH}"

; Uninstaller signing command
!if "${UNINSTALLERSIGNCOMMAND}" != ""
  !uninstfinalize '${UNINSTALLERSIGNCOMMAND}'
!endif

; Handle install mode, `perUser`, `perMachine` or `both`
!if "${INSTALLMODE}" == "perMachine"
  RequestExecutionLevel admin
!endif

!if "${INSTALLMODE}" == "currentUser"
  RequestExecutionLevel user
!endif

!if "${INSTALLMODE}" == "both"
  !define MULTIUSER_MUI
  !define MULTIUSER_INSTALLMODE_INSTDIR "${PRODUCTNAME}"
  !define MULTIUSER_INSTALLMODE_COMMANDLINE
  !if "${ARCH}" == "x64"
    !define MULTIUSER_USE_PROGRAMFILES64
  !else if "${ARCH}" == "arm64"
    !define MULTIUSER_USE_PROGRAMFILES64
  !endif
  !define MULTIUSER_INSTALLMODE_DEFAULT_REGISTRY_KEY "${UNINSTKEY}"
  !define MULTIUSER_INSTALLMODE_DEFAULT_REGISTRY_VALUENAME "CurrentUser"
  !define MULTIUSER_INSTALLMODEPAGE_SHOWUSERNAME
  !define MULTIUSER_INSTALLMODE_FUNCTION RestorePreviousInstallLocation
  !define MULTIUSER_EXECUTIONLEVEL Highest
  !include MultiUser.nsh
!endif

; Installer icon
!if "${INSTALLERICON}" != ""
  !define MUI_ICON "${INSTALLERICON}"
!endif

; Installer sidebar image
!if "${SIDEBARIMAGE}" != ""
  !define MUI_WELCOMEFINISHPAGE_BITMAP "${SIDEBARIMAGE}"
!endif

; Enable header images for installer and uninstaller pages when either image is configured.
!if "${HEADERIMAGE}" != ""
  !define MUI_HEADERIMAGE
!else if "${UNINSTALLERHEADERIMAGE}" != ""
  !define MUI_HEADERIMAGE
!endif

; Installer header image
!if "${HEADERIMAGE}" != ""
  !define MUI_HEADERIMAGE_BITMAP "${HEADERIMAGE}"
!endif

; Uninstaller header image
!if "${UNINSTALLERHEADERIMAGE}" != ""
  !define MUI_HEADERIMAGE_UNBITMAP "${UNINSTALLERHEADERIMAGE}"
!endif

; Uninstaller icon
!if "${UNINSTALLERICON}" != ""
  !define MUI_UNICON "${UNINSTALLERICON}"
!endif

; Define registry key to store installer language
!define MUI_LANGDLL_REGISTRY_ROOT "HKCU"
!define MUI_LANGDLL_REGISTRY_KEY "${MANUPRODUCTKEY}"
!define MUI_LANGDLL_REGISTRY_VALUENAME "Installer Language"

; Installer pages, must be ordered as they appear
; 1. Welcome Page
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
!insertmacro MUI_PAGE_WELCOME

; 2. License Page (if defined)
!if "${LICENSE}" != ""
  !define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
  !insertmacro MUI_PAGE_LICENSE "${LICENSE}"
!endif

; 3. Install mode (if it is set to `both`)
!if "${INSTALLMODE}" == "both"
  !define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
  !insertmacro MULTIUSER_PAGE_INSTALLMODE
!endif

;; ---------------------------------------------------------------------------
;; Nivora Launcher: собственный шаблон установщика (на основе шаблона Tauri 2).
;; Отличия от стандартного:
;;  * если лаунчер уже установлен — страница «Обслуживание»: обновить
;;    (просто заменить файлы программы), удалить (папка лаунчера с
;;    экземплярами остаётся) или удалить полностью (вместе с папкой лаунчера);
;;  * деинсталлятор спрашивает, удалять ли папку лаунчера, и при полном
;;    удалении действительно чистит все каталоги лаунчера (а не только
;;    %APPDATA%\${BUNDLEID}, как стандартный шаблон);
;;  * перед заменой файлов останавливается Python-бэкенд лаунчера, который
;;    иначе держал свой exe и ломал обновление.
;; Ключи командной строки Tauri (/UPDATE, /P, /S, /NS, /R, /ARGS) работают
;; как раньше — на них опирается встроенное самообновление.
;; Деинсталлятор дополнительно понимает /MODE=keep и /MODE=full (/FULL).
;; ---------------------------------------------------------------------------

; 4. Страница «Обслуживание» — только если лаунчер уже установлен
Var MaintenanceAction ; "", "update", "remove", "purge"
Var InstalledVersion
Var VersionRelation   ; 1 — ставим новее, 0 — та же, -1 — старее
Var MaintUpdateRadio
Var MaintRemoveRadio
Var MaintPurgeRadio
Page custom PageMaintenance PageLeaveMaintenance
Function PageMaintenance
  StrCpy $MaintenanceAction ""
  ; Старые MSI-установки (WiX) ищутся так же, как в стандартном шаблоне.
  StrCpy $0 0
  wix_loop:
    EnumRegKey $1 HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall" $0
    StrCmp $1 "" wix_loop_done
    IntOp $0 $0 + 1
    ReadRegStr $R0 HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\$1" "DisplayName"
    ReadRegStr $R1 HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\$1" "Publisher"
    StrCmp "$R0$R1" "${PRODUCTNAME}${MANUFACTURER}" 0 wix_loop
    ReadRegStr $R0 HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\$1" "UninstallString"
    ${StrCase} $R1 $R0 "L"
    ${StrLoc} $R0 $R1 "msiexec" ">"
    StrCmp $R0 0 0 wix_loop_done
    StrCpy $WixMode 1
    StrCpy $R6 "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\$1"
    Goto read_version
  wix_loop_done:

  ReadRegStr $R0 SHCTX "${UNINSTKEY}" ""
  ReadRegStr $R1 SHCTX "${UNINSTKEY}" "UninstallString"
  ${IfThen} "$R0$R1" == "" ${|} Abort ${|}

  read_version:
  ${If} $WixMode = 1
    ReadRegStr $InstalledVersion HKLM "$R6" "DisplayVersion"
  ${Else}
    ReadRegStr $InstalledVersion SHCTX "${UNINSTKEY}" "DisplayVersion"
  ${EndIf}
  ${If} $InstalledVersion == ""
    StrCpy $InstalledVersion "$(nvUnknownVersion)"
    StrCpy $VersionRelation 1
  ${Else}
    nsis_tauri_utils::SemverCompare "${VERSION}" $InstalledVersion
    Pop $VersionRelation
  ${EndIf}

  ; Самообновление (/UPDATE) и пассивный режим (/P) вопросов не задают:
  ; файлы программы просто заменяются.
  ${If} $UpdateMode = 1
  ${OrIf} $PassiveMode = 1
    StrCpy $MaintenanceAction "update"
    Call PageLeaveMaintenance
    Abort
  ${EndIf}

  ${If} $VersionRelation = 1
    !insertmacro MUI_HEADER_TEXT "$(nvMaintTitleUpdate)" "$(nvMaintSubtitle)"
    StrCpy $R2 "$(nvMaintUpdate)"
    StrCpy $R3 "$(nvMaintUpdateHint)"
  ${ElseIf} $VersionRelation = 0
    !insertmacro MUI_HEADER_TEXT "$(nvMaintTitleSame)" "$(nvMaintSubtitle)"
    StrCpy $R2 "$(nvMaintReinstall)"
    StrCpy $R3 "$(nvMaintUpdateHint)"
  ${Else}
    !insertmacro MUI_HEADER_TEXT "$(nvMaintTitleNewer)" "$(nvMaintSubtitle)"
    StrCpy $R2 "$(nvMaintDowngrade)"
    StrCpy $R3 "$(nvMaintDowngradeHint)"
  ${EndIf}

  nsDialogs::Create 1018
  Pop $0
  ${IfThen} $(^RTL) = 1 ${|} nsDialogs::SetRTL $(^RTL) ${|}

  ${NSD_CreateLabel} 0 0 100% 20u "$(nvMaintIntro)"
  Pop $0

  ${NSD_CreateRadioButton} 0 24u 100% 10u $R2
  Pop $MaintUpdateRadio
  ${NSD_CreateLabel} 12u 35u -12u 18u $R3
  Pop $0

  ${NSD_CreateRadioButton} 0 57u 100% 10u "$(nvMaintRemove)"
  Pop $MaintRemoveRadio
  ${NSD_CreateLabel} 12u 68u -12u 18u "$(nvMaintRemoveHint)"
  Pop $0

  ${NSD_CreateRadioButton} 0 90u 100% 10u "$(nvMaintPurge)"
  Pop $MaintPurgeRadio
  ${NSD_CreateLabel} 12u 101u -12u 26u "$(nvMaintPurgeHint)"
  Pop $0

  ; Старую установку через MSI нельзя обновить поверх — только удалить.
  ${If} $WixMode = 1
    EnableWindow $MaintUpdateRadio 0
    ${NSD_Check} $MaintRemoveRadio
  ${ElseIf} $VersionRelation = -1
    !if "${ALLOWDOWNGRADES}" == "false"
      EnableWindow $MaintUpdateRadio 0
      ${NSD_Check} $MaintRemoveRadio
    !else
      ${NSD_Check} $MaintUpdateRadio
    !endif
  ${Else}
    ${NSD_Check} $MaintUpdateRadio
  ${EndIf}

  nsDialogs::Show
FunctionEnd

Function PageLeaveMaintenance
  ${If} $MaintenanceAction != "update"
    ${NSD_GetState} $MaintPurgeRadio $0
    ${NSD_GetState} $MaintRemoveRadio $1
    ${If} $0 = ${BST_CHECKED}
      StrCpy $MaintenanceAction "purge"
    ${ElseIf} $1 = ${BST_CHECKED}
      StrCpy $MaintenanceAction "remove"
    ${Else}
      StrCpy $MaintenanceAction "update"
    ${EndIf}
  ${EndIf}

  ${If} $MaintenanceAction == "purge"
    MessageBox MB_YESNO|MB_ICONEXCLAMATION|MB_DEFBUTTON2 "$(nvPurgeConfirm)" IDYES purge_confirmed
    StrCpy $MaintenanceAction ""
    Abort
    purge_confirmed:
  ${EndIf}

  ; Обновление: файлы программы заменяются на месте, ярлыки и данные
  ; не трогаются. Старую MSI-установку сначала удаляем её же деинсталлятором.
  ${If} $MaintenanceAction == "update"
    ${If} $WixMode = 1
      HideWindow
      ReadRegStr $R1 HKLM "$R6" "UninstallString"
      ExecWait '$R1' $0
      BringToFront
      ${If} $0 <> 0
        MessageBox MB_ICONEXCLAMATION "$(unableToUninstall)"
        Abort
      ${EndIf}
    ${Else}
      StrCpy $UpdateMode 1
    ${EndIf}
    Return
  ${EndIf}

  ; Удаление (с папкой лаунчера или без) — из самого установщика.
  HideWindow
  ClearErrors
  ${If} $WixMode = 1
    ReadRegStr $R1 HKLM "$R6" "UninstallString"
    ExecWait '$R1' $0
  ${Else}
    ReadRegStr $4 SHCTX "${MANUPRODUCTKEY}" ""
    ${If} $4 == ""
      StrCpy $4 $INSTDIR
    ${EndIf}
    ReadRegStr $R1 SHCTX "${UNINSTKEY}" "UninstallString"
    ${If} $MaintenanceAction == "purge"
      StrCpy $R1 "$R1 /P /MODE=full"
    ${Else}
      StrCpy $R1 "$R1 /P /MODE=keep"
    ${EndIf}
    ExecWait '$R1 _?=$4' $0
  ${EndIf}
  ${IfThen} ${Errors} ${|} StrCpy $0 2 ${|}
  BringToFront

  ${If} $0 <> 0
    ${If} $0 = 1
    ${OrIf} $0 = 1602
      ; пользователь отменил удаление — назад на страницу выбора
      StrCpy $MaintenanceAction ""
      Abort
    ${EndIf}
    MessageBox MB_ICONEXCLAMATION "$(unableToUninstall)"
    StrCpy $MaintenanceAction ""
    Abort
  ${EndIf}

  ; Запущенный с _?= деинсталлятор не может удалить сам себя.
  ${If} $WixMode <> 1
    Delete "$4\uninstall.exe"
    RMDir "$4"
  ${EndIf}

  ; Полное удаление выполняет и сам установщик: деинсталляторы старых
  ; версий не знают про /MODE=full и папку лаунчера оставили бы.
  ${If} $MaintenanceAction == "purge"
    !insertmacro NivoraPurgeData
    MessageBox MB_ICONINFORMATION "$(nvPurgeDone)"
  ${Else}
    MessageBox MB_ICONINFORMATION "$(nvRemoveDone)"
  ${EndIf}
  Quit
FunctionEnd

; 5. Choose install directory page (при обновлении — папка прежней установки)
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassiveOrUpdate
!insertmacro MUI_PAGE_DIRECTORY

; 6. Start menu shortcut page
Var AppStartMenuFolder
!if "${STARTMENUFOLDER}" != ""
  !define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassiveOrUpdate
  !define MUI_STARTMENUPAGE_DEFAULTFOLDER "${STARTMENUFOLDER}"
!else
  !define MUI_PAGE_CUSTOMFUNCTION_PRE Skip
!endif
!insertmacro MUI_PAGE_STARTMENU Application $AppStartMenuFolder

; 7. Installation page
!insertmacro MUI_PAGE_INSTFILES

; 8. Finish page
;
; Don't auto jump to finish page after installation page,
; because the installation page has useful info that can be used debug any issues with the installer.
!define MUI_FINISHPAGE_NOAUTOCLOSE
; Use show readme button in the finish page as a button create a desktop shortcut
!define MUI_FINISHPAGE_SHOWREADME
!define MUI_FINISHPAGE_SHOWREADME_TEXT "$(createDesktop)"
!define MUI_FINISHPAGE_SHOWREADME_FUNCTION CreateOrUpdateDesktopShortcut
; Show run app after installation.
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_FUNCTION RunMainBinary
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
!insertmacro MUI_PAGE_FINISH

Function RunMainBinary
  nsis_tauri_utils::RunAsUser "$INSTDIR\${MAINBINARYNAME}.exe" ""
FunctionEnd

; Uninstaller Pages
; 1. Выбор: удалить только программу или вместе с папкой лаунчера
Var UninstallMode      ; "keep" | "full"
Var UninstallModeFixed ; 1 — режим задан ключом /MODE=…, страницу не показываем
Var UnKeepRadio
Var UnFullRadio
UninstPage custom un.PageUninstallMode un.PageLeaveUninstallMode
Function un.PageUninstallMode
  ${If} $PassiveMode = 1
  ${OrIf} $UpdateMode = 1
  ${OrIf} $UninstallModeFixed = 1
    Abort
  ${EndIf}
  !insertmacro MUI_HEADER_TEXT "$(nvUnTitle)" "$(nvUnSubtitle)"
  nsDialogs::Create 1018
  Pop $0
  ${IfThen} $(^RTL) = 1 ${|} nsDialogs::SetRTL $(^RTL) ${|}

  ${NSD_CreateRadioButton} 0 0 100% 10u "$(nvMaintRemove)"
  Pop $UnKeepRadio
  ${NSD_CreateLabel} 12u 11u -12u 18u "$(nvMaintRemoveHint)"
  Pop $0

  ${NSD_CreateRadioButton} 0 36u 100% 10u "$(nvMaintPurge)"
  Pop $UnFullRadio
  ${NSD_CreateLabel} 12u 47u -12u 34u "$(nvMaintPurgeHint)"
  Pop $0

  ${If} $UninstallMode == "full"
    ${NSD_Check} $UnFullRadio
  ${Else}
    ${NSD_Check} $UnKeepRadio
  ${EndIf}
  nsDialogs::Show
FunctionEnd
Function un.PageLeaveUninstallMode
  ${NSD_GetState} $UnFullRadio $0
  ${If} $0 = ${BST_CHECKED}
    MessageBox MB_YESNO|MB_ICONEXCLAMATION|MB_DEFBUTTON2 "$(nvPurgeConfirm)" IDYES un_purge_confirmed
    Abort
    un_purge_confirmed:
    StrCpy $UninstallMode "full"
  ${Else}
    StrCpy $UninstallMode "keep"
  ${EndIf}
FunctionEnd

; 2. Uninstalling Page
!insertmacro MUI_UNPAGE_INSTFILES

;Languages
{{#each languages}}
!insertmacro MUI_LANGUAGE "{{this}}"
{{/each}}
!insertmacro MUI_RESERVEFILE_LANGDLL
{{#each language_files}}
  !include "{{this}}"
{{/each}}

; Тексты страниц Nivora (русский и английский)
!macro NIVORA_STRING NAME RU EN
  !ifdef LANG_RUSSIAN
    LangString ${NAME} ${LANG_RUSSIAN} "${RU}"
  !endif
  !ifdef LANG_ENGLISH
    LangString ${NAME} ${LANG_ENGLISH} "${EN}"
  !endif
!macroend
!insertmacro NIVORA_STRING nvUnknownVersion "неизвестной версии" "unknown version"
!insertmacro NIVORA_STRING nvMaintTitleUpdate "Найдена старая версия ${PRODUCTNAME}" "An older ${PRODUCTNAME} is installed"
!insertmacro NIVORA_STRING nvMaintTitleSame "${PRODUCTNAME} ${VERSION} уже установлен" "${PRODUCTNAME} ${VERSION} is already installed"
!insertmacro NIVORA_STRING nvMaintTitleNewer "Установлена более новая версия ${PRODUCTNAME}" "A newer ${PRODUCTNAME} is installed"
!insertmacro NIVORA_STRING nvMaintSubtitle "Выберите, что сделать с установленным лаунчером." "Choose what to do with the installed launcher."
!insertmacro NIVORA_STRING nvMaintIntro "Установлена версия $InstalledVersion. Этот установщик содержит версию ${VERSION}." "Installed version: $InstalledVersion. This installer contains version ${VERSION}."
!insertmacro NIVORA_STRING nvMaintUpdate "Обновить до ${VERSION} (рекомендуется)" "Update to ${VERSION} (recommended)"
!insertmacro NIVORA_STRING nvMaintReinstall "Переустановить ${VERSION}" "Reinstall ${VERSION}"
!insertmacro NIVORA_STRING nvMaintDowngrade "Установить ${VERSION} поверх более новой версии" "Install ${VERSION} over the newer version"
!insertmacro NIVORA_STRING nvMaintUpdateHint "Заменяются только файлы программы. Экземпляры, миры, аккаунты, настройки и ярлыки остаются." "Only the program files are replaced. Instances, worlds, accounts, settings and shortcuts are kept."
!insertmacro NIVORA_STRING nvMaintDowngradeHint "Не рекомендуется: более старая версия может не понять данные новой." "Not recommended: an older version may not understand newer data."
!insertmacro NIVORA_STRING nvMaintRemove "Удалить лаунчер" "Uninstall the launcher"
!insertmacro NIVORA_STRING nvMaintRemoveHint "Удаляется только программа. Папка лаунчера (экземпляры, миры, настройки, аккаунты) остаётся." "Only the program is removed. The launcher folder (instances, worlds, settings, accounts) is kept."
!insertmacro NIVORA_STRING nvMaintPurge "Полностью удалить лаунчер" "Completely remove the launcher"
!insertmacro NIVORA_STRING nvMaintPurgeHint "Удаляется всё: программа и папка лаунчера со всеми экземплярами, мирами, скриншотами, Java и кэшем, настройки, темы и сохранённые входы Microsoft. Отменить нельзя." "Everything is removed: the program and the launcher folder with all instances, worlds, screenshots, Java and cache, settings, themes and saved Microsoft sign-ins. This cannot be undone."
!insertmacro NIVORA_STRING nvPurgeConfirm "Удалить папку лаунчера со ВСЕМИ экземплярами и мирами без возможности восстановления?" "Delete the launcher folder with ALL instances and worlds permanently?"
!insertmacro NIVORA_STRING nvPurging "Удаление папки лаунчера и всех данных..." "Removing the launcher folder and all data..."
!insertmacro NIVORA_STRING nvPurgeDone "${PRODUCTNAME} и все его данные удалены." "${PRODUCTNAME} and all of its data have been removed."
!insertmacro NIVORA_STRING nvRemoveDone "${PRODUCTNAME} удалён. Папка лаунчера с экземплярами сохранена." "${PRODUCTNAME} has been uninstalled. The launcher folder with your instances was kept."
!insertmacro NIVORA_STRING nvUnTitle "Удаление ${PRODUCTNAME}" "Uninstall ${PRODUCTNAME}"
!insertmacro NIVORA_STRING nvUnSubtitle "Выберите, удалять ли папку лаунчера." "Choose whether to delete the launcher folder."

Function .onInit
  ${GetOptions} $CMDLINE "/P" $PassiveMode
  ${IfNot} ${Errors}
    StrCpy $PassiveMode 1
  ${EndIf}

  ${GetOptions} $CMDLINE "/NS" $NoShortcutMode
  ${IfNot} ${Errors}
    StrCpy $NoShortcutMode 1
  ${EndIf}

  ${GetOptions} $CMDLINE "/UPDATE" $UpdateMode
  ${IfNot} ${Errors}
    StrCpy $UpdateMode 1
  ${EndIf}

  !if "${DISPLAYLANGUAGESELECTOR}" == "true"
    !insertmacro MUI_LANGDLL_DISPLAY
  !endif

  !insertmacro SetContext

  ${If} $INSTDIR == "${PLACEHOLDER_INSTALL_DIR}"
    ; Set default install location
    !if "${INSTALLMODE}" == "perMachine"
      ${If} ${RunningX64}
        !if "${ARCH}" == "x64"
          StrCpy $INSTDIR "$PROGRAMFILES64\${PRODUCTNAME}"
        !else if "${ARCH}" == "arm64"
          StrCpy $INSTDIR "$PROGRAMFILES64\${PRODUCTNAME}"
        !else
          StrCpy $INSTDIR "$PROGRAMFILES\${PRODUCTNAME}"
        !endif
      ${Else}
        StrCpy $INSTDIR "$PROGRAMFILES\${PRODUCTNAME}"
      ${EndIf}
    !else if "${INSTALLMODE}" == "currentUser"
      StrCpy $INSTDIR "$LOCALAPPDATA\${PRODUCTNAME}"
    !endif

    Call RestorePreviousInstallLocation
  ${EndIf}


  !if "${INSTALLMODE}" == "both"
    !insertmacro MULTIUSER_INIT
  !endif
FunctionEnd


Section EarlyChecks
  ; Abort silent installer if downgrades is disabled
  !if "${ALLOWDOWNGRADES}" == "false"
  ${If} ${Silent}
    ; If downgrading
    ${If} $R0 = -1
      System::Call 'kernel32::AttachConsole(i -1)i.r0'
      ${If} $0 <> 0
        System::Call 'kernel32::GetStdHandle(i -11)i.r0'
        System::call 'kernel32::SetConsoleTextAttribute(i r0, i 0x0004)' ; set red color
        FileWrite $0 "$(silentDowngrades)"
      ${EndIf}
      Abort
    ${EndIf}
  ${EndIf}
  !endif

SectionEnd

Section WebView2
  ; Check if Webview2 is already installed and skip this section
  ${If} ${RunningX64}
    ReadRegStr $4 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\${WEBVIEW2APPGUID}" "pv"
  ${Else}
    ReadRegStr $4 HKLM "SOFTWARE\Microsoft\EdgeUpdate\Clients\${WEBVIEW2APPGUID}" "pv"
  ${EndIf}
  ${If} $4 == ""
    ReadRegStr $4 HKCU "SOFTWARE\Microsoft\EdgeUpdate\Clients\${WEBVIEW2APPGUID}" "pv"
  ${EndIf}

  ${If} $4 == ""
    ; Webview2 installation
    ;
    ; Skip if updating
    ${If} $UpdateMode <> 1
      !if "${INSTALLWEBVIEW2MODE}" == "downloadBootstrapper"
        Delete "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        DetailPrint "$(webview2Downloading)"
        NSISdl::download "https://go.microsoft.com/fwlink/p/?LinkId=2124703" "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        Pop $0
        ${If} $0 == "success"
          DetailPrint "$(webview2DownloadSuccess)"
        ${Else}
          DetailPrint "$(webview2DownloadError)"
          Abort "$(webview2AbortError)"
        ${EndIf}
        StrCpy $6 "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        Goto install_webview2
      !endif

      !if "${INSTALLWEBVIEW2MODE}" == "embedBootstrapper"
        Delete "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        File "/oname=$TEMP\MicrosoftEdgeWebview2Setup.exe" "${WEBVIEW2BOOTSTRAPPERPATH}"
        DetailPrint "$(installingWebview2)"
        StrCpy $6 "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        Goto install_webview2
      !endif

      !if "${INSTALLWEBVIEW2MODE}" == "offlineInstaller"
        Delete "$TEMP\MicrosoftEdgeWebView2RuntimeInstaller.exe"
        File "/oname=$TEMP\MicrosoftEdgeWebView2RuntimeInstaller.exe" "${WEBVIEW2INSTALLERPATH}"
        DetailPrint "$(installingWebview2)"
        StrCpy $6 "$TEMP\MicrosoftEdgeWebView2RuntimeInstaller.exe"
        Goto install_webview2
      !endif

      Goto webview2_done

      install_webview2:
        DetailPrint "$(installingWebview2)"
        ; $6 holds the path to the webview2 installer
        ExecWait "$6 ${WEBVIEW2INSTALLERARGS} /install" $1
        ${If} $1 = 0
          DetailPrint "$(webview2InstallSuccess)"
        ${Else}
          DetailPrint "$(webview2InstallError)"
          Abort "$(webview2AbortError)"
        ${EndIf}
      webview2_done:
    ${EndIf}
  ${Else}
    !if "${MINIMUMWEBVIEW2VERSION}" != ""
      ${VersionCompare} "${MINIMUMWEBVIEW2VERSION}" "$4" $R0
      ${If} $R0 = 1
        update_webview:
          DetailPrint "$(installingWebview2)"
          ${If} ${RunningX64}
            ReadRegStr $R1 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate" "path"
          ${Else}
            ReadRegStr $R1 HKLM "SOFTWARE\Microsoft\EdgeUpdate" "path"
          ${EndIf}
          ${If} $R1 == ""
            ReadRegStr $R1 HKCU "SOFTWARE\Microsoft\EdgeUpdate" "path"
          ${EndIf}
          ${If} $R1 != ""
            ; Chromium updater docs: https://source.chromium.org/chromium/chromium/src/+/main:docs/updater/user_manual.md
            ; Modified from "HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Microsoft EdgeWebView\ModifyPath"
            ExecWait `"$R1" /install appguid=${WEBVIEW2APPGUID}&needsadmin=true` $1
            ${If} $1 = 0
              DetailPrint "$(webview2InstallSuccess)"
            ${Else}
              MessageBox MB_ICONEXCLAMATION|MB_ABORTRETRYIGNORE "$(webview2InstallError)" IDIGNORE ignore IDRETRY update_webview
              Quit
              ignore:
            ${EndIf}
          ${EndIf}
      ${EndIf}
    !endif
  ${EndIf}
SectionEnd

Section Install
  SetOutPath $INSTDIR

  !ifmacrodef NSIS_HOOK_PREINSTALL
    !insertmacro NSIS_HOOK_PREINSTALL
  !endif

  !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
  ; Бэкенд лаунчера — отдельный процесс; живой он держит свой exe, и
  ; замена файлов при обновлении падала с «ошибкой открытия файла».
  !insertmacro NivoraStopBackend

  ; Copy main executable
  File "${MAINBINARYSRCPATH}"

  ; Copy resources
  {{#each resources_dirs}}
    CreateDirectory "$INSTDIR\\{{this}}"
  {{/each}}
  {{#each resources}}
    File /a "/oname={{this.[1]}}" "{{no-escape @key}}"
  {{/each}}

  ; Copy external binaries
  {{#each binaries}}
    File /a "/oname={{this}}" "{{no-escape @key}}"
  {{/each}}

  ; Create file associations
  {{#each file_associations as |association| ~}}
    {{#each association.ext as |ext| ~}}
       !insertmacro APP_ASSOCIATE "{{ext}}" "{{or association.name ext}}" "{{association-description association.description ext}}" "$INSTDIR\${MAINBINARYNAME}.exe,0" "Open with ${PRODUCTNAME}" "$INSTDIR\${MAINBINARYNAME}.exe $\"%1$\""
    {{/each}}
  {{/each}}

  ; Register deep links
  {{#each deep_link_protocols as |protocol| ~}}
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}" "URL Protocol" ""
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}" "" "URL:${BUNDLEID} protocol"
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}\DefaultIcon" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\",0"
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}\shell\open\command" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
  {{/each}}

  ; Create uninstaller
  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; Save $INSTDIR in registry for future installations
  WriteRegStr SHCTX "${MANUPRODUCTKEY}" "" $INSTDIR
  ; Язык установки — деинсталлятор возьмёт его и не будет спрашивать.
  WriteRegStr HKCU "${MANUPRODUCTKEY}" "Installer Language" $LANGUAGE

  !if "${INSTALLMODE}" == "both"
    ; Save install mode to be selected by default for the next installation such as updating
    ; or when uninstalling
    WriteRegStr SHCTX "${UNINSTKEY}" $MultiUser.InstallMode 1
  !endif

  ; Remove old main binary if it doesn't match new main binary name
  ReadRegStr $OldMainBinaryName SHCTX "${UNINSTKEY}" "MainBinaryName"
  ${If} $OldMainBinaryName != ""
  ${AndIf} $OldMainBinaryName != "${MAINBINARYNAME}.exe"
    Delete "$INSTDIR\$OldMainBinaryName"
  ${EndIf}

  ; Save current MAINBINARYNAME for future updates
  WriteRegStr SHCTX "${UNINSTKEY}" "MainBinaryName" "${MAINBINARYNAME}.exe"

  ; Registry information for add/remove programs
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayName" "${PRODUCTNAME}"
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayIcon" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\""
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr SHCTX "${UNINSTKEY}" "Publisher" "${MANUFACTURER}"
  WriteRegStr SHCTX "${UNINSTKEY}" "InstallLocation" "$\"$INSTDIR$\""
  WriteRegStr SHCTX "${UNINSTKEY}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  WriteRegDWORD SHCTX "${UNINSTKEY}" "NoModify" "1"
  WriteRegDWORD SHCTX "${UNINSTKEY}" "NoRepair" "1"

  ${GetSize} "$INSTDIR" "/M=uninstall.exe /S=0K /G=0" $0 $1 $2
  IntOp $0 $0 + ${ESTIMATEDSIZE}
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD SHCTX "${UNINSTKEY}" "EstimatedSize" "$0"

  !if "${HOMEPAGE}" != ""
    WriteRegStr SHCTX "${UNINSTKEY}" "URLInfoAbout" "${HOMEPAGE}"
    WriteRegStr SHCTX "${UNINSTKEY}" "URLUpdateInfo" "${HOMEPAGE}"
    WriteRegStr SHCTX "${UNINSTKEY}" "HelpLink" "${HOMEPAGE}"
  !endif

  ; Create start menu shortcut
  !insertmacro MUI_STARTMENU_WRITE_BEGIN Application
    Call CreateOrUpdateStartMenuShortcut
  !insertmacro MUI_STARTMENU_WRITE_END

  ; Create desktop shortcut for silent and passive installers
  ; because finish page will be skipped
  ${If} $PassiveMode = 1
  ${OrIf} ${Silent}
    Call CreateOrUpdateDesktopShortcut
  ${EndIf}

  !ifmacrodef NSIS_HOOK_POSTINSTALL
    !insertmacro NSIS_HOOK_POSTINSTALL
  !endif

  ; Auto close this page for passive mode
  ${If} $PassiveMode = 1
    SetAutoClose true
  ${EndIf}
SectionEnd

Function .onInstSuccess
  ; Check for `/R` flag only in silent and passive installers because
  ; GUI installer has a toggle for the user to (re)start the app
  ${If} $PassiveMode = 1
  ${OrIf} ${Silent}
    ${GetOptions} $CMDLINE "/R" $R0
    ${IfNot} ${Errors}
      ${GetOptions} $CMDLINE "/ARGS" $R0
      nsis_tauri_utils::RunAsUser "$INSTDIR\${MAINBINARYNAME}.exe" "$R0"
    ${EndIf}
  ${EndIf}
FunctionEnd

Function un.onInit
  !insertmacro SetContext

  !if "${INSTALLMODE}" == "both"
    !insertmacro MULTIUSER_UNINIT
  !endif

  !insertmacro MUI_UNGETLANGUAGE

  ${GetOptions} $CMDLINE "/P" $PassiveMode
  ${IfNot} ${Errors}
    StrCpy $PassiveMode 1
  ${EndIf}

  ${GetOptions} $CMDLINE "/UPDATE" $UpdateMode
  ${IfNot} ${Errors}
    StrCpy $UpdateMode 1
  ${EndIf}

  StrCpy $UninstallMode "keep"
  StrCpy $UninstallModeFixed 0
  ${GetOptions} $CMDLINE "/MODE=" $0
  ${IfNot} ${Errors}
    StrCpy $UninstallModeFixed 1
    ${If} $0 == "full"
      StrCpy $UninstallMode "full"
    ${EndIf}
  ${EndIf}
  ${GetOptions} $CMDLINE "/FULL" $0
  ${IfNot} ${Errors}
    StrCpy $UninstallMode "full"
    StrCpy $UninstallModeFixed 1
  ${EndIf}
FunctionEnd

Section Uninstall

  !ifmacrodef NSIS_HOOK_PREUNINSTALL
    !insertmacro NSIS_HOOK_PREUNINSTALL
  !endif

  !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
  !insertmacro NivoraStopBackend

  ; Delete the app directory and its content from disk
  ; Copy main executable
  Delete "$INSTDIR\${MAINBINARYNAME}.exe"

  ; Delete resources
  {{#each resources}}
    Delete "$INSTDIR\\{{this.[1]}}"
  {{/each}}

  ; Delete external binaries
  {{#each binaries}}
    Delete "$INSTDIR\\{{this}}"
  {{/each}}

  ; Delete app associations
  {{#each file_associations as |association| ~}}
    {{#each association.ext as |ext| ~}}
      !insertmacro APP_UNASSOCIATE "{{ext}}" "{{or association.name ext}}"
    {{/each}}
  {{/each}}

  ; Delete deep links
  {{#each deep_link_protocols as |protocol| ~}}
    ReadRegStr $R7 SHCTX "Software\Classes\\{{protocol}}\shell\open\command" ""
    ${If} $R7 == "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
      DeleteRegKey SHCTX "Software\Classes\\{{protocol}}"
    ${EndIf}
  {{/each}}


  ; Delete uninstaller
  Delete "$INSTDIR\uninstall.exe"

  {{#each resources_ancestors}}
  RMDir /REBOOTOK "$INSTDIR\\{{this}}"
  {{/each}}
  RMDir "$INSTDIR"

  ; Remove shortcuts if not updating
  ${If} $UpdateMode <> 1
    !insertmacro DeleteAppUserModelId

    ; Remove start menu shortcut
    !insertmacro MUI_STARTMENU_GETFOLDER Application $AppStartMenuFolder
    !insertmacro IsShortcutTarget "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${If} $0 = 1
      !insertmacro UnpinShortcut "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk"
      Delete "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk"
      RMDir "$SMPROGRAMS\$AppStartMenuFolder"
    ${EndIf}
    !insertmacro IsShortcutTarget "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${If} $0 = 1
      !insertmacro UnpinShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk"
      Delete "$SMPROGRAMS\${PRODUCTNAME}.lnk"
    ${EndIf}

    ; Remove desktop shortcuts
    !insertmacro IsShortcutTarget "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${If} $0 = 1
      !insertmacro UnpinShortcut "$DESKTOP\${PRODUCTNAME}.lnk"
      Delete "$DESKTOP\${PRODUCTNAME}.lnk"
    ${EndIf}
  ${EndIf}

  ; Remove registry information for add/remove programs
  !if "${INSTALLMODE}" == "both"
    DeleteRegKey SHCTX "${UNINSTKEY}"
  !else if "${INSTALLMODE}" == "perMachine"
    DeleteRegKey HKLM "${UNINSTKEY}"
  !else
    DeleteRegKey HKCU "${UNINSTKEY}"
  !endif

  ; Removes the Autostart entry for ${PRODUCTNAME} from the HKCU Run key if it exists.
  ; This ensures the program does not launch automatically after uninstallation if it exists.
  ; If it doesn't exist, it does nothing.
  ; We do this when not updating (to preserve the registry value on updates)
  ${If} $UpdateMode <> 1
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${PRODUCTNAME}"
  ${EndIf}

  ; Полное удаление: папка лаунчера (экземпляры, миры, Java, кэш),
  ; настройки, данные WebView и сохранённые входы Microsoft.
  ; При обновлении (/UPDATE) данные не трогаются никогда.
  ${If} $UninstallMode == "full"
  ${AndIf} $UpdateMode <> 1
    DeleteRegKey SHCTX "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty SHCTX "${MANUKEY}"
    !insertmacro NivoraPurgeData
  ${EndIf}

  !ifmacrodef NSIS_HOOK_POSTUNINSTALL
    !insertmacro NSIS_HOOK_POSTUNINSTALL
  !endif

  ; Auto close if passive mode or updating
  ${If} $PassiveMode = 1
  ${OrIf} $UpdateMode = 1
    SetAutoClose true
  ${EndIf}
SectionEnd

Function RestorePreviousInstallLocation
  ReadRegStr $4 SHCTX "${MANUPRODUCTKEY}" ""
  StrCmp $4 "" +2 0
    StrCpy $INSTDIR $4
FunctionEnd

Function Skip
  Abort
FunctionEnd

Function SkipIfPassive
  ${IfThen} $PassiveMode = 1  ${|} Abort ${|}
FunctionEnd
Function SkipIfPassiveOrUpdate
  ${IfThen} $PassiveMode = 1  ${|} Abort ${|}
  ${IfThen} $UpdateMode = 1  ${|} Abort ${|}
FunctionEnd

Function CreateOrUpdateStartMenuShortcut
  ; We used to use product name as MAINBINARYNAME
  ; migrate old shortcuts to target the new MAINBINARYNAME
  StrCpy $R0 0

  !insertmacro IsShortcutTarget "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk" "$INSTDIR\$OldMainBinaryName"
  Pop $0
  ${If} $0 = 1
    !insertmacro SetShortcutTarget "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    StrCpy $R0 1
  ${EndIf}

  !insertmacro IsShortcutTarget "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\$OldMainBinaryName"
  Pop $0
  ${If} $0 = 1
    !insertmacro SetShortcutTarget "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    StrCpy $R0 1
  ${EndIf}

  ${If} $R0 = 1
    Return
  ${EndIf}

  ; Skip creating shortcut if in update mode or no shortcut mode
  ; but always create if migrating from wix
  ${If} $WixMode = 0
    ${If} $UpdateMode = 1
    ${OrIf} $NoShortcutMode = 1
      Return
    ${EndIf}
  ${EndIf}

  !if "${STARTMENUFOLDER}" != ""
    CreateDirectory "$SMPROGRAMS\$AppStartMenuFolder"
    CreateShortcut "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk"
  !else
    CreateShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\${PRODUCTNAME}.lnk"
  !endif
FunctionEnd

Function CreateOrUpdateDesktopShortcut
  ; We used to use product name as MAINBINARYNAME
  ; migrate old shortcuts to target the new MAINBINARYNAME
  !insertmacro IsShortcutTarget "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\$OldMainBinaryName"
  Pop $0
  ${If} $0 = 1
    !insertmacro SetShortcutTarget "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Return
  ${EndIf}

  ; Skip creating shortcut if in update mode or no shortcut mode
  ; but always create if migrating from wix
  ${If} $WixMode = 0
    ${If} $UpdateMode = 1
    ${OrIf} $NoShortcutMode = 1
      Return
    ${EndIf}
  ${EndIf}

  CreateShortcut "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro SetLnkAppUserModelId "$DESKTOP\${PRODUCTNAME}.lnk"
FunctionEnd
