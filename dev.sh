#!/bin/bash
# Convenience wrapper for local iteration on macOS. Sets the pkg-config path
# for the keg-only ffmpeg@7 formula, then runs whatever cargo command you
# passed (defaulting to `cargo run`).
#
# Examples:
#   ./dev.sh                 # cargo run
#   ./dev.sh check
#   ./dev.sh build --release
set -euo pipefail
cd "$(dirname "$0")"

if [ -f "$HOME/.cargo/env" ]; then
    # shellcheck disable=SC1091
    source "$HOME/.cargo/env"
fi

export PKG_CONFIG_PATH="/opt/homebrew/opt/ffmpeg@7/lib/pkgconfig:${PKG_CONFIG_PATH:-}"

if [ $# -eq 0 ]; then
    exec cargo run
else
    exec cargo "$@"
fi
