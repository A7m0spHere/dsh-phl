@echo off
rem ---------------------------------------------------------------
rem  PHL - DSH Instance and Runtime Manager
rem  Development launcher.
rem
rem  ASCII only on purpose: cmd.exe reads .cmd files using the OEM
rem  codepage (936 on Chinese Windows), so non-ASCII text here gets
rem  mis-decoded and can corrupt block parsing.
rem ---------------------------------------------------------------
setlocal
cd /d "%~dp0"

echo.
echo   PHL - development mode
echo.

where node >nul 2>nul
if errorlevel 1 goto :no_node

where npm >nul 2>nul
if errorlevel 1 goto :no_node

where cargo >nul 2>nul
if errorlevel 1 goto :no_rust

rem ---- 1/4 -------------------------------------------------------
rem Tauri points the window at a fixed dev URL, so Vite runs with
rem strictPort. A stale dev server from a previous session would make
rem the whole launch fail; clear the port instead of failing.
echo   [1/4] Checking port 5180...
set "FREED="
for /f "tokens=5" %%p in ('netstat -ano ^| findstr /r /c:":5180 .*LISTENING" 2^>nul') do (
  echo         Port held by PID %%p - stopping it.
  taskkill /F /PID %%p >nul 2>nul
  set "FREED=1"
)
if defined FREED (
  rem Give Windows a moment to actually release the socket.
  ping -n 2 127.0.0.1 >nul 2>nul
) else (
  echo         Port is free.
)

rem ---- 2/4 -------------------------------------------------------
rem Always sync dependencies. npm is a no-op when nothing changed, and
rem checking for node_modules is not enough: package.json may have grown
rem new dependencies since the last install.
echo   [2/4] Installing / syncing dependencies...
call npm install
if errorlevel 1 goto :failed

rem ---- 3/4 -------------------------------------------------------
if exist "src-tauri\icons\icon.ico" goto :icon_ok
echo   [3/4] Generating application icons...
call npm run icon
if errorlevel 1 goto :failed
goto :icon_done

:icon_ok
echo   [3/4] Icons already present.

:icon_done

rem ---- 4/4 -------------------------------------------------------
echo   [4/4] Starting the desktop window...
echo         First run compiles the Rust side. This can take several minutes.
echo.
call npm run app:dev
if errorlevel 1 goto :failed

exit /b 0

:no_node
echo   [x] Node.js / npm not found on PATH.
echo       Install it from https://nodejs.org and reopen the terminal.
echo.
pause
exit /b 1

:no_rust
echo   [x] Rust toolchain not found on PATH.
echo.
echo       The desktop shell needs Rust (MSVC target):
echo         1. https://rustup.rs
echo         2. Visual Studio Build Tools, workload "Desktop development with C++"
echo         3. Reopen the terminal so PATH picks up cargo
echo.
echo       To preview the UI in a browser instead, run:  npm run dev
echo.
pause
exit /b 1

:failed
echo.
echo   Failed. Please copy the output above and send it back.
echo.
pause
exit /b 1
