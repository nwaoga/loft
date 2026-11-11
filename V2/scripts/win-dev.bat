@echo off
REM Launch Loft in dev mode with the correct MSVC environment active.
REM Usage:  scripts\win-dev.bat
call "%~dp0msvc-env.bat" || exit /b 1
cd /d "%~dp0.."
npm run tauri dev
