@echo off
REM ---------------------------------------------------------------------------
REM Activates a COMPLETE MSVC toolset for Rust/Tauri builds on Windows.
REM
REM Why this exists: this machine has several Visual Studio installs, and Rust's
REM linker auto-detection picked an incomplete one (VS 2026 Professional, whose
REM C++ libraries are not fully installed) -> LNK1104: cannot open 'msvcrt.lib'.
REM `vswhere -requires ...VC.Tools.x86.x64` returns only installs that actually
REM have the full C++ toolset (the Build Tools), and we activate that one.
REM ---------------------------------------------------------------------------
set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if not exist "%VSWHERE%" (
  echo [msvc-env] vswhere not found; is Visual Studio / Build Tools installed?
  exit /b 1
)
set "VSPATH="
for /f "usebackq tokens=*" %%i in (`"%VSWHERE%" -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set "VSPATH=%%i"
if "%VSPATH%"=="" (
  echo [msvc-env] No VS install with the C++ toolset found. Install "Desktop development with C++".
  exit /b 1
)
call "%VSPATH%\VC\Auxiliary\Build\vcvars64.bat" >nul
if errorlevel 1 (
  echo [msvc-env] vcvars64 failed.
  exit /b 1
)
REM Put cargo/rustc on PATH for this shell.
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
echo [msvc-env] Using MSVC at: %VSPATH%
exit /b 0
