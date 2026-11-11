@echo off
REM Runs the UI smoke test (scripts\smoke-ui.js) in a debug build.
REM
REM The test uses a throwaway profile, not your own: its index and settings
REM live in %TEMP%\loft-smoke-profile, and the only folder in it is
REM sample-docs. Your folders, index and settings are neither read nor
REM changed. It does use the models you have downloaded (read-only), and it
REM picks the same chat and embedding model you have selected.
REM
REM Usage:  scripts\win-smoke.bat        (each step is printed as "[dev] ...")
setlocal
set "LOFT_DATA_DIR=%TEMP%\loft-smoke-profile"
for %%I in ("%~dp0..\sample-docs") do set "LOFT_SEED_FOLDER=%%~fI"
set "LOFT_DEV_EVAL=%~dp0smoke-ui.js"

REM Start from an empty profile that has only the model choice in it.
if not exist "%LOFT_DATA_DIR%" mkdir "%LOFT_DATA_DIR%"
for %%F in (loft.db loft.db-wal loft.db-shm selection.json) do if exist "%LOFT_DATA_DIR%\%%F" del "%LOFT_DATA_DIR%\%%F"
powershell -NoProfile -Command "$real = Join-Path $env:APPDATA 'com.loft.desktop\selection.json'; $sel = if (Test-Path $real) { Get-Content $real -Raw | ConvertFrom-Json } else { $null }; @{ chat = $sel.chat; embed = $sel.embed } | ConvertTo-Json | Set-Content (Join-Path $env:LOFT_DATA_DIR 'selection.json') -Encoding ascii"

call "%~dp0win-dev.bat"
