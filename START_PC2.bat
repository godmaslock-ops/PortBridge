@echo off
setlocal
cd /d "%~dp0"
chcp 65001 >nul
title PortBridge PC2 - Remote USB Test

echo ==========================================
echo   PortBridge PC2 - Auto Connect Test
echo ==========================================
echo.
if not exist portbridge-client.exe (
  echo [ERROR] portbridge-client.exe not found in this folder.
  echo Extract the whole ZIP first.
  echo.
  pause
  exit /b 2
)

echo Searching local network for PortBridge PC1...
echo This can take a few seconds.
echo.

for /f "usebackq delims=" %%I in (`powershell -NoProfile -ExecutionPolicy Bypass -Command "$ip=(Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue ^| Where-Object { $_.IPAddress -notlike '169.254*' -and $_.IPAddress -notlike '127.*' } ^| Select-Object -First 1 -ExpandProperty IPAddress); if(-not $ip){exit 3}; $p=$ip.Substring(0,$ip.LastIndexOf('.')); foreach($n in 1..254){ $h=\"$p.$n\"; $c=New-Object Net.Sockets.TcpClient; try{ $a=$c.BeginConnect($h,47990,$null,$null); if($a.AsyncWaitHandle.WaitOne(60,$false) -and $c.Connected){ $c.EndConnect($a); $c.Close(); Write-Output $h; break } } catch{} finally{ $c.Close() } }"`) do set SERVER_IP=%%I

if "%SERVER_IP%"=="" (
  echo [ERROR] PortBridge PC1 was not found automatically.
  echo Make sure START_PC1.bat is running on PC1 and both PCs are on the same LAN.
  echo.
  pause
  exit /b 3
)

echo Found PC1: %SERVER_IP%
echo Running remote USB control test...
echo.

portbridge-client.exe --remote-control-smoke %SERVER_IP%:47990 > remote_control_test_log.txt 2>&1
type remote_control_test_log.txt

echo.
echo Log saved to:
echo %cd%\remote_control_test_log.txt
echo.
pause
