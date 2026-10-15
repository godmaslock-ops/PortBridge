@echo off
setlocal
cd /d "%~dp0"
chcp 65001 >nul
echo === PortBridge PC1 USB diagnostic v0.11 ===
echo Date: %date% %time%
if not exist portbridge-server.exe (
  echo [ERROR] portbridge-server.exe not found next to this BAT file.
  echo Use the Windows artifact from GitHub Actions or run build_windows.bat first.
  pause
  exit /b 2
)
(
  echo === Windows PnP enumeration ===
  portbridge-server.exe --list-usb
  echo.
  echo === Low-level RAW USB probe ===
  portbridge-server.exe --probe-raw-usb
  echo.
  echo === Safe interface claim probe ===
  portbridge-server.exe --probe-claim
  echo.
  echo === ADB transport probe ===
  portbridge-server.exe --probe-adb
) > usb_test_log.txt 2>&1
type usb_test_log.txt
echo.
echo Log saved to: %cd%\usb_test_log.txt
pause
