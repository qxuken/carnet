#!/usr/bin/env bash
# Renders the app icon from its sources into what each OS takes:
#
#   carnet.svg          the page, drawn for 128 px and up
#   carnet-small.svg    the same page for 64 px and under: fewer, heavier
#                       lines, so it still reads as paper
#   carnet-dark.svg     the Espresso twin, for whatever asks for one
#
#   carnet.png          1024 px, the master
#   carnet-dark.png     1024 px
#   carnet-256.png      every window's icon (Windows, X11), built into the binary
#   carnet.icns         a macOS bundle's icon
#   carnet.ico          Windows' executable icon, a frame per size
#
# Needs resvg and ImageMagick (`brew install resvg imagemagick`); the .icns
# needs macOS's iconutil and is skipped elsewhere.
set -euo pipefail
cd "$(dirname "$0")"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# The artwork for a size: the small page up to 64 px.
png() {
    local src=carnet.svg
    [ "$1" -le 64 ] && src=carnet-small.svg
    resvg -w "$1" "$src" "$2"
}

png 1024 carnet.png
resvg -w 1024 carnet-dark.svg carnet-dark.png
png 256 carnet-256.png

if command -v iconutil >/dev/null; then
    set_dir="$tmp/carnet.iconset"
    mkdir "$set_dir"
    for s in 16 32 128 256 512; do
        png "$s" "$set_dir/icon_${s}x${s}.png"
        png $((s * 2)) "$set_dir/icon_${s}x${s}@2x.png"
    done
    iconutil -c icns "$set_dir" -o carnet.icns
fi

frames=()
for s in 16 24 32 48 64 128 256; do
    png "$s" "$tmp/ico-$s.png"
    frames+=("$tmp/ico-$s.png")
done
magick "${frames[@]}" carnet.ico
