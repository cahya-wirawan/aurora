#!/usr/bin/env bash
# Verify the written PSD with readers that are independent of the writer.
#
# Neither of these is Photoshop, which remains the only check that settles
# ADR 0004. But a file that these two reject would certainly fail there, so
# they are a cheap lower bound.
set -uo pipefail
cd "$(dirname "$0")"

FILE=out/spike.psd
[ -f "$FILE" ] || { echo "run 'cargo run' first"; exit 1; }
fail=0

echo "=== 1. Apple system decoder (sips) — reads the composite image"
if sips -g pixelWidth -g pixelHeight -g format "$FILE" 2>&1 | sed 's/^/    /'; then
  sips -s format png "$FILE" --out out/composite-sips.png >/dev/null 2>&1 \
    && echo "    composite decoded to out/composite-sips.png" \
    || { echo "    FAILED to decode composite"; fail=1; }
else
  echo "    FAILED: sips could not read the file"; fail=1
fi

echo
echo "=== 2. psd-tools — reads the layer structure"
python3 - "$FILE" <<'PY'
import sys
from psd_tools import PSDImage

try:
    psd = PSDImage.open(sys.argv[1])
except Exception as e:
    print(f"    FAILED to open: {e}")
    sys.exit(1)

print(f"    document {psd.width}x{psd.height}, {psd.color_mode}, {len(list(psd))} layers")
ok = True
for layer in psd:
    print(f"      {layer.name!r:<20} {layer.size} at {layer.offset} "
          f"opacity={layer.opacity} blend={layer.blend_mode} visible={layer.visible}")
    try:
        img = layer.topil()
        if img is None:
            print("        WARNING: no pixel data")
            ok = False
    except Exception as e:
        print(f"        FAILED to decode pixels: {e}")
        ok = False

try:
    comp = psd.composite()
    comp.save("out/composite-psdtools.png")
    print("    composite rendered to out/composite-psdtools.png")
except Exception as e:
    print(f"    FAILED to composite: {e}")
    ok = False

sys.exit(0 if ok else 1)
PY
[ $? -ne 0 ] && fail=1

echo
if [ $fail -eq 0 ]; then
  echo "PASS — both independent readers accept the file."
  echo "Still outstanding: Photoshop itself. Nothing here proves that."
else
  echo "FAIL — see above."
fi
exit $fail
