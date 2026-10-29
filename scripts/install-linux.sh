#!/usr/bin/env bash
# Builds Slipstream Remote and installs it for the current user
# (~/.local/bin, desktop entry and icon). No root needed.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> Building (release)…"
scripts/check.sh cargo build --release -p slipstream -p slipstream-server

BIN="${XDG_BIN_HOME:-$HOME/.local/bin}"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
install -Dm755 target/release/slipstream "$BIN/slipstream"
install -Dm755 target/release/slipstream-server "$BIN/slipstream-server"
install -Dm644 packaging/linux/slipstream.png "$DATA/icons/hicolor/256x256/apps/slipstream-remote.png"
install -Dm644 packaging/linux/slipstream.png "$DATA/icons/hicolor/256x256/mimetypes/application-x-slipstream-remote.png"
install -Dm644 packaging/linux/slipstream-remote.xml "$DATA/mime/packages/slipstream-remote.xml"
if command -v update-mime-database >/dev/null; then
    update-mime-database "$DATA/mime" || echo "    note: MIME refresh failed; relaunch Slipstream to retry."
else
    echo "    note: install shared-mime-info for executable file icons (Debian: sudo apt install shared-mime-info)."
fi
install -Dm644 packaging/linux/slipstream-remote.desktop "$DATA/applications/slipstream-remote.desktop"
sed -i "s|^Exec=slipstream|Exec=$BIN/slipstream|" "$DATA/applications/slipstream-remote.desktop"
command -v update-desktop-database >/dev/null && update-desktop-database "$DATA/applications" || true
command -v xdg-mime >/dev/null && xdg-mime default slipstream-remote.desktop x-scheme-handler/slipstream || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q "$DATA/icons/hicolor" || true

echo "==> Installed:"
echo "    $BIN/slipstream          (app — also in your application menu)"
echo "    $BIN/slipstream-server   (ID + relay server)"
case ":$PATH:" in *":$BIN:"*) ;; *) echo "    note: add $BIN to your PATH";; esac
