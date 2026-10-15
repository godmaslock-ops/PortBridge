@echo off
cd /d "%~dp0"
portbridge-server.exe --mock-usb 0.0.0.0:47990
pause
