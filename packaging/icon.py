#!/usr/bin/env python3
"""Draw the IRCord icon (a blurple disc with a white "#", the tray icon's
design) and write every packaged copy of it. Standard library only.

    python3 packaging/icon.py

writes packaging/linux/ircord.svg, packaging/linux/ircord.png (256 px),
packaging/windows/ircord.ico (16 to 256 px) and packaging/macos/ircord.icns
(32 to 1024 px). Commit the results.
"""
import os
import struct
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
BLURPLE = (88, 101, 242)
WHITE = (255, 255, 255)
# The design on a 32-unit grid, as in crates/app/src/tray.rs.
BARS = [(11, 7, 3, 18), (18, 7, 3, 18), (7, 11, 18, 3), (7, 18, 18, 3)]


def colour_at(x, y):
    """The colour at a point of the 32-unit grid, or None outside the disc."""
    if (x - 16) ** 2 + (y - 16) ** 2 > 16 * 16:
        return None
    for bx, by, bw, bh in BARS:
        if bx <= x < bx + bw and by <= y < by + bh:
            return WHITE
    return BLURPLE


def render(size, samples=4):
    """RGBA rows, anti-aliased by supersampling."""
    rows = []
    for py in range(size):
        row = bytearray()
        for px in range(size):
            r = g = b = a = 0
            for sy in range(samples):
                for sx in range(samples):
                    x = (px + (sx + 0.5) / samples) * 32 / size
                    y = (py + (sy + 0.5) / samples) * 32 / size
                    c = colour_at(x, y)
                    if c:
                        r, g, b, a = r + c[0], g + c[1], b + c[2], a + 1
            n = samples * samples
            if a:
                row += bytes((r // a, g // a, b // a, 255 * a // n))
            else:
                row += bytes(4)
        rows.append(bytes(row))
    return rows


def png(size, samples=4):
    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    raw = b"".join(b"\x00" + row for row in render(size, samples))
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def ico(sizes):
    """An .ico whose images are PNGs, which Windows Vista and later read."""
    images = [png(s) for s in sizes]
    out = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    for s, data in zip(sizes, images):
        dim = 0 if s >= 256 else s
        out += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    return out + b"".join(images)


def icns():
    """An .icns whose images are PNGs (macOS 10.7 and later)."""
    kinds = [(b"ic11", 32), (b"ic12", 64), (b"ic07", 128), (b"ic08", 256),
             (b"ic09", 512), (b"ic10", 1024)]
    body = b""
    for kind, size in kinds:
        data = png(size, samples=4 if size <= 256 else 2)
        body += kind + struct.pack(">I", 8 + len(data)) + data
    return b"icns" + struct.pack(">I", 8 + len(body)) + body


def svg():
    bars = "".join(
        f'<rect x="{x}" y="{y}" width="{w}" height="{h}"/>' for x, y, w, h in BARS
    )
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">'
        '<circle cx="16" cy="16" r="16" fill="#5865f2"/>'
        f'<g fill="#fff">{bars}</g></svg>\n'
    )


def write(path, data):
    mode = "w" if isinstance(data, str) else "wb"
    with open(os.path.join(HERE, path), mode) as f:
        f.write(data)


write("linux/ircord.svg", svg())
write("linux/ircord.png", png(256))
write("windows/ircord.ico", ico([16, 24, 32, 48, 64, 128, 256]))
write("macos/ircord.icns", icns())
