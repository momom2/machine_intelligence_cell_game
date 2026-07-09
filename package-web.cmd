@echo off
rem Package the BROWSER build for itch.io: compile to wasm, stage the web shell + binary,
rem zip. Upload dist\machine-intelligence-web.zip as an itch.io HTML project ("This file
rem will be played in the browser"). Suggested viewport: 1280 x 800.
cargo build --release -p game --target wasm32-unknown-unknown || exit /b 1
copy /y target\wasm32-unknown-unknown\release\game.wasm web\game.wasm >nul
if not exist dist mkdir dist
powershell -NoProfile -Command "Compress-Archive -Path 'web/index.html','web/mq_js_bundle.js','web/game.wasm' -DestinationPath 'dist/machine-intelligence-web.zip' -Force" || exit /b 1
echo dist\machine-intelligence-web.zip ready.
