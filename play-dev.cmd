@echo off
rem Dev launcher: rebuild if needed (a fast no-op when fresh), then run the game.
rem game-dev.lnk points here, so the shortcut ALWAYS launches current code - it survives
rem rebuilds, `cargo clean`, and forgetting to rebuild alike.
cd /d %~dp0
cargo build --release -p game
if errorlevel 1 pause & exit /b 1
start "" target\release\game.exe
