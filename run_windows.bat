@echo off
cd /d "%~dp0"
python -m filemanager.app
if errorlevel 1 pause
