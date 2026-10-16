@echo off
rem Builder3D - build and run (needs Rust from https://rustup.rs)
where cargo >nul 2>nul
if errorlevel 1 (
  echo Rust is not installed. Get it from https://rustup.rs and run this again.
  pause
  exit /b 1
)
rem Build the C runtime into the exe, so it runs on any 64-bit Windows 10/11 PC without
rem the Visual C++ Redistributable. The exe is then portable: zip it and copy it anywhere.
set RUSTFLAGS=-C target-feature=+crt-static
cargo build --release || (pause & exit /b 1)
echo.
echo Built: %~dp0target\release\builder3d.exe
start "" "%~dp0target\release\builder3d.exe" %*
