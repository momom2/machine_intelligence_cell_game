@echo off
rem Build the game (release) and place the executable at the repo root, next to assets\.
cargo build --release -p game || exit /b 1
copy /y target\release\game.exe game.exe >nul
echo game.exe ready at the repo root.
