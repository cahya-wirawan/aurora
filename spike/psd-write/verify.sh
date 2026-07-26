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

def walk(layers, depth=0):
    ok = True
    indent = "  " * depth
    for layer in layers:
        if layer.is_group():
            print(f"    {indent}[GROUP {'open' if layer.open_folder else 'closed'}] "
                  f"{layer.name!r} blend={layer.blend_mode} visible={layer.visible} "
                  f"opacity={layer.opacity}")
            ok &= walk(list(layer), depth + 1)
            continue
        print(f"    {indent}{layer.name!r:<20} {layer.size} at {layer.offset} "
              f"opacity={layer.opacity} blend={layer.blend_mode} visible={layer.visible}")
        try:
            img = layer.topil()
            if img is None:
                print(f"    {indent}  WARNING: no pixel data")
                ok = False
        except Exception as e:
            print(f"    {indent}  FAILED to decode pixels: {e}")
            ok = False
    return ok

print(f"    document {psd.width}x{psd.height}, {psd.color_mode}")
ok = walk(list(psd))

# Structural assertions, not just a print — this is what actually catches a
# regression rather than relying on someone eyeballing the tree each time.
try:
    top = list(psd)
    names = [l.name for l in top]
    assert "Background" in names, "top-level Background missing"
    assert "Warm cluster" in names, "top-level group missing"

    cluster = next(l for l in top if l.name == "Warm cluster")
    assert cluster.is_group(), "Warm cluster did not round-trip as a group"
    assert cluster.open_folder is True, "Warm cluster should be open"

    cluster_children = {l.name for l in cluster}
    assert "Warm shape" in cluster_children, "Warm shape missing from its group"
    nested = next(l for l in cluster if l.name == "Nested (closed)")
    assert nested.is_group(), "Nested (closed) did not round-trip as a group"
    assert nested.open_folder is False, "Nested (closed) should be closed"
    assert {l.name for l in nested} == {"Multiply 60%"}, "nested group has the wrong child"

    print("    structural assertions: PASS (2-level nesting, open/closed state, membership)")
except AssertionError as e:
    print(f"    structural assertions: FAIL — {e}")
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
