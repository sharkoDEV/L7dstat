@echo off
setlocal

where cargo >nul 2>nul
if errorlevel 1 (
  echo Rust/Cargo n'est pas installe ou pas dans le PATH.
  echo Installe Rust depuis https://rustup.rs/ puis relance ce fichier.
  pause
  exit /b 1
)

cargo build --release
if errorlevel 1 (
  pause
  exit /b 1
)

echo Build OK: target\release\l7dstat.exe
pause
