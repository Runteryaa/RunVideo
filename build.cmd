@echo off
REM ---------------------------------------------------------------------------
REM RunVideo build helper for Windows.
REM
REM The Rust GNU toolchain needs a working MinGW (dlltool/ld/as). This script
REM locates a MinGW installation and wires it up for bash, PowerShell and CMD.
REM
REM Usage:  build.cmd            (debug build)
REM         build.cmd release    (optimized single binary)
REM ---------------------------------------------------------------------------
setlocal

set "TARGET=x86_64-pc-windows-gnu"
set "MINGW="

REM 1) Already available on PATH?
where dlltool.exe >nul 2>nul && where gcc.exe >nul 2>nul && for /f "delims=" %%i in ('where gcc.exe') do set "MINGW=%%~dpi"

REM 2) WinLibs installed via winget.
if not defined MINGW (
  for /d %%d in ("%LOCALAPPDATA%\Microsoft\WinGet\Packages\*WinLibs*") do (
    if exist "%%d\mingw64\bin\x86_64-w64-mingw32-gcc.exe" set "MINGW=%%d\mingw64\bin"
  )
)

if not defined MINGW (
  echo [RunVideo] MinGW not found.
  echo Install one with:  winget install BrechtSanders.WinLibs.POSIX.UCRT
  exit /b 1
)

echo [RunVideo] Using MinGW: %MINGW%
set "PATH=%MINGW%;%PATH%"
set "CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=%MINGW%\x86_64-w64-mingw32-gcc.exe"

if /I "%~1"=="release" (
  cargo build --release --target %TARGET% || exit /b 1
  echo [RunVideo] Built target\%TARGET%\release\runvideo.exe
) else (
  cargo build --target %TARGET% || exit /b 1
  echo [RunVideo] Built target\%TARGET%\debug\runvideo.exe
)
endlocal
