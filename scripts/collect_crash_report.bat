@echo off
rem Nivora Launcher: сбор диагностики краша на Windows за один запуск.
rem Положит отчёт на рабочий стол (nivora-crash-report.txt) и откроет его.
rem Можно перетащить exe лаунчера на этот bat — тогда отчёт запишет ещё и
rem код завершения запуска (0xC0000005 и т.п. расшифрованы внизу).
setlocal

set "OUT=%USERPROFILE%\Desktop\nivora-crash-report.txt"
>  "%OUT%" echo === Nivora Launcher crash report ===
>> "%OUT%" echo time: %DATE% %TIME%
>> "%OUT%" echo os: %OS%
ver >> "%OUT%" 2>&1
>> "%OUT%" echo.

>> "%OUT%" echo --- [1] Application event log (nivora, latest 12) ---
powershell -NoProfile -ExecutionPolicy Bypass -Command "Get-WinEvent -LogName Application -MaxEvents 400 | Where-Object { $_.Message -match 'nivora' } | Select-Object -First 12 | Format-List TimeCreated, ProviderName, Id, Message" >> "%OUT%" 2>&1
>> "%OUT%" echo.

>> "%OUT%" echo --- [2] Windows Defender detections (latest 10) ---
powershell -NoProfile -ExecutionPolicy Bypass -Command "Get-MpThreatDetection | Sort-Object InitialDetectionTime -Descending | Select-Object -First 10 InitialDetectionTime, ProcessName, Resources | Format-List" >> "%OUT%" 2>&1
>> "%OUT%" echo.

>> "%OUT%" echo --- [3] %LOCALAPPDATA%\nivora-launcher (recursive) ---
dir "%LOCALAPPDATA%\nivora-launcher" /s >> "%OUT%" 2>&1
>> "%OUT%" echo.

>> "%OUT%" echo --- [3b] launcher exe candidates (timestamps matter!) ---
dir "%LOCALAPPDATA%\Nivora Launcher" >> "%OUT%" 2>&1
dir "%LOCALAPPDATA%\Nivora Launcher\backend" >> "%OUT%" 2>&1
dir "%LOCALAPPDATA%\Programs\Nivora Launcher\*.exe" >> "%OUT%" 2>&1
dir "%USERPROFILE%\Desktop\*nivora*.exe" >> "%OUT%" 2>&1
dir "%USERPROFILE%\Downloads\*nivora*.exe" >> "%OUT%" 2>&1
>> "%OUT%" echo.

>> "%OUT%" echo --- [4] startup.log (ключевой файл: какой exe и как далеко дошёл) ---
type "%LOCALAPPDATA%\nivora-launcher\startup.log" >> "%OUT%" 2>&1
>> "%OUT%" echo.

>> "%OUT%" echo --- [5] panic.log ---
type "%LOCALAPPDATA%\nivora-launcher\panic.log" >> "%OUT%" 2>&1
>> "%OUT%" echo.

>> "%OUT%" echo --- [6] native-crash.log ---
type "%LOCALAPPDATA%\nivora-launcher\native-crash.log" >> "%OUT%" 2>&1
>> "%OUT%" echo.

>> "%OUT%" echo --- [6] backend.log (last 80 lines) ---
powershell -NoProfile -ExecutionPolicy Bypass -Command "if (Test-Path \"$env:LOCALAPPDATA\nivora-launcher\logs\backend.log\") { Get-Content \"$env:LOCALAPPDATA\nivora-launcher\logs\backend.log\" -Tail 80 }" >> "%OUT%" 2>&1
>> "%OUT%" echo.

set "EXE=%~1"
if not defined EXE set "EXE=%LOCALAPPDATA%\Nivora Launcher\nivora-launcher.exe"
if not exist "%EXE%" set "EXE=%LOCALAPPDATA%\Programs\Nivora Launcher\nivora-launcher.exe"
if exist "%EXE%" (
  >> "%OUT%" echo --- [7] launching: "%EXE%" ---
  "%EXE%"
  >> "%OUT%" echo exit code (decimal): %ERRORLEVEL%
  >> "%OUT%" echo exit code (hex^): use calc, common ones:
  if "%ERRORLEVEL%"=="3221225477" >> "%OUT%" echo   ^> 0xC0000005 access violation
  if "%ERRORLEVEL%"=="3221225781" >> "%OUT%" echo   ^> 0xC0000135 DLL not found (VC++ redist?)
  if "%ERRORLEVEL%"=="3221225794" >> "%OUT%" echo   ^> 0xC0000142 DLL initialization failed
  if "%ERRORLEVEL%"=="3221226505" >> "%OUT%" echo   ^> 0xC0000409 fail-fast / stack overrun
  if "%ERRORLEVEL%"=="5" >> "%OUT%" echo   ^> access denied (defender or acl)
) else (
  >> "%OUT%" echo --- [7] launcher exe not found; drag exe onto this bat to capture exit code ---
)

start notepad "%OUT%"
