#!/usr/bin/env python3
"""Opens every fixture with psd-tools (independent of any future Aurora
reader) and records what actually opened, what didn't, and what structural
features are present -- so Phase 3 planning starts from a real inventory
instead of a file count. See README.md.

Usage: python3 inventory.py > inventory.md
"""
import sys
from pathlib import Path

from psd_tools import PSDImage

ROOT = Path(__file__).parent / "psd-tools-fixtures"

# PSD file header color mode codes (Adobe spec); psd-tools exposes the raw
# int here rather than a named enum.
COLOR_MODE_NAMES = {
    0: "Bitmap",
    1: "Grayscale",
    2: "Indexed",
    3: "RGB",
    4: "CMYK",
    7: "Multichannel",
    8: "Duotone",
    9: "Lab",
}


def layer_kinds(psd):
    kinds = set()

    def walk(layers):
        for layer in layers:
            kinds.add(layer.kind)
            if layer.is_group():
                walk(layer)

    walk(psd)
    return kinds


def main():
    files = sorted(p for p in ROOT.rglob("*") if p.suffix.lower() in (".psd", ".psb"))
    if not files:
        print("No fixtures found -- run ./fetch-samples.sh first.", file=sys.stderr)
        sys.exit(1)

    ok, failed = [], []
    color_modes = {}
    kinds_seen = {}

    for path in files:
        rel = path.relative_to(ROOT)
        try:
            psd = PSDImage.open(path)
            mode = COLOR_MODE_NAMES.get(int(psd.color_mode), f"unknown({psd.color_mode})")
            color_modes[mode] = color_modes.get(mode, 0) + 1
            for kind in layer_kinds(psd):
                kinds_seen[kind] = kinds_seen.get(kind, 0) + 1
            ok.append(rel)
        except Exception as exc:  # noqa: BLE001 -- inventory, not production code
            failed.append((rel, repr(exc)))

    print(f"# PSD corpus inventory\n")
    print(f"{len(ok)}/{len(files)} fixtures opened successfully with psd-tools "
          f"{__import__('psd_tools').__version__}.\n")

    if failed:
        print("## Failed to open\n")
        for rel, exc in failed:
            print(f"- `{rel}` — {exc}")
        print()

    print("## Color modes seen\n")
    for mode, count in sorted(color_modes.items(), key=lambda kv: -kv[1]):
        print(f"- {mode}: {count} files")
    print()

    print("## Layer kinds seen\n")
    for kind, count in sorted(kinds_seen.items(), key=lambda kv: -kv[1]):
        print(f"- `{kind}`: {count} files")


if __name__ == "__main__":
    main()
