#!/bin/sh
# The desktop's fonts (assets/fonts) as WOFF2 for the viewer, into
# web/fonts/, which is kept in the repository: about a third of the TTFs'
# size, which matters for what the service worker keeps on a phone. Run
# again when the desktop's fonts change. Needs Python with fonttools and
# brotli (`pip install fonttools brotli`); PYTHON names another one.
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
root=$(cd "$here/.." && pwd)
py=${PYTHON:-python3}
mkdir -p "$here/fonts"
for f in Inter-Regular Inter-Bold Inter-Italic SourceSerif4-Regular SourceSerif4-Semibold SourceSerif4-It; do
  "$py" -c "import sys; from fontTools.ttLib import TTFont; t = TTFont(sys.argv[1]); t.flavor = 'woff2'; t.save(sys.argv[2])" \
    "$root/assets/fonts/$f.ttf" "$here/fonts/$f.woff2"
done
cp "$root/assets/fonts/Inter-OFL.txt" "$root/assets/fonts/SourceSerif4-OFL.md" "$here/fonts/"
