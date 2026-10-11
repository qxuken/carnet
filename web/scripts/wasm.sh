#!/bin/sh
# The PWA's Rust (crates/carnet-web) as wasm, bound for the page into
# src/wasm/; the fonts (web/fonts, the desktop's as WOFF2: scripts/fonts.sh)
# and the icon, into public/.
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
root=$(cd "$here/.." && pwd)
cd "$root"
cargo build -q -p carnet-web --profile web --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir "$here/src/wasm" target/wasm32-unknown-unknown/web/carnet_web.wasm
rm -rf "$here/public/fonts"
mkdir -p "$here/public/fonts"
cp "$here"/fonts/* "$here/public/fonts/"
cp assets/icon/carnet-256.png "$here/public/icon-256.png"
cp assets/icon/carnet.svg "$here/public/icon.svg"
