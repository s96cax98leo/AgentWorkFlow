@echo off
rem awf launcher for Windows: picks the executable for this machine, no logic here.
rem Keep this file ASCII only: cmd.exe reads it in the console code page.
rem Set AWF_BIN to the path of your own build to use that instead.
rem Windows on arm64 runs the x64 build. See docs for details.
setlocal DisableDelayedExpansion
if defined AWF_BIN goto custom
set "arch=%PROCESSOR_ARCHITECTURE%"
if defined PROCESSOR_ARCHITEW6432 set "arch=%PROCESSOR_ARCHITEW6432%"
if /i "%arch%"=="AMD64" goto pick
if /i "%arch%"=="ARM64" goto pick
echo awf: unsupported platform [%arch%]; build it yourself and set AWF_BIN to the executable 1>&2
exit /b 127
:pick
set "bin=%~dp0awf-windows-x64.exe"
goto run
:custom
set "bin=%AWF_BIN%"
:run
if exist "%bin%" goto found
echo awf: executable not found: "%bin%"; update the plugin, or build it yourself and set AWF_BIN 1>&2
exit /b 127
:found
"%bin%" %*
exit /b %ERRORLEVEL%
