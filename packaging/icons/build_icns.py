#!/usr/bin/env python3
"""Build a macOS .icns from a set of PNG sizes.

Apple's .icns format:
- 8-byte file header: 'icns' magic + 4-byte big-endian total file size
- Followed by chunks: 4-byte type code + 4-byte big-endian chunk size
  (which INCLUDES the 8-byte chunk header) + payload (raw PNG for the
  modern type codes used here).

Type codes used (modern macOS only — we don't ship pre-Big-Sur):
- ic07 → 128x128       (1x)
- ic08 → 256x256       (1x)
- ic09 → 512x512       (1x)
- ic10 → 1024x1024     (512@2x)
- ic11 → 32x32         (16@2x)
- ic12 → 64x64         (32@2x)
- ic13 → 256x256       (128@2x)
- ic14 → 512x512       (256@2x)
"""
import os
import struct
import sys

ICONS = [
    ("ic07", "icon-128.png"),
    ("ic08", "icon-256.png"),
    ("ic09", "icon-512.png"),
    ("ic10", "icon-1024.png"),
    ("ic11", "icon-32.png"),
    ("ic12", "icon-64.png"),
    ("ic13", "icon-256.png"),
    ("ic14", "icon-512.png"),
]


def main(src_dir: str, dst: str) -> None:
    chunks = bytearray()
    for type_code, fname in ICONS:
        path = os.path.join(src_dir, fname)
        with open(path, "rb") as f:
            png = f.read()
        chunk_size = 8 + len(png)
        chunks.extend(type_code.encode("ascii"))
        chunks.extend(struct.pack(">I", chunk_size))
        chunks.extend(png)
    total = 8 + len(chunks)
    with open(dst, "wb") as out:
        out.write(b"icns")
        out.write(struct.pack(">I", total))
        out.write(chunks)
    print(f"wrote {dst} ({total} bytes, {len(ICONS)} variants)")


if __name__ == "__main__":
    src = sys.argv[1] if len(sys.argv) > 1 else "."
    dst = sys.argv[2] if len(sys.argv) > 2 else "kino.icns"
    main(src, dst)
