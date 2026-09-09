#!/bin/bash
# Build a shareable .app + zip on the Desktop:
#   1. Release-build via deploy.sh (copies .app to Desktop)
#   2. dylibbundler pulls FFmpeg dylibs into Contents/Frameworks and rewrites
#      the binary's install names so it no longer depends on /opt/homebrew.
#   3. Strip the quarantine attribute so the recipient doesn't hit Gatekeeper
#      unnecessarily on their own machine.
#   4. Zip the whole .app for easy sharing.
set -euo pipefail

cd "$(dirname "$0")"

./deploy.sh

APP_NAME="Wallpaper PoC.app"
APP="$HOME/Desktop/$APP_NAME"
BINARY="$APP/Contents/MacOS/wallpaper_poc"
FRAMEWORKS="$APP/Contents/Frameworks"

mkdir -p "$FRAMEWORKS"

echo "==> Bundling dylibs into $APP …"
dylibbundler \
    --overwrite-files \
    --bundle-deps \
    --create-dir \
    --fix-file "$BINARY" \
    --dest-dir "$FRAMEWORKS" \
    --install-path "@executable_path/../Frameworks/"

# Make sure the app has no lingering quarantine attribute on our own machine.
xattr -cr "$APP" 2>/dev/null || true

# Sanity check: any remaining /opt/homebrew reference will break on other Macs.
if otool -L "$BINARY" | grep -q "/opt/homebrew"; then
    echo "!! WARNING: binary still references /opt/homebrew paths:"
    otool -L "$BINARY" | grep "/opt/homebrew"
    exit 1
fi

ZIP="$HOME/Desktop/Wallpaper_PoC.zip"
rm -f "$ZIP"
echo "==> Zipping …"
(cd "$HOME/Desktop" && zip -qr "$(basename "$ZIP")" "$APP_NAME")

SIZE=$(du -sh "$ZIP" | awk '{print $1}')
echo ""
echo "Ready to share:"
echo "  $ZIP  ($SIZE)"
echo ""
echo "Tell the recipient (Apple Silicon Mac only):"
echo "  1. Unzip"
echo "  2. Right-click the app → Open → Open (bypass Gatekeeper first time)"
echo "     (or run: xattr -dr com.apple.quarantine \"/path/to/Wallpaper PoC.app\")"
