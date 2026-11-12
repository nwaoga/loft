@echo off
REM Runs the UI smoke test (scripts\smoke-ui.js) in a debug build.
REM
REM The test uses a throwaway profile, not your own: its index and settings
REM live in %TEMP%\loft-smoke-profile, and the only folder in it is
REM sample-docs. Your folders, index and settings are neither read nor
REM changed. It does use the models you have downloaded (read-only), and it
REM picks the same chat and embedding model you have selected.
REM
REM Usage:  scripts\win-smoke.bat                    the UI test (smoke-ui.js)
REM         scripts\win-smoke.bat smoke-engines.js   Performance and Battery:
REM                                                  engines, power, fallback
REM         scripts\win-smoke.bat smoke-optional-engines.js   installs CUDA and
REM                                                  OpenVINO (downloads 700 MB)
REM         set LOFT_SMOKE_KEEP=1 ^& scripts\win-smoke.bat smoke-restart.js
REM                                                  after the UI test: the chat
REM                                                  history survived a restart
REM Each step is printed as "[dev] ...".
setlocal
set "LOFT_DATA_DIR=%TEMP%\loft-smoke-profile"
for %%I in ("%~dp0..\sample-docs") do set "LOFT_SEED_FOLDER=%%~fI"
set "SMOKE_SCRIPT=%~1"
if "%SMOKE_SCRIPT%"=="" set "SMOKE_SCRIPT=smoke-ui.js"
set "LOFT_DEV_EVAL=%~dp0%SMOKE_SCRIPT%"
if not exist "%LOFT_DEV_EVAL%" (echo no such test script: %LOFT_DEV_EVAL% & exit /b 1)

REM Start from an empty profile that has only the model choice in it.
REM (LOFT_SMOKE_KEEP=1 keeps the profile as the last run left it: that is how
REM smoke-restart.js checks that the chat history survives closing Loft.)
if not exist "%LOFT_DATA_DIR%" mkdir "%LOFT_DATA_DIR%"
if "%LOFT_SMOKE_KEEP%"=="1" goto start
for %%F in (loft.db loft.db-wal loft.db-shm chats.db chats.db-wal chats.db-shm selection.json) do if exist "%LOFT_DATA_DIR%\%%F" del "%LOFT_DATA_DIR%\%%F"
powershell -NoProfile -Command "$real = Join-Path $env:APPDATA 'com.loft.desktop\selection.json'; $sel = if (Test-Path $real) { Get-Content $real -Raw | ConvertFrom-Json } else { $null }; @{ chat = $sel.chat; embed = $sel.embed } | ConvertTo-Json | Set-Content (Join-Path $env:LOFT_DATA_DIR 'selection.json') -Encoding ascii"

:start
call "%~dp0win-dev.bat"
