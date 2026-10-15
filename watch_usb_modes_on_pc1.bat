@echo off
setlocal
cd /d "%~dp0"
chcp 65001 >nul
echo === PortBridge USB mode watcher ===
echo.
echo Leave this window open.
echo Now switch the phone between Android / Recovery-Mi Assistant / Fastboot.
echo PortBridge will print removed/added USB identities.
echo Press Ctrl+C when finished.
echo.
if not exist portbridge-server.exe (
  echo [ERROR] portbridge-server.exe not found next to this BAT file.
  pause
  exit /b 2
)
portbridge-server.exe --watch-usb
pause
