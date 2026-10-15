@echo off
setlocal
cd /d "%~dp0"
chcp 65001 >nul
echo === PortBridge PC2 remote control smoke test ===
echo.
set /p SERVER_IP=PC1 IP address:
if "%SERVER_IP%"=="" (
  echo [ERROR] PC1 IP is required.
  pause
  exit /b 2
)
portbridge-client.exe --remote-control-smoke %SERVER_IP%:47990 > remote_control_test_log.txt 2>&1
type remote_control_test_log.txt
echo.
echo Log saved to: %cd%\remote_control_test_log.txt
pause
