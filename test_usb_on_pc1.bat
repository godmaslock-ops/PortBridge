@echo off
setlocal
cd /d "%~dp0"
echo === PortBridge PC1 USB discovery test ===
echo Date: %date% %time%
if not exist portbridge-server.exe (
  echo [ERROR] portbridge-server.exe not found next to this BAT file.
  echo Use the Windows artifact from GitHub Actions or run build_windows.bat first.
  pause
  exit /b 2
)
portbridge-server.exe --list-usb > usb_test_log.txt 2>&1
type usb_test_log.txt
echo.
echo Log saved to: %cd%\usb_test_log.txt
pause
