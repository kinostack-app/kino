#!/usr/bin/env python3
"""Build the MSIX tile assets for kino's Microsoft Store package.

MSIX requires specific image sizes for tile rendering on Start, search,
and the Store listing. We ship the minimum-viable scale set:

  Square44x44Logo.scale-100.png   ( 44x 44)  small list/search icon
  Square44x44Logo.scale-200.png   ( 88x 88)  ditto, HiDPI
  Square150x150Logo.scale-100.png (150x150)  Start medium tile
  Square150x150Logo.scale-200.png (300x300)  ditto, HiDPI
  StoreLogo.scale-100.png         ( 50x 50)  Store listing thumbnail

Source: packaging/icons/linux/kino-512.png (the highest-res square
PNG in the repo). Resized via ImageMagick `convert -resize -strip`.
ImageMagick (`/usr/bin/convert`) ships in the dev container and on
GitHub `windows-latest` / `ubuntu-latest` runners, so no new dep.

Run from the repo root:
    python3 packaging/icons/build_msix_tiles.py

This regenerates every tile in
`backend/crates/kino/msix/Assets/`. Output is deterministic; commit
the tiles alongside changes.
"""
import os
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
SRC = REPO_ROOT / "packaging" / "icons" / "linux" / "kino-512.png"
DST_DIR = REPO_ROOT / "backend" / "crates" / "kino" / "msix" / "Assets"

# (output filename, edge length in px)
TILES = [
    ("Square44x44Logo.scale-100.png", 44),
    ("Square44x44Logo.scale-200.png", 88),
    ("Square150x150Logo.scale-100.png", 150),
    ("Square150x150Logo.scale-200.png", 300),
    ("StoreLogo.scale-100.png", 50),
]


def render(size: int, out: Path) -> None:
    # `-strip` removes EXIF/colour-profile metadata so the rendered
    # PNGs are byte-stable across runs (deterministic git diffs).
    # `-filter Lanczos` for high-quality downscale; default is Mitchell
    # which over-softens small icons.
    cmd = [
        "convert",
        str(SRC),
        "-filter", "Lanczos",
        "-resize", f"{size}x{size}",
        "-strip",
        str(out),
    ]
    subprocess.run(cmd, check=True)


def main() -> int:
    if not SRC.exists():
        print(f"error: source missing: {SRC}", file=sys.stderr)
        return 1
    DST_DIR.mkdir(parents=True, exist_ok=True)
    for name, size in TILES:
        out = DST_DIR / name
        render(size, out)
        print(f"wrote {out.relative_to(REPO_ROOT)} ({size}x{size})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
