@echo off
setlocal
cd /d "%~dp0"
chcp 65001 >nul
title PortBridge PC1 - USB Server

echo ==========================================
echo   PortBridge PC1 - USB Server
echo ==========================================
echo.
if not exist portbridge-server.exe (
  echo [ERROR] portbridge-server.exe not found in this folder.
  echo Extract the whole ZIP first.
  echo.
  pause
  exit /b 2
)

echo [1/2] Adding Windows Firewall rule if possible...
netsh advfirewall firewall show rule name="PortBridge USB Server" >nul 2>&1
if errorlevel 1 (
  netsh advfirewall firewall add rule name="PortBridge USB Server" dir=in action=allow protocol=TCP localport=47990 >nul 2>&1
)

echo [2/2] Starting PortBridge on TCP 47990...
echo.
echo Leave this window OPEN while the phone is connected.
echo If Windows asks for network access, click Allow.
echo.
portbridge-server.exe 0.0.0.0:47990

echo.
echo PortBridge server stopped.
pause
