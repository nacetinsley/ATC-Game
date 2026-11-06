"""Draw the app icon: a green radar scope with a sweep and two targets.

Writes PNGs at the sizes the macOS app bundle uses (cargo-bundle turns them
into an .icns). Pure Python, no image libraries needed.

Usage, from the repository root:

    python tools/make_icon.py
"""

import math
import struct
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "assets" / "icon"
SIZES = [32, 64, 128, 256, 512]
# Supersampling per axis, for smooth edges.
SS = 4

BACKGROUND = (10, 18, 14)
GREEN = (80, 230, 110)
DIM = (40, 120, 60)


def coverage(size):
    """Per-pixel colour, averaged over SS x SS samples."""
    pixels = []
    for py in range(size):
        row = []
        for px in range(size):
            acc = [0.0, 0.0, 0.0, 0.0]
            for sy in range(SS):
                for sx in range(SS):
                    # Unit coordinates, centre at 0, edge at 1.
                    x = ((px + (sx + 0.5) / SS) / size) * 2 - 1
                    y = ((py + (sy + 0.5) / SS) / size) * 2 - 1
                    r, g, b, a = sample(x, y, 2.0 / size)
                    acc[0] += r * a
                    acc[1] += g * a
                    acc[2] += b * a
                    acc[3] += a
            n = SS * SS
            a = acc[3] / n
            if a > 0:
                row.append(tuple(int(acc[i] / acc[3]) for i in range(3)) + (int(a * 255),))
            else:
                row.append((0, 0, 0, 0))
        pixels.append(row)
    return pixels


def sample(x, y, pixel):
    """Colour and alpha at a point. `pixel` is one pixel in unit coordinates."""
    # Rounded square, as macOS icons are, with a margin.
    margin, radius = 0.10, 0.36
    half = 1 - margin
    qx, qy = abs(x) - (half - radius), abs(y) - (half - radius)
    outside = math.hypot(max(qx, 0), max(qy, 0)) - radius + min(max(qx, qy), 0)
    if outside > 0:
        return (0, 0, 0, 0)

    d = math.hypot(x, y)
    colour = BACKGROUND
    line = max(0.018, pixel * 1.2)

    # Sweep: a fading wedge behind the beam, pointing up and to the right.
    beam = math.radians(-40)
    angle = math.atan2(y, x)
    behind = (beam - angle) % (2 * math.pi)
    if d < 0.72 and behind < math.radians(70):
        fade = 1 - behind / math.radians(70)
        colour = mix(colour, GREEN, 0.35 * fade * fade)

    # Range rings.
    for ring in (0.24, 0.48, 0.72):
        if abs(d - ring) < line / 2:
            colour = mix(colour, DIM, 1.0)

    # Beam line.
    bx, by = math.cos(beam), math.sin(beam)
    along = x * bx + y * by
    across = abs(-x * by + y * bx)
    if 0 < along < 0.72 and across < line * 0.8:
        colour = GREEN

    # Targets.
    for tx, ty, size in ((0.30, -0.42, 0.055), (-0.38, 0.18, 0.045)):
        if math.hypot(x - tx, y - ty) < size:
            colour = GREEN
    return colour + (1.0,)


def mix(a, b, t):
    return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(3))


def write_png(path, pixels):
    size = len(pixels)
    raw = b"".join(b"\x00" + bytes(c for px in row for c in px) for row in pixels)

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    header = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")
    path.write_bytes(png)


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    for size in SIZES:
        path = OUT / f"icon_{size}.png"
        write_png(path, coverage(size))
        print(f"wrote {path.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
