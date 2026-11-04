#!/usr/bin/env bash
# Cross-builds a portable Windows test package from Linux:
#   game-capture-<version>-windows-x64.zip
#     game-capture.exe, WebView2Loader.dll, binaries/ (FFmpeg), TESTING.txt
#
# Needs: rustup target x86_64-pc-windows-gnu, mingw-w64, node, zip, curl.
# Usage: scripts/package-portable.sh [output-dir]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-$ROOT/target/portable}"
TARGET=x86_64-pc-windows-gnu
VERSION="$(grep -m1 '^version' "$ROOT/Cargo.toml" | cut -d'"' -f2)"
NAME="game-capture-$VERSION-windows-x64"
FFMPEG_ZIP="ffmpeg-n8.1-latest-win64-gpl-shared-8.1.zip"
FFMPEG_URL="https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/$FFMPEG_ZIP"

echo "==> building UI"
npm --prefix "$ROOT/ui" run build

echo "==> building game-capture.exe"
# `tauri/custom-protocol` embeds the UI in the exe. Without it a release
# build still loads the dev server (http://localhost:5173) and shows
# "localhost refused to connect". `tauri build` sets it automatically.
cargo build --manifest-path "$ROOT/Cargo.toml" -p game-capture --release \
  --target "$TARGET" --features tauri/custom-protocol

BIN="$ROOT/target/$TARGET/release"
STAGE="$OUT/$NAME"
rm -rf "$STAGE" && mkdir -p "$STAGE/binaries"
cp "$BIN/game-capture.exe" "$BIN/WebView2Loader.dll" "$STAGE/"

CACHE="$ROOT/target/ffmpeg-cache"
mkdir -p "$CACHE"
if [ ! -f "$CACHE/$FFMPEG_ZIP" ]; then
  echo "==> downloading FFmpeg"
  curl -fL -o "$CACHE/$FFMPEG_ZIP.part" "$FFMPEG_URL"
  mv "$CACHE/$FFMPEG_ZIP.part" "$CACHE/$FFMPEG_ZIP"
fi
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
unzip -q "$CACHE/$FFMPEG_ZIP" -d "$TMP"
FF="$(find "$TMP" -maxdepth 1 -type d -name 'ffmpeg-*' | head -1)"
cp "$FF"/bin/ffmpeg.exe "$FF"/bin/ffprobe.exe "$FF"/bin/*.dll "$STAGE/binaries/"
cp "$FF/LICENSE.txt" "$STAGE/binaries/FFMPEG-LICENSE.txt"
cp "$ROOT/scripts/TESTING.txt" "$STAGE/TESTING.txt"

(cd "$OUT" && rm -f "$NAME.zip" && zip -q -r -9 "$NAME.zip" "$NAME")
echo "==> $OUT/$NAME.zip"
