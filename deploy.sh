#!/bin/bash
# Rebuild the .app bundle and replace the copy on Desktop.
# Usage: ./deploy.sh
set -euo pipefail

cd "$(dirname "$0")"

# Kill any running instance so we can overwrite the .app cleanly.
pkill -x wallpaper_poc 2>/dev/null || true

# Ensure cargo is on PATH even when run outside an interactive shell.
if [ -f "$HOME/.cargo/env" ]; then
    # shellcheck disable=SC1091
    source "$HOME/.cargo/env"
fi

# ffmpeg@7 is keg-only in Homebrew; point pkg-config at its .pc files so
# ffmpeg-sys-next can find it.
export PKG_CONFIG_PATH="/opt/homebrew/opt/ffmpeg@7/lib/pkgconfig:${PKG_CONFIG_PATH:-}"

cargo bundle --release

APP_NAME="Wallpaper PoC.app"
SRC="target/release/bundle/osx/$APP_NAME"
DEST="$HOME/Desktop/$APP_NAME"

rm -rf "$DEST"
cp -R "$SRC" "$DEST"

echo "Deployed $DEST"
