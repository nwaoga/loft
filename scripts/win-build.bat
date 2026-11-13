@echo off
REM Produce a release build / installers with the correct MSVC environment.
REM Usage:  scripts\win-build.bat
call "%~dp0msvc-env.bat" || exit /b 1
cd /d "%~dp0.."
npm run tauri build
