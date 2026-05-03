#!/usr/bin/env bash
#
# Renders site/og-card.html to site/og-card.png at 1200x630 using a
# headless Chromium-based browser.
#
# Usage:
#   ./site/make-og-card.sh           # render once
#   MORPH_OG_CHROME=/path/to/chrome ./site/make-og-card.sh
#
# Re-run whenever site/og-card.html changes; commit the resulting PNG.

set -euo pipefail

cd "$(dirname "$0")"

INPUT_HTML="$(pwd)/og-card.html"
OUTPUT_PNG="$(pwd)/og-card.png"

if [[ ! -f "$INPUT_HTML" ]]; then
  echo "error: $INPUT_HTML not found" >&2
  exit 1
fi

CHROME="${MORPH_OG_CHROME:-}"

if [[ -z "$CHROME" ]]; then
  for candidate in \
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
    "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary" \
    "/Applications/Google Chrome Beta.app/Contents/MacOS/Google Chrome Beta" \
    "/Applications/Chromium.app/Contents/MacOS/Chromium" \
    "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser" \
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge" \
    "$(command -v google-chrome 2>/dev/null || true)" \
    "$(command -v chromium 2>/dev/null || true)" \
    "$(command -v chromium-browser 2>/dev/null || true)"
  do
    if [[ -n "$candidate" && -x "$candidate" ]]; then
      CHROME="$candidate"
      break
    fi
  done
fi

if [[ -z "$CHROME" || ! -x "$CHROME" ]]; then
  cat >&2 <<EOF
error: no Chromium-based browser found.

Tried: Google Chrome / Canary / Beta, Chromium, Brave, Microsoft Edge,
and PATH lookups for google-chrome / chromium / chromium-browser.

Set MORPH_OG_CHROME=/path/to/chrome to override.
EOF
  exit 1
fi

echo "rendering with: $CHROME"

# Render at a taller viewport so absolute-positioned bottom-anchored content
# is guaranteed to be inside the captured area, then crop to 1200x630 with
# sips. (Headless Chrome has been seen to clip absolute children near the
# viewport's lower edge at exact target heights — rendering tall and cropping
# is the reliable workaround.)
RENDER_HEIGHT=800
TARGET_WIDTH=1200
TARGET_HEIGHT=630

# --virtual-time-budget keeps the page alive long enough for Google Fonts to
# fetch and apply.
"$CHROME" \
  --headless=new \
  --disable-gpu \
  --hide-scrollbars \
  --no-sandbox \
  --no-first-run \
  --no-default-browser-check \
  --window-size=${TARGET_WIDTH},${RENDER_HEIGHT} \
  --force-device-scale-factor=1 \
  --default-background-color=00000000 \
  --virtual-time-budget=10000 \
  --screenshot="$OUTPUT_PNG" \
  "file://$INPUT_HTML" \
  >/dev/null 2>&1 || {
    echo "error: chrome screenshot failed" >&2
    exit 1
  }

if [[ ! -s "$OUTPUT_PNG" ]]; then
  echo "error: $OUTPUT_PNG was not produced (or is empty)" >&2
  exit 1
fi

# Crop to 1200x630 from the top-left. Python+PIL is more deterministic than
# sips for offset-based crops.
python3 - "$OUTPUT_PNG" ${TARGET_WIDTH} ${TARGET_HEIGHT} <<'PYEOF'
import sys
from PIL import Image
path, w, h = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
img = Image.open(path)
img.crop((0, 0, w, h)).save(path, optimize=True)
PYEOF

bytes=$(wc -c <"$OUTPUT_PNG" | tr -d ' ')
dims=$(sips -g pixelWidth -g pixelHeight "$OUTPUT_PNG" 2>/dev/null \
       | awk '/pixel/{print $2}' | paste -sd 'x' -)
echo "wrote: $OUTPUT_PNG (${dims}, ${bytes} bytes)"
