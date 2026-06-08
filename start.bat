@echo off
setlocal

where cargo >nul 2>nul
if errorlevel 1 (
  echo Rust/Cargo n'est pas installe ou pas dans le PATH.
  echo Installe Rust depuis https://rustup.rs/ puis relance ce fichier.
  pause
  exit /b 1
)

echo Lancement de L7dstat sur http://localhost:5000
cargo run --release
pause
