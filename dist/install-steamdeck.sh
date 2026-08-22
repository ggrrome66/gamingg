#!/usr/bin/env bash
# gamingg — Steam Deck / Linux installer.
#
# Run this from the dist folder (Desktop Mode on a Deck):
#
#     ./install-steamdeck.sh
#
# It verifies the binary against SHA256SUMS, installs it for the current
# user (no root, no flatpak, nothing system-wide), and drops a launcher in
# the applications menu. It prints the two-step "add to Steam" instructions
# at the end — Game Mode needs the game registered with Steam to launch it.
#
# Uninstall: ./install-steamdeck.sh --uninstall

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BINARY="gamingg-linux-x86_64"
TARGET_DIR="$HOME/.local/share/gamingg"
TARGET="$TARGET_DIR/gamingg"
DESKTOP_DIR="$HOME/.local/share/applications"
DESKTOP="$DESKTOP_DIR/gamingg.desktop"

if [[ "${1:-}" == "--uninstall" ]]; then
    rm -f "$TARGET" "$DESKTOP"
    rmdir --ignore-fail-on-non-empty "$TARGET_DIR" 2>/dev/null || true
    echo "gamingg removed. Saves are kept in ~/.local/share/gamingg/saves;"
    echo "delete that folder too if you want them gone."
    exit 0
fi

if [[ ! -f "$HERE/$BINARY" ]]; then
    echo "error: $BINARY not found next to this script." >&2
    echo "Run the installer from inside the dist folder." >&2
    exit 1
fi

# Trust, then verify: refuse to install a binary that does not match the
# checksum shipped beside it.
if command -v sha256sum >/dev/null 2>&1 && [[ -f "$HERE/SHA256SUMS" ]]; then
    (cd "$HERE" && sha256sum --check --quiet SHA256SUMS)
    echo "checksum OK"
else
    echo "warning: skipping checksum (no sha256sum or no SHA256SUMS)" >&2
fi

mkdir -p "$TARGET_DIR" "$DESKTOP_DIR"
install -m 755 "$HERE/$BINARY" "$TARGET"

cat > "$DESKTOP" <<DESKTOP_ENTRY
[Desktop Entry]
Type=Application
Name=gamingg
Comment=Voxel mining on the frontier
Exec=$TARGET
Icon=applications-games
Terminal=false
Categories=Game;
DESKTOP_ENTRY
command -v update-desktop-database >/dev/null 2>&1 && \
    update-desktop-database "$DESKTOP_DIR" 2>/dev/null || true

echo
echo "Installed to $TARGET"
echo "A launcher is in the applications menu (Desktop Mode)."
echo
echo "To play in Game Mode:"
echo "  1. In Desktop Mode, open Steam -> Games -> 'Add a Non-Steam Game"
echo "     to My Library...' -> Browse -> $TARGET"
echo "  2. Back in Game Mode it appears under Library -> Non-Steam."
echo
echo "The controller works out of the box (Steam Input's default Gamepad"
echo "layout). Press SELECT in game for the control scheme. Saves live in"
echo "~/.local/share/gamingg/saves."
