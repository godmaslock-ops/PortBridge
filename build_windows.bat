@echo off
setlocal
cd /d "%~dp0"
where cargo >nul 2>nul || (
  echo [ERROR] Rust/Cargo is not installed.
  echo Install from https://rustup.rs/ or use GitHub Actions.
  pause
  exit /b 1
)
cargo test --workspace || goto :fail
cargo build --release --workspace || goto :fail
if not exist dist mkdir dist
copy /Y target\release\portbridge-server.exe dist\ >nul
copy /Y target\release\portbridge-client.exe dist\ >nul
copy /Y test_usb_on_pc1.bat dist\ >nul
copy /Y run_mock_server.bat dist\ >nul
copy /Y run_mock_selftest.bat dist\ >nul
echo.
echo [OK] Windows binaries are in dist\
pause
exit /b 0
:fail
echo.
echo [ERROR] Build failed. Copy the full console output and send it to me.
pause
exit /b 1
