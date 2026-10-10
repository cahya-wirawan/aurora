#!/usr/bin/env python3
"""Checks a PSD written by Aurora's PSD writer with psd-tools, an
independent reader (0.176.0, PSD write round R1).

Not part of the CI gate: it needs `psd-tools` and `numpy`
(`pip install psd-tools`). Write the fixture first, then check it:

    mkdir -p /tmp/psdw
    AURORA_PSD_FIXTURE_DIR=/tmp/psdw cargo test -p aurora-io --lib \
        write_psd_fixtures_for_psd_tools -- --ignored
    python3 scripts/verify_psd_write.py /tmp/psdw

The Rust test writes `fixture.psd` and `fixture.json` (what the file
should hold: canvas, layers bottom-to-top with name, blend key, opacity,
fill, visibility, position and every expected 16-bit sample, and the
white-matted merged image). This script asserts each of them, pixels
within 1/65535. Exit status 0 means every check passed.
"""

import json
import sys
from pathlib import Path

import numpy as np
from psd_tools import PSDImage
from psd_tools.constants import Tag

TOLERANCE = 1.0 / 65535.0 + 1e-9


def fail(message):
    print(f"FAIL: {message}")
    sys.exit(1)


def check(what, got, want):
    if got != want:
        fail(f"{what}: got {got!r}, expected {want!r}")


def planes(want):
    return np.stack(
        [np.asarray(want[c], dtype=np.float64) / 65535.0 for c in "rgba"], axis=-1
    )


def remove_white_matte(data):
    """psd-tools' own `_remove_background`: c = (c' + a - 1) / a where a > 0."""
    color = data[:, :, :3].copy()
    alpha = np.repeat(data[:, :, 3:4], 3, axis=2)
    mask = alpha > 0
    color[mask] = (color + alpha - 1)[mask] / alpha[mask]
    out = data.copy()
    out[:, :, :3] = color
    return out


def check_pixels(what, got, want, expected=None):
    """`got` is HxWxC float in 0..1; `want` is a dict of r/g/b/a u16 rows."""
    if expected is None:
        expected = planes(want)
    check(f"{what} shape", tuple(got.shape), tuple(expected.shape))
    diff = float(np.max(np.abs(got.astype(np.float64) - expected)))
    if diff > TOLERANCE:
        fail(f"{what}: max difference {diff * 65535.0:.3f}/65535")
    return diff


def main():
    folder = Path(sys.argv[1] if len(sys.argv) > 1 else ".")
    spec = json.loads((folder / "fixture.json").read_text(encoding="utf-8"))
    psd = PSDImage.open(folder / "fixture.psd")

    check("version", psd.version, 1)
    check("width", psd.width, spec["width"])
    check("height", psd.height, spec["height"])
    check("depth", psd.depth, 16)
    check("channels", psd.channels, 4)
    check("colour mode", int(psd.color_mode), 3)

    layers = list(psd)  # psd-tools lists the top level bottom-to-top
    check("layer count", len(layers), len(spec["layers"]))
    worst = 0.0
    for i, (layer, want) in enumerate(zip(layers, spec["layers"])):
        tag = f"layer {i} ({want['name']})"
        check(f"{tag} kind", layer.kind, "pixel")
        check(f"{tag} name", layer.name, want["name"])
        check(f"{tag} blend", layer.blend_mode.value, want["blend"].encode("ascii"))
        check(f"{tag} opacity", layer.opacity, want["opacity"])
        fill = layer.tagged_blocks.get_data(Tag.BLEND_FILL_OPACITY)
        check(f"{tag} fill", fill, want["fill"])
        check(f"{tag} visible", layer.visible, want["visible"])
        check(f"{tag} left", layer.left, want["left"])
        check(f"{tag} top", layer.top, want["top"])
        check(f"{tag} size", (layer.width, layer.height), (want["width"], want["height"]))
        worst = max(worst, check_pixels(f"{tag} pixels", layer.numpy(), want))

    # psd-tools un-mattes the stored merged planes (they are matted onto
    # white because the file has a negative layer count and four
    # channels); that it does so, and gets exactly what un-matting the
    # written samples gives, shows it read them as white-matted RGBA.
    merged = psd.numpy()
    unmatted = remove_white_matte(planes(spec["merged"]))
    worst = max(
        worst, check_pixels("merged image", merged, spec["merged"], expected=unmatted)
    )
    stored = np.stack(
        [np.asarray(spec["merged"][c]) for c in "rgba"], axis=-1
    )
    if np.array_equal(merged * 65535.0, stored):
        fail("merged image came back still matted")
    print(
        f"OK: psd-tools {__import__('psd_tools').__version__} read {len(layers)} layers "
        f"and the merged image; largest pixel difference {worst * 65535.0:.3f}/65535"
    )


if __name__ == "__main__":
    main()
