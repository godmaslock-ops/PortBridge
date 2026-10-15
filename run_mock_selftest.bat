@echo off
setlocal
cd /d "%~dp0"
set /p SERVER_IP=Server IP [127.0.0.1]: 
if "%SERVER_IP%"=="" set SERVER_IP=127.0.0.1
portbridge-client.exe --usb-selftest %SERVER_IP%:47990
pause
