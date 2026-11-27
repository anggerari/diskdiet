#!/usr/bin/env python3
"""Draw the Disk Diet app icon (no dependencies beyond the standard library).

The mark is the same as docs/logo-*.svg: storage as a donut chart, with the
slice Disk Diet gives back pulled out and faded. It sits on a dark macOS-style
rounded square. Shapes are rendered with signed distance fields, so edges are
anti-aliased at every size.

    python3 scripts/make-icon.py          # writes assets/icon-1024.png, assets/icon-256.png
    (macOS) also builds assets/DiskDiet.icns with sips + iconutil
"""

import math
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import zlib

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
ASSETS = os.path.join(ROOT, "assets")

ACCENT = (0x59, 0xC2, 0x7F)
BG_TOP = (0x26, 0x28, 0x2E)
BG_BOTTOM = (0x14, 0x15, 0x18)


def png(path, size, pixels):
    raw = b"".join(b"\x00" + bytes(pixels[y * size * 4:(y + 1) * size * 4]) for y in range(size))

    def chunk(kind, data):
        c = struct.pack(">I", len(data)) + kind + data
        return c + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)))
        f.write(chunk(b"IDAT", zlib.compress(raw, 9)))
        f.write(chunk(b"IEND", b""))


def rounded_rect_sdf(px, py, cx, cy, half, radius):
    qx = abs(px - cx) - half + radius
    qy = abs(py - cy) - half + radius
    outside = math.hypot(max(qx, 0.0), max(qy, 0.0))
    return outside + min(max(qx, qy), 0.0) - radius


def cross(angle_deg, x, y):
    a = math.radians(angle_deg)
    return math.cos(a) * y - math.sin(a) * x


def sector_sdf(x, y, start, end):
    """Signed distance to the wedge between two rays (span < 180°); <0 inside."""
    return max(-cross(start, x, y), cross(end, x, y))


def arc_sdf(x, y, r, w, start, end):
    d = math.hypot(x, y)
    radial = abs(d - r) - w / 2
    if end - start <= 180:
        ang = sector_sdf(x, y, start, end)
    else:  # the complement of the gap wedge
        ang = -sector_sdf(x, y, end - 360, start)
    return max(radial, ang)


def cover(sdf):
    return min(max(0.5 - sdf, 0.0), 1.0)


def over(dst, color, alpha):
    a = alpha + dst[3] * (1 - alpha)
    if a == 0:
        return [0, 0, 0, 0]
    return [
        (color[i] * alpha + dst[i] * dst[3] * (1 - alpha)) / a for i in range(3)
    ] + [a]


def render(size):
    s = size / 1024.0
    # Apple's icon grid: an 824px body centered in a 1024px canvas.
    half, radius, center = 412 * s, 185 * s, 512 * s
    # Logo geometry (104x104 viewBox) scaled into the body.
    k = 6.0 * s
    ox, oy = 505 * s - 52 * k, 520 * s - 52 * k
    ring_c = (ox + 52 * k, oy + 52 * k)
    off = (7 * math.cos(math.radians(-70)), 7 * math.sin(math.radians(-70)))
    slice_c = (ox + (52 + off[0]) * k, oy + (52 + off[1]) * k)
    r, w = 35 * k, 18 * k
    faded = tuple(int(c * 0.45 + b * 0.55) for c, b in zip(ACCENT, BG_TOP))

    out = bytearray(size * size * 4)
    for y in range(size):
        py = y + 0.5
        t = y / (size - 1)
        bg = tuple(BG_TOP[i] * (1 - t) + BG_BOTTOM[i] * t for i in range(3))
        for x in range(size):
            px = x + 0.5
            a_bg = cover(rounded_rect_sdf(px, py, center, center, half, radius))
            if a_bg == 0:
                continue
            pix = [bg[0], bg[1], bg[2], a_bg]
            # Hairline highlight along the top edge of the body.
            edge = rounded_rect_sdf(px, py, center, center, half, radius)
            if -2 * s < edge <= 0 and py < center:
                pix = over(pix, (255, 255, 255), 0.06 * a_bg)
            a = cover(arc_sdf(px - slice_c[0], py - slice_c[1], r, w, -105, -35))
            if a:
                pix = over(pix, faded, a * a_bg)
            a = cover(arc_sdf(px - ring_c[0], py - ring_c[1], r, w, -25, 245))
            if a:
                pix = over(pix, ACCENT, a * a_bg)
            i = (y * size + x) * 4
            out[i:i + 4] = bytes(int(round(v)) for v in pix[:3]) + bytes([int(round(pix[3] * 255))])
    return out


def main():
    os.makedirs(ASSETS, exist_ok=True)
    for size in (1024, 256):
        path = os.path.join(ASSETS, f"icon-{size}.png")
        png(path, size, render(size))
        print("wrote", os.path.relpath(path, ROOT))

    if sys.platform == "darwin" and shutil.which("iconutil"):
        src = os.path.join(ASSETS, "icon-1024.png")
        with tempfile.TemporaryDirectory() as tmp:
            iconset = os.path.join(tmp, "DiskDiet.iconset")
            os.makedirs(iconset)
            for base in (16, 32, 128, 256, 512):
                for scale in (1, 2):
                    px = base * scale
                    name = f"icon_{base}x{base}{'@2x' if scale == 2 else ''}.png"
                    subprocess.run(
                        ["sips", "-z", str(px), str(px), src, "--out", os.path.join(iconset, name)],
                        check=True,
                        capture_output=True,
                    )
            out = os.path.join(ASSETS, "DiskDiet.icns")
            subprocess.run(["iconutil", "-c", "icns", iconset, "-o", out], check=True)
            print("wrote", os.path.relpath(out, ROOT))


if __name__ == "__main__":
    main()
