"""A small, dependency-free PNG decoder (python3 stdlib only).

Decodes non-interlaced 8- and 16-bit greyscale, grey+alpha, RGB and RGBA
PNGs (and 8-bit palette images) to a flat ``bytearray`` of 8-bit RGBA
values, so two tools' flattened outputs can be compared pixel for pixel
without trusting either tool's own decoder. 16-bit samples are reduced to
8 bits by rounding (``(v * 255 + 32767) // 65535``).

It is slow (pure Python unfiltering, a few MB/s for Paeth rows), so the
harness decodes each tool's output once per input file and checks the
other runs' outputs by SHA-256 only.
"""

import struct
import zlib


class PngError(Exception):
    pass


def _paeth(a, b, c):
    p = a + b - c
    pa = abs(p - a)
    pb = abs(p - b)
    pc = abs(p - c)
    if pa <= pb and pa <= pc:
        return a
    if pb <= pc:
        return b
    return c


def _unfilter(raw, width, height, bpp, stride):
    out = bytearray(height * stride)
    prev = bytearray(stride)
    pos = 0
    for y in range(height):
        if pos >= len(raw):
            raise PngError("truncated image data")
        ftype = raw[pos]
        pos += 1
        line = bytearray(raw[pos:pos + stride])
        pos += stride
        if len(line) != stride:
            raise PngError("truncated scanline")
        if ftype == 0:
            pass
        elif ftype == 1:
            for i in range(bpp, stride):
                line[i] = (line[i] + line[i - bpp]) & 0xFF
        elif ftype == 2:
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 0xFF
        elif ftype == 3:
            for i in range(stride):
                left = line[i - bpp] if i >= bpp else 0
                line[i] = (line[i] + ((left + prev[i]) >> 1)) & 0xFF
        elif ftype == 4:
            for i in range(stride):
                if i >= bpp:
                    a = line[i - bpp]
                    c = prev[i - bpp]
                else:
                    a = 0
                    c = 0
                line[i] = (line[i] + _paeth(a, prev[i], c)) & 0xFF
        else:
            raise PngError("bad filter type %d" % ftype)
        out[y * stride:(y + 1) * stride] = line
        prev = line
    return out


def read_png_rgba8(path):
    """Return ``(width, height, info, rgba)``; ``rgba`` holds 4 bytes a pixel."""
    with open(path, "rb") as f:
        data = f.read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise PngError("not a PNG")
    pos = 8
    ihdr = None
    idat = []
    palette = None
    trns = None
    while pos + 8 <= len(data):
        (length,) = struct.unpack(">I", data[pos:pos + 4])
        ctype = data[pos + 4:pos + 8]
        body = data[pos + 8:pos + 8 + length]
        pos += 12 + length
        if ctype == b"IHDR":
            ihdr = struct.unpack(">IIBBBBB", body)
        elif ctype == b"IDAT":
            idat.append(body)
        elif ctype == b"PLTE":
            palette = body
        elif ctype == b"tRNS":
            trns = body
        elif ctype == b"IEND":
            break
    if ihdr is None:
        raise PngError("no IHDR")
    width, height, depth, colour, _comp, _filt, interlace = ihdr
    if interlace != 0:
        raise PngError("interlaced PNG not supported")
    channels = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}.get(colour)
    if channels is None:
        raise PngError("colour type %d" % colour)
    if depth not in (8, 16) or (colour == 3 and depth != 8):
        raise PngError("bit depth %d (colour type %d) not supported" % (depth, colour))
    bps = depth // 8
    bpp = channels * bps
    stride = width * bpp
    raw = zlib.decompress(b"".join(idat))
    px = _unfilter(raw, width, height, bpp, stride)
    if depth == 16:
        hi = px[0::2]
        lo = px[1::2]
        px = bytearray(((h << 8 | l) * 255 + 32767) // 65535 for h, l in zip(hi, lo))
    n = width * height
    rgba = bytearray(n * 4)
    if colour == 6:
        rgba = px
    elif colour == 2:
        rgba[0::4] = px[0::3]
        rgba[1::4] = px[1::3]
        rgba[2::4] = px[2::3]
        rgba[3::4] = b"\xff" * n
    elif colour == 0:
        rgba[0::4] = px
        rgba[1::4] = px
        rgba[2::4] = px
        rgba[3::4] = b"\xff" * n
    elif colour == 4:
        g = px[0::2]
        rgba[0::4] = g
        rgba[1::4] = g
        rgba[2::4] = g
        rgba[3::4] = px[1::2]
    elif colour == 3:
        if palette is None:
            raise PngError("palette image without PLTE")
        for i, idx in enumerate(px):
            o = i * 4
            rgba[o:o + 3] = palette[idx * 3:idx * 3 + 3]
            rgba[o + 3] = trns[idx] if trns is not None and idx < len(trns) else 255
    info = {"bit_depth": depth, "colour_type": colour}
    return width, height, info, rgba


def compare_rgba8(a, b, threshold=2):
    """Per-channel mean/max |a - b| and the share of pixels with any channel > threshold.

    Colour channels are compared on alpha-premultiplied values (rounded to
    8 bits) so a fully transparent pixel's arbitrary colour does not count;
    the alpha channel is compared as stored.
    """
    if len(a) != len(b):
        raise PngError("sizes differ")
    n = len(a) // 4
    sums = [0, 0, 0, 0]
    maxs = [0, 0, 0, 0]
    over = 0
    for i in range(0, len(a), 4):
        aa = a[i + 3]
        ba = b[i + 3]
        worst = 0
        for c in range(3):
            va = (a[i + c] * aa + 127) // 255
            vb = (b[i + c] * ba + 127) // 255
            d = va - vb if va >= vb else vb - va
            sums[c] += d
            if d > maxs[c]:
                maxs[c] = d
            if d > worst:
                worst = d
        d = aa - ba if aa >= ba else ba - aa
        sums[3] += d
        if d > maxs[3]:
            maxs[3] = d
        if d > worst:
            worst = d
        if worst > threshold:
            over += 1
    if n == 0:
        return {"mean": [0, 0, 0, 0], "max": [0, 0, 0, 0], "share_over_2": 0.0, "pixels": 0}
    return {
        "mean": [round(s / n, 4) for s in sums],
        "max": maxs,
        "share_over_2": round(over / n, 6),
        "pixels": n,
    }
