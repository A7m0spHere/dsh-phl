@echo off
rem ---------------------------------------------------------------
rem  PHL - build the desktop application.
rem
rem  Output:
rem    src-tauri\target\release\PHL.exe
rem    src-tauri\target\release\bundle\nsis\PHL_<version>_x64-setup.exe
rem
rem  ASCII only on purpose - see the note in run-dev.cmd.
rem ---------------------------------------------------------------
setlocal
cd /d "%~dp0"

echo.
echo   PHL - building desktop application
echo.

where node >nul 2>nul
if errorlevel 1 goto :no_node

where npm >nul 2>nul
if errorlevel 1 goto :no_node

where cargo >nul 2>nul
if errorlevel 1 goto :no_rust

rem The release build runs Vite once and exits, but a stale dev server on
rem 5180 can still confuse a follow-up `app:dev`, so clear it here too.
for /f "tokens=5" %%p in ('netstat -ano ^| findstr /r /c:":5180 .*LISTENING" 2^>nul') do (
  echo   Stopping stale dev server on port 5180 (PID %%p^).
  taskkill /F /PID %%p >nul 2>nul
)

echo   [1/3] Installing / syncing dependencies...
call npm install
if errorlevel 1 goto :failed

if exist "src-tauri\icons\icon.ico" goto :icon_ok
echo   [2/3] Generating application icons...
call npm run icon
if errorlevel 1 goto :failed
goto :icon_done

:icon_ok
echo   [2/3] Icons already present.

:icon_done
echo   [3/3] Building release bundle. This is slow on the first run.
echo.
call npm run app:build
if errorlevel 1 goto :failed

echo.
echo   Done. Opening the output folder...
if exist "src-tauri\target\release\bundle\nsis\" (
  start "" "src-tauri\target\release\bundle\nsis"
) else (
  start "" "src-tauri\target\release"
)
echo.
pause
exit /b 0

:no_node
echo   [x] Node.js / npm not found on PATH.
echo       Install it from https://nodejs.org and reopen the terminal.
echo.
pause
exit /b 1

:no_rust
echo   [x] Rust toolchain not found on PATH.
echo       Install from https://rustup.rs plus the Visual Studio
echo       "Desktop development with C++" workload, then reopen the terminal.
echo.
pause
exit /b 1

:failed
echo.
echo   Build failed. Please copy the output above and send it back.
echo.
pause
exit /b 1
