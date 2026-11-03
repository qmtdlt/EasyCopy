@echo off
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0script\build_and_run.ps1" %*
if errorlevel 1 (
  echo EasyCopy could not start. Review the error above.
  pause
  exit /b 1
)
