@echo off
setlocal EnableDelayedExpansion
cd /d "%~dp0"
chcp 65001 >nul
title PortBridge PC2 - Auto Connect Test

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

del /q portbridge_candidates.txt 2>nul
del /q portbridge_probe_tmp.txt 2>nul

echo Searching all local IPv4 networks for PortBridge PC1...
echo No IP typing is required.
echo.

powershell -NoProfile -ExecutionPolicy Bypass -Command ^
"$ips=@(Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue ^| Where-Object { $_.IPAddress -notlike '127.*' -and $_.IPAddress -notlike '169.254*' -and $_.PrefixLength -le 24 } ^| Select-Object -ExpandProperty IPAddress -Unique); $locals=@{}; foreach($x in $ips){$locals[$x]=$true}; $prefixes=@(); foreach($ip in $ips){$p=$ip.Substring(0,$ip.LastIndexOf('.')); if($prefixes -notcontains $p){$prefixes+=$p}}; foreach($p in $prefixes){ foreach($n in 1..254){ $h=\"$p.$n\"; if($locals.ContainsKey($h)){continue}; $c=New-Object Net.Sockets.TcpClient; try{ $a=$c.BeginConnect($h,47990,$null,$null); if($a.AsyncWaitHandle.WaitOne(45,$false) -and $c.Connected){ try{$c.EndConnect($a)}catch{}; Add-Content -Path 'portbridge_candidates.txt' -Value $h } } catch{} finally{ $c.Close() } } }"

if not exist portbridge_candidates.txt (
  echo [ERROR] No computer with TCP 47990 was found.
  echo Make sure START_PC1.bat is still running and both PCs are on the same LAN.
  echo.
  pause
  exit /b 3
)

set SERVER_IP=
for /f "usebackq delims=" %%H in ("portbridge_candidates.txt") do (
  echo Verifying %%H ...
  portbridge-client.exe --probe-server %%H:47990 > portbridge_probe_tmp.txt 2>&1
  if !errorlevel! EQU 0 (
    findstr /c:"PORTBRIDGE_OK" portbridge_probe_tmp.txt >nul 2>&1
    if !errorlevel! EQU 0 (
      set SERVER_IP=%%H
      goto :FOUND
    )
  )
)

:FOUND
if "%SERVER_IP%"=="" (
  echo.
  echo [ERROR] TCP 47990 was found, but none of those computers answered as PortBridge.
  echo Check that START_PC1.bat is running on PC1.
  echo.
  pause
  exit /b 4
)

echo.
echo PortBridge PC1 verified at %SERVER_IP%
echo Running remote USB control test...
echo.

portbridge-client.exe --remote-control-smoke %SERVER_IP%:47990 > remote_control_test_log.txt 2>&1
type remote_control_test_log.txt

echo.
echo ==========================================
echo   SAFE BULK ENDPOINT PROBE
echo ==========================================
echo This probe does NOT send USB payloads.
echo.

portbridge-client.exe --safe-bulk-probe %SERVER_IP%:47990 > bulk_probe_log.txt 2>&1
type bulk_probe_log.txt

echo.
echo Logs saved to:
echo %cd%\remote_control_test_log.txt
echo %cd%\bulk_probe_log.txt
echo.
del /q portbridge_candidates.txt 2>nul
del /q portbridge_probe_tmp.txt 2>nul
pause
