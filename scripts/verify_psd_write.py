#!/usr/bin/env python3
"""Checks a PSD written by Aurora's PSD writer with psd-tools, an
independent reader (0.176.0, PSD write round R1; groups and masks
0.178.0, R3).

Not part of the CI gate: it needs `psd-tools` and `numpy`
(`pip install psd-tools`). Write the fixture first, then check it:

    mkdir -p /tmp/psdw
    AURORA_PSD_FIXTURE_DIR=/tmp/psdw cargo test -p aurora-io --lib \
        fixtures_for_psd_tools -- --ignored
    python3 scripts/verify_psd_write.py /tmp/psdw

The Rust test writes `fixture.psd` and `fixture.json` (what the file
should hold: canvas, layers bottom-to-top with name, blend key, opacity,
fill, visibility, position and every expected 16-bit sample, and the
white-matted merged image). This script asserts each of them, pixels
within 1/65535. Exit status 0 means every check passed.

Since 0.178.0 a second fixture, `groups.psd` / `groups.json`, holds
nested groups and user masks on a layer and a group: the tree (every
group's membership and order, its blend key, opacity, fill and
visibility), each mask's bounding box, default colour, disabled flag,
user-mask density, and every `u16` mask sample read from the `-2`
channel's own decompressed data — exactly, not within a tolerance.
"""

import json
import sys
from pathlib import Path

import numpy as np
from psd_tools import PSDImage
from psd_tools.constants import Resource, Tag

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

    # 0.177.0: the sRGB ICC profile as image resource 1039.
    icc = psd.image_resources.get_data(Resource.ICC_PROFILE)
    if not icc:
        fail("no ICC profile resource (1039)")
    check("ICC size field", int.from_bytes(icc[0:4], "big"), len(icc))
    check("ICC signature", icc[36:40], b"acsp")
    check("ICC colour space", icc[16:20], b"RGB ")
    if b"s\x00R\x00G\x00B" not in icc and b"sRGB" not in icc:
        fail("ICC profile does not name sRGB")
    try:
        import io
        from PIL import ImageCms

        name = ImageCms.getProfileDescription(ImageCms.ImageCmsProfile(io.BytesIO(icc)))
        print(f"ICC profile parsed by Pillow/LittleCMS: {name.strip()!r}")
    except ImportError:
        print("Pillow's ImageCms is not installed; ICC header checked by hand only")

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


def check_mask(tag, layer, want):
    if want is None:
        if layer.mask is not None and layer.mask.width > 0:
            fail(f"{tag}: a mask that was not written")
        return
    mask = layer.mask
    if mask is None:
        fail(f"{tag}: no mask")
    check(f"{tag} mask bbox", tuple(mask.bbox), tuple(want["bbox"]))
    check(f"{tag} mask default colour", mask.background_color, want["default"])
    check(f"{tag} mask disabled", bool(mask.disabled), want["disabled"])
    check(f"{tag} mask invert flag", bool(mask.flags.invert_mask), False)
    check(f"{tag} mask parameters flag", bool(mask.flags.parameters_applied), want["density"] is not None)
    density = None
    if mask.parameters is not None:
        density = mask.parameters.user_mask_density
    check(f"{tag} mask density", density, want["density"])
    left, top, right, bottom = want["bbox"]
    width, height = right - left, bottom - top
    channels = dict(
        (int(info.id), data)
        for info, data in zip(layer._record.channel_info, layer._channels)
    )
    raw = channels[-2].get_data(width, height, 16, 1)
    got = np.frombuffer(raw, dtype=">u2").reshape(height, width)
    want_data = np.asarray(want["data"], dtype=np.uint16)
    if not np.array_equal(got, want_data):
        diff = int(np.max(np.abs(got.astype(np.int64) - want_data)))
        fail(f"{tag} mask samples differ by up to {diff}/65535")


def check_nodes(path, layers, nodes, counts):
    check(f"{path} member count", len(layers), len(nodes))
    for layer, want in zip(layers, nodes):
        tag = f"{path}/{want['name']}"
        check(f"{tag} name", layer.name, want["name"])
        check(f"{tag} blend", layer.blend_mode.value, want["blend"].encode("ascii"))
        check(f"{tag} opacity", layer.opacity, want["opacity"])
        fill = layer.tagged_blocks.get_data(Tag.BLEND_FILL_OPACITY)
        check(f"{tag} fill", fill, want["fill"])
        check(f"{tag} visible", layer.visible, want["visible"])
        check_mask(tag, layer, want["mask"])
        if want["kind"] == "group":
            check(f"{tag} kind", layer.kind, "group")
            divider = layer._record.tagged_blocks.get_data(Tag.SECTION_DIVIDER_SETTING)
            check(f"{tag} lsct blend", divider.blend_mode.value, want["blend"].encode("ascii"))
            counts["groups"] += 1
            check_nodes(tag, list(layer), want["children"], counts)
        else:
            check(f"{tag} kind", layer.kind, "pixel")
            check(f"{tag} left", layer.left, want["left"])
            check(f"{tag} top", layer.top, want["top"])
            check(f"{tag} size", (layer.width, layer.height), (want["width"], want["height"]))
            counts["worst"] = max(counts["worst"], check_pixels(f"{tag} pixels", layer.numpy(), want))
            counts["layers"] += 1
        if want["mask"] is not None:
            counts["masks"] += 1


