@echo off
REM Make a Windows release of Loft: an installer at a version number, in
REM releases\windows\<version>\. See release.ps1 for what it does.
REM
REM   scripts\windows\release.bat 0.2.0      release version 0.2.0
REM   scripts\windows\release.bat patch      0.1.0 -> 0.1.1 (also: minor, major)
REM   scripts\windows\release.bat            the current version again
REM   options: -Msi  -SkipTests  -Force
call "%~dp0..\msvc-env.bat" > nul || exit /b 1
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0release.ps1" %*
exit /b %ERRORLEVEL%
