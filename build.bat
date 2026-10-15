@echo off
rem Builder3D - build and run (needs Rust from https://rustup.rs)
where cargo >nul 2>nul
if errorlevel 1 (
  echo Rust is not installed. Get it from https://rustup.rs and run this again.
  pause
  exit /b 1
)
cargo build --release || (pause & exit /b 1)
echo.
echo Built: %~dp0target\release\builder3d.exe
start "" "%~dp0target\release\builder3d.exe" %*
