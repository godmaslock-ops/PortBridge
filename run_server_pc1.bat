@echo off
setlocal
cd /d "%~dp0"
chcp 65001 >nul
echo === PortBridge server PC1 ===
echo Listening on TCP 47990.
echo Keep this window open during the test.
echo.
portbridge-server.exe 0.0.0.0:47990
pause
