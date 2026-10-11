#!/bin/sh
# Installs Carnet for the current user from the unpacked archive:
#
#   ./install.sh              into ~/.local (bin, the menu entry, the icon)
#   ./install.sh --uninstall  takes those away again; the notes stay
#
# PREFIX=/usr/local sudo -E ./install.sh installs it for everyone instead.
set -eu
cd "$(dirname "$0")"
prefix=${PREFIX:-$HOME/.local}
bin=$prefix/bin/carnet
desktop=$prefix/share/applications/carnet.desktop
icons=$prefix/share/icons/hicolor

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$bin" "$desktop" "$icons/256x256/apps/carnet.png" "$icons/scalable/apps/carnet.svg"
    echo "Carnet removed. Your notes are where they were (~/Documents/Carnet)."
    exit 0
fi

install -Dm755 carnet "$bin"
install -Dm644 carnet.png "$icons/256x256/apps/carnet.png"
install -Dm644 carnet.svg "$icons/scalable/apps/carnet.svg"
# The menu entry names the binary by its full path: ~/.local/bin is not on
# every desktop session's PATH.
mkdir -p "$(dirname "$desktop")"
sed "s|^Exec=carnet$|Exec=$bin|" carnet.desktop > "$desktop"
chmod 644 "$desktop"
command -v update-desktop-database >/dev/null && update-desktop-database -q "$(dirname "$desktop")" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$icons" 2>/dev/null || true
echo "Carnet installed: $bin (and in the applications menu)."
