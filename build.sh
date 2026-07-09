#!/bin/sh
# Build the game (release) and place the executable at the repo root, next to assets/.
set -e
cargo build --release -p game
cp target/release/game.exe ./game.exe 2>/dev/null || cp target/release/game ./game
echo "game binary ready at the repo root."
