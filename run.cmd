@echo off
REM ---------------------------------------------------------------------------
REM RunVideo launcher.
REM
REM Double-click this file (or run `run.cmd`) to start the editor. It uses the
REM newest existing build and builds one first if there is none yet.
REM
REM Usage:
REM   run.cmd                       start the editor
REM   run.cmd --export in.rvproj out.mp4 [preset]
REM   run.cmd --version
REM   run.cmd --rebuild             force a fresh debug build, then start
REM   run.cmd --release             build the optimized version, then start
REM ---------------------------------------------------------------------------
setlocal
cd /d "%~dp0"

set "TARGET=x86_64-pc-windows-gnu"
set "RELEASE_EXE=target\%TARGET%\release\runvideo.exe"
set "DEBUG_EXE=target\%TARGET%\debug\runvideo.exe"

set "REBUILD="
set "WANT_RELEASE="
set "ARGS="

:parse
if "%~1"=="" goto parsed
if /I "%~1"=="--rebuild" (
  set "REBUILD=1"
  shift
  goto parse
)
if /I "%~1"=="--release" (
  set "WANT_RELEASE=1"
  shift
  goto parse
)
set ARGS=%ARGS% "%~1"
shift
goto parse

:parsed
set "EXE="
if defined REBUILD goto build
if defined WANT_RELEASE goto build
if exist "%RELEASE_EXE%" set "EXE=%RELEASE_EXE%"
if not defined EXE if exist "%DEBUG_EXE%" set "EXE=%DEBUG_EXE%"
if defined EXE goto launch

:build
if defined WANT_RELEASE (
  echo [RunVideo] Building release ^(this can take a few minutes^)...
  call "%~dp0build.cmd" release
) else (
  echo [RunVideo] Building ^(this can take a few minutes^)...
  call "%~dp0build.cmd"
)
if errorlevel 1 (
  echo [RunVideo] Build failed.
  pause
  exit /b 1
)
if defined WANT_RELEASE (
  set "EXE=%RELEASE_EXE%"
) else (
  set "EXE=%DEBUG_EXE%"
)

:launch
if not exist "%EXE%" (
  echo [RunVideo] Could not find a build to run.
  pause
  exit /b 1
)

if "%ARGS%"=="" (
  REM Plain launch: open the editor without leaving a console window behind.
  start "" "%EXE%"
) else (
  REM CLI mode (--export / --version / ...): keep the console for its output.
  "%EXE%" %ARGS%
)

endlocal
