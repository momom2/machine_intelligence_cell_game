@echo off
rem Package a playable Windows release: build, stage game.exe + the level assets into
rem dist\machine-intelligence\, zip it. Upload the zip as a GitHub release asset, e.g.:
rem   gh release create v0.1.0 dist\machine-intelligence-win64.zip --title "..." --notes "..."
cargo build --release -p game || exit /b 1
if exist dist rmdir /s /q dist
mkdir dist\machine-intelligence\assets
copy /y target\release\game.exe dist\machine-intelligence\game.exe >nul
xcopy /e /i /q assets\levels dist\machine-intelligence\assets\levels >nul
powershell -NoProfile -Command "Compress-Archive -Path 'dist/machine-intelligence' -DestinationPath 'dist/machine-intelligence-win64.zip' -Force" || exit /b 1
echo dist\machine-intelligence-win64.zip ready.
