@echo off
rem Dev launcher for PHL: starts the Vite server and the Tauri window
rem (npm run app:dev -> "tauri dev", which runs "npm run dev" as its
rem beforeDevCommand). Closing this console stops both.
rem The project root is this script's parent folder (%~dp0 = scripts\).
cd /d "%~dp0.."
call npm run app:dev
