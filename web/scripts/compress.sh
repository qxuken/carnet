#!/bin/sh
# Brotli and gzip copies beside the big files, for carnet-server to send
# as they are (its ServeDir serves a .br or .gz when the browser takes it).
set -eu
cd "$(dirname "$0")/../dist"
find . -type f \( -name '*.wasm' -o -name '*.js' -o -name '*.css' -o -name '*.html' -o -name '*.svg' \) | while read -r f; do
  gzip -9 -k -f "$f"
  if command -v brotli >/dev/null; then brotli -q 11 -f -k "$f"; fi
done