def check_groups(folder):
    spec_path = folder / "groups.json"
    if not spec_path.exists():
        print("no groups.json: the group and mask pass was skipped")
        return
    spec = json.loads(spec_path.read_text(encoding="utf-8"))
    psd = PSDImage.open(folder / "groups.psd")
    check("groups width", psd.width, spec["width"])
    check("groups height", psd.height, spec["height"])
    check("groups depth", psd.depth, 16)
    counts = {"groups": 0, "layers": 0, "masks": 0, "worst": 0.0}
    check_nodes("", list(psd), spec["tree"], counts)
    print(
        f"OK: psd-tools read {counts['groups']} groups, {counts['layers']} layers and "
        f"{counts['masks']} masks (tree, props, bbox, flags, density, exact samples); "
        f"largest pixel difference {counts['worst'] * 65535.0:.3f}/65535"
    )


def check_locks(folder):
    """0.198.0: every lock combination reads back through psd-tools.

    Photoshop's own encoding: the record flags' bit 0 is transparency, the
    `lspf` block's bit 0/1/2 are transparency/composite (pixels)/position,
    and exactly 0x80000000 is Lock All (all three set in Aurora's model).
    """
    spec_path = folder / "locks.json"
    if not spec_path.exists():
        print("no locks.json: the lock pass was skipped")
        return
    spec = json.loads(spec_path.read_text(encoding="utf-8"))
    psd = PSDImage.open(folder / "locks.psd")
    by_name = {layer.name: layer for layer in psd}
    for want in spec:
        layer = by_name.get(want["name"])
        if layer is None:
            fail(f"locks: no layer {want['name']}")
        locks = layer.locks
        wanted = (want["transparency"], want["pixels"], want["position"])
        if locks is None:
            got = (False, False, False)
            if layer._record.flags.transparency_protected:
                fail(f"{want['name']}: flag bit 0 without an lspf block")
        elif locks.complete:
            got = (True, True, True)
        else:
            got = (locks.transparency, locks.composite, locks.position)
        check(f"{want['name']} locks", got, wanted)
        check(
            f"{want['name']} flag bit 0",
            bool(layer._record.flags.transparency_protected),
            want["transparency"],
        )
    print(f"OK: psd-tools read the locks of {len(spec)} layers")


def check_clips(folder):
    """0.201.0: the record clipping byte reads back as psd-tools' clipping."""
    spec_path = folder / "clips.json"
    if not spec_path.exists():
        print("no clips.json: the clipping pass was skipped")
        return
    spec = json.loads(spec_path.read_text(encoding="utf-8"))
    psd = PSDImage.open(folder / "clips.psd")
    by_name = {layer.name: layer for layer in psd}
    for want in spec:
        layer = by_name.get(want["name"])
        if layer is None:
            fail(f"clips: no layer {want['name']}")
        check(f"{want['name']} clipping", bool(layer.clipping), want["clipping"])
    print(f"OK: psd-tools read the clipping of {len(spec)} layers")


def smart_objects(psd):
    """Every smart object layer of `psd`, as (name, unique id, data) in file order."""
    out = []
    for layer in psd.descendants():
        if layer.kind == "smartobject":
            so = layer.smart_object
            out.append((layer.name, so.unique_id, so.data))
    return out


def check_smart(folder):
    """0.200.0: preserved smart objects read back as smart objects.

    `smart.psd` (synthetic, written by the Rust fixture test) must hold the
    three smart-object layers of `smart.json`, each with its embedded
    bytes. When the user's template was available, `nyepi-roundtrip.psd`
    must hold exactly the smart objects of the original (same count, same
    unique IDs, byte-identical embedded files), and its layer tree must
    have the same names.
    """
    spec_path = folder / "smart.json"
    if not spec_path.exists():
        print("no smart.json: the smart object pass was skipped")
        return
    spec = json.loads(spec_path.read_text(encoding="utf-8"))["smart"]
    got = smart_objects(PSDImage.open(folder / "smart.psd"))
    check("smart object count", len(got), len(spec))
    for (name, unique_id, data), want in zip(got, spec):
        check("smart object name", name, want["name"])
        check(f"{name} unique id", unique_id, want["unique_id"])
        check(f"{name} embedded file", bytes(data).hex(), want["data"])
    print(f"OK: psd-tools read {len(got)} smart objects with their embedded files")

    nyepi_path = folder / "nyepi.json"
    if not nyepi_path.exists():
        print("no nyepi.json: the template pass was skipped")
        return
    original = PSDImage.open(json.loads(nyepi_path.read_text())["original"])
    again = PSDImage.open(folder / "nyepi-roundtrip.psd")
    before, after = smart_objects(original), smart_objects(again)
    check("template smart object count", len(after), len(before))
    check("template smart object count is 35", len(after), 35)
    check("template layer names", [l.name for l in again.descendants()],
          [l.name for l in original.descendants()])
    for (name, uid, data), (name2, uid2, data2) in zip(before, after):
        check(f"{name} name", name2, name)
        check(f"{name} unique id", uid2, uid)
        check(f"{name} embedded file", bytes(data2) == bytes(data), True)
    print(f"OK: the template's {len(after)} smart objects survived open -> save byte for byte")


if __name__ == "__main__":
    main()
    check_groups(Path(sys.argv[1] if len(sys.argv) > 1 else "."))
    check_locks(Path(sys.argv[1] if len(sys.argv) > 1 else "."))
    check_clips(Path(sys.argv[1] if len(sys.argv) > 1 else "."))
    check_smart(Path(sys.argv[1] if len(sys.argv) > 1 else "."))
