#!/usr/bin/env python3
"""Generate large synthetic test documents (python3 stdlib only).

Writes, for each requested size, an 8-bit RGB layered PSD (raw, uncompressed
channel data, so the generator is fast and the file is the same for every
reader) and a flat 8-bit RGBA PNG. Neither tool wrote these files, so neither
tool's own writer is favoured.

Layers of the PSD, bottom to top:
  1. "Background" - opaque, full canvas, diagonal ramps, Normal.
  2. "Multiply"   - full canvas, alpha varying across the canvas, Multiply, 100 %.
  3. "Screen"     - the centre half of the canvas only, opaque, Screen, 70 %.
The merged (compatibility) image is the background layer only: it is NOT a
reference composite, and the harness marks synthetic files as having none.

Usage: gen_synthetic.py OUT_DIR [--sizes 24,60] (megapixels, square-ish 3:2)
"""

import argparse
import math
import os
import struct
import zlib


def dims_for(mp):
    # 3:2 landscape, like a camera frame.
    h = int(math.sqrt(mp * 1_000_000 / 1.5))
    w = int(h * 1.5)
    return w, h


def ramp(n, phase=0, scale=1):
    return bytes(((x * 255 * scale) // max(n - 1, 1) + phase) & 0xFF for x in range(n))


def rotate(row, s):
    s %= len(row)
    return row[s:] + row[:s]


class Channel:
    """A channel as a callable row source: row(y) -> bytes of width w."""

    def __init__(self, w, base, step):
        self.w = w
        self.base = base
        self.step = step

    def row(self, y):
        return rotate(self.base, y * self.step)


def layer_channels(w, h):
    bg = [Channel(w, ramp(w, 0), 1), Channel(w, ramp(w, 85, 2), 3), Channel(w, ramp(w, 170), 7)]
    mul = [Channel(w, ramp(w, 200), 5), Channel(w, ramp(w, 40, 3), 2), Channel(w, ramp(w, 120), 11)]
    mul_alpha = Channel(w, ramp(w, 0), 13)
    sw, sh = w // 2, h // 2
    scr = [Channel(sw, ramp(sw, 30), 9), Channel(sw, ramp(sw, 140, 2), 4), Channel(sw, ramp(sw, 60), 6)]
    return bg, mul, mul_alpha, scr, (sw, sh)


def pascal(name):
    b = name.encode("ascii")
    s = bytes([len(b)]) + b
    while len(s) % 4:
        s += b"\0"
    return s


def write_psd(path, w, h):
    bg, mul, mul_alpha, scr, (sw, sh) = layer_channels(w, h)
    top, left = (h - sh) // 2, (w - sw) // 2
    # (name, rect(top,left,bottom,right), [(channel id, source)], blend, opacity)
    layers = [
        ("Background", (0, 0, h, w), [(0, bg[0]), (1, bg[1]), (2, bg[2])], b"norm", 255),
        ("Multiply", (0, 0, h, w), [(-1, mul_alpha), (0, mul[0]), (1, mul[1]), (2, mul[2])], b"mul ", 255),
        ("Screen", (top, left, top + sh, left + sw), [(0, scr[0]), (1, scr[1]), (2, scr[2])], b"scrn", 178),
    ]
    records = b""
    for name, (t, l, b, r), chans, blend, opacity in layers:
        lh, lw = b - t, r - l
        rec = struct.pack(">iiiiH", t, l, b, r, len(chans))
        for cid, _src in chans:
            rec += struct.pack(">hI", cid, 2 + lw * lh)
        rec += b"8BIM" + blend + struct.pack(">BBBB", opacity, 0, 0, 0)
        extra = struct.pack(">I", 0) + struct.pack(">I", 0) + pascal(name)
        rec += struct.pack(">I", len(extra)) + extra
        records += rec
    layer_info_len_without_pixels = 2 + len(records)
    pixel_bytes = 0
    for _name, (t, l, b, r), chans, _blend, _op in layers:
        pixel_bytes += len(chans) * (2 + (r - l) * (b - t))
    layer_info_len = layer_info_len_without_pixels + pixel_bytes
    if layer_info_len % 2:
        layer_info_len += 1
    with open(path, "wb") as f:
        f.write(b"8BPS" + struct.pack(">H", 1) + b"\0" * 6 + struct.pack(">HIIHH", 3, h, w, 8, 3))
        f.write(struct.pack(">I", 0))  # colour mode data
        f.write(struct.pack(">I", 0))  # image resources
        f.write(struct.pack(">I", 4 + layer_info_len + 4))
        f.write(struct.pack(">I", layer_info_len))
        f.write(struct.pack(">h", len(layers)))
        f.write(records)
        written = 2 + len(records)
        for _name, (t, l, b, r), chans, _blend, _op in layers:
            for _cid, src in chans:
                f.write(struct.pack(">H", 0))
                for y in range(b - t):
                    f.write(src.row(y))
                written += 2 + (r - l) * (b - t)
        if written % 2:
            f.write(b"\0")
        f.write(struct.pack(">I", 0))  # global layer mask info
        # Merged image: the background only (raw).
        f.write(struct.pack(">H", 0))
        for c in bg:
            for y in range(h):
                f.write(c.row(y))


def write_png(path, w, h):
    bg, mul, mul_alpha, _scr, _ = layer_channels(w, h)

    def chunk(t, body):
        return struct.pack(">I", len(body)) + t + body + struct.pack(">I", zlib.crc32(t + body) & 0xFFFFFFFF)

    comp = zlib.compressobj(1)
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)))
        out = bytearray()
        for y in range(h):
            r, g, b, a = bg[0].row(y), mul[1].row(y), bg[2].row(y), mul_alpha.row(y)
            line = bytearray(w * 4)
            line[0::4] = r
            line[1::4] = g
            line[2::4] = b
            line[3::4] = bytes(min(255, 64 + v) for v in a) if y % 64 == 0 else a
            out += comp.compress(b"\0" + bytes(line))
            if len(out) > (64 << 20):
                f.write(chunk(b"IDAT", bytes(out)))
                out = bytearray()
        out += comp.flush()
        f.write(chunk(b"IDAT", bytes(out)))
        f.write(chunk(b"IEND", b""))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out_dir")
    ap.add_argument("--sizes", default="24,60")
    args = ap.parse_args()
    os.makedirs(args.out_dir, exist_ok=True)
    for mp in [int(s) for s in args.sizes.split(",") if s]:
        w, h = dims_for(mp)
        psd = os.path.join(args.out_dir, "synthetic-%dmp-layers.psd" % mp)
        png = os.path.join(args.out_dir, "synthetic-%dmp-flat.png" % mp)
        if not os.path.exists(psd):
            write_psd(psd + ".tmp", w, h)
            os.replace(psd + ".tmp", psd)
        if not os.path.exists(png):
            write_png(png + ".tmp", w, h)
            os.replace(png + ".tmp", png)
        print("%s %dx%d" % (psd, w, h))
        print("%s %dx%d" % (png, w, h))


if __name__ == "__main__":
    main()
