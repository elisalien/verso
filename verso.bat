@echo off
rem Builds Verso if needed and starts it from the repository folder, so the
rem bundled effects and MilkDrop presets are found.
cd /d "%~dp0"
rem Cargo by its full path: a malformed PATH entry can hide it from cmd.
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
"%USERPROFILE%\.cargo\bin\cargo.exe" build --release -p verso-app
if errorlevel 1 (
    echo.
    echo Build failed.
    pause
    exit /b 1
)
start "" "%~dp0target\release\verso.exe"
