@echo off
rem Package the BROWSER build for itch.io: compile to wasm, stage the web shell + binary,
rem bake the replay-upload key, zip. Upload dist\machine-intelligence-web.zip as an
rem itch.io HTML project ("This file will be played in the browser"). Viewport: 1280 x 800.
rem Web-only codegen: the native release profile trades runtime for compile speed
rem (thin LTO, 16 codegen units - the dev loop); packaging is rare, so the browser
rem binary gets the full treatment (smaller wasm, faster ticks).
set CARGO_PROFILE_RELEASE_LTO=fat
set CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
cargo build --release -p game --target wasm32-unknown-unknown || exit /b 1
set CARGO_PROFILE_RELEASE_LTO=
set CARGO_PROFILE_RELEASE_CODEGEN_UNITS=
copy /y target\wasm32-unknown-unknown\release\game.wasm web\game.wasm >nul

rem Stage (this packager cleans ONLY its own outputs), then bake the replay-upload key
rem into index.html from the gitignored local key file. Without the key file the build
rem still packages - the placeholder stays and the client's upload hook no-ops.
if not exist dist mkdir dist
if exist dist\web-stage rmdir /s /q dist\web-stage
mkdir dist\web-stage
copy /y web\index.html dist\web-stage\ >nul
copy /y web\mq_js_bundle.js dist\web-stage\ >nul
copy /y web\game.wasm dist\web-stage\ >nul
if exist infra\upload-key.local.txt (
    powershell -NoProfile -Command "$k = (Get-Content 'infra/upload-key.local.txt' -Raw).Trim(); (Get-Content 'dist/web-stage/index.html' -Raw).Replace('__MI_UPLOAD_KEY__', $k) | Set-Content 'dist/web-stage/index.html' -Encoding ascii -NoNewline" || exit /b 1
    echo replay-upload key baked into index.html.
) else (
    echo WARNING: infra\upload-key.local.txt not found - replay upload DISABLED in this build.
)
powershell -NoProfile -Command "Compress-Archive -Path 'dist/web-stage/*' -DestinationPath 'dist/machine-intelligence-web.zip' -Force" || exit /b 1
echo dist\machine-intelligence-web.zip ready.
