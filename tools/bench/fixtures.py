#!/usr/bin/env python3
"""Prepares the basemap fixture of the benchmarks (README.md):

    fixtures.py <directory>

Downloads the files that fixtures.json pins into <directory>, unless they are
there with the pinned digests, extracts the pinned font's glyph ranges, and
writes the sprite. Only this step uses the network; the benchmarks read the
directory.

fixtures.json and style.json are maplibre-native-ffi's (benchmarks/), so both
harnesses draw the same data, and this is its preparation, ported.
"""

from __future__ import annotations

import colorsys
import concurrent.futures
import hashlib
import json
import math
import os
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import urllib.request
import zlib
from pathlib import Path, PurePosixPath

MANIFEST = Path(__file__).resolve().parent / "fixtures.json"
GLYPH_RANGE = re.compile(r"[0-9]+-[0-9]+\.pbf")


def log(message: str) -> None:
    print(f"fixtures: {message}", file=sys.stderr, flush=True)


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def fetch(url: str, target: Path, expected: str) -> None:
    request = urllib.request.Request(url, headers={"User-Agent": "mln-rs-bench"})
    with urllib.request.urlopen(request, timeout=60) as response:
        data = response.read()
    actual = hashlib.sha256(data).hexdigest()
    if actual != expected:
        raise SystemExit(f"{url} has sha256 {actual}, but fixtures.json pins {expected}")
    target.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=target.parent, delete=False) as temporary:
        temporary.write(data)
    os.replace(temporary.name, target)


def prepare_glyphs(glyphs: dict, directory: Path) -> None:
    """Extracts one font's glyph ranges from the pinned archive, unless a stamp
    says this archive's are there. The archive is untrusted, so only plain range
    files under the font's directory are kept."""
    font = glyphs["font"]
    stamp = directory / font / ".sha256"
    if stamp.is_file() and stamp.read_text().strip() == glyphs["sha256"]:
        return
    log(f"downloading {glyphs['url']}")
    directory.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=directory) as temporary:
        archive = Path(temporary) / "fonts.tar.gz"
        fetch(glyphs["url"], archive, glyphs["sha256"])
        names, lines = (
            subprocess.run(["tar", flags, str(archive)], check=True, stdout=subprocess.PIPE)
            .stdout.decode()
            .splitlines()
            for flags in ("-tzf", "-tvzf")
        )
        # Refuses paths that leave the extraction directory, and links where
        # the font's files go.
        for name, line in zip(names, lines, strict=True):
            path = PurePosixPath(name)
            link = path.parts[:1] == (font,) and line[:1] not in ("-", "d")
            if path.is_absolute() or ".." in path.parts or ":" in name or link:
                raise SystemExit(f"{glyphs['url']} holds an unsafe entry: {name}")
        extracted = Path(temporary) / "extracted"
        extracted.mkdir()
        subprocess.run(["tar", "-xzf", str(archive), "-C", str(extracted), font], check=True)
        target = directory / font
        if target.exists():
            shutil.rmtree(target)
        target.mkdir(parents=True)
        for path in (extracted / font).iterdir():
            if path.is_file() and not path.is_symlink() and GLYPH_RANGE.fullmatch(path.name):
                shutil.copyfile(path, target / path.name)
    stamp.write_text(glyphs["sha256"] + "\n")


# The style's icons, which the snapshot lacks: one marker of its own color per
# name, so that sprite loading, the icon atlas and icon collision run as they
# do for a basemap's icons.
SPRITE_ICONS = (
    "food", "cafe", "bar", "shop", "school", "health", "bank", "parking",
    "bus", "rail", "park", "worship", "culture", "lodging", "toilets", "poi",
)  # fmt: skip
SPRITE_ICON_SIZE = 17


def encode_png(width: int, height: int, pixels: bytes) -> bytes:
    """An 8-bit RGBA PNG of unfiltered rows."""

    def chunk(kind: bytes, data: bytes) -> bytes:
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    stride = width * 4
    rows = b"".join(b"\x00" + pixels[y * stride : (y + 1) * stride] for y in range(height))
    header = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(rows, 9))
        + chunk(b"IEND", b"")
    )


def write_sprite(directory: Path) -> None:
    """Writes each icon as a colored disc in a white ring."""
    columns, size = 4, SPRITE_ICON_SIZE
    width = columns * size
    height = math.ceil(len(SPRITE_ICONS) / columns) * size
    pixels = bytearray(width * height * 4)
    center = (size - 1) / 2
    index = {}
    for number, name in enumerate(SPRITE_ICONS):
        hue = number / len(SPRITE_ICONS)
        color = bytes(round(channel * 255) for channel in colorsys.hsv_to_rgb(hue, 0.7, 0.8))
        left, top = (number % columns) * size, (number // columns) * size
        for y in range(size):
            for x in range(size):
                distance = math.hypot(x - center, y - center) / center
                if distance <= 1.0:
                    at = ((top + y) * width + left + x) * 4
                    pixel = color if distance <= 0.72 else b"\xff\xff\xff"
                    pixels[at : at + 4] = pixel + b"\xff"
        index[name] = {"x": left, "y": top, "width": size, "height": size, "pixelRatio": 1}
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "sprite.png").write_bytes(encode_png(width, height, bytes(pixels)))
    (directory / "sprite.json").write_text(json.dumps(index, indent=2) + "\n")


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: fixtures.py <directory>")
    directory = Path(sys.argv[1])
    manifest = json.loads(MANIFEST.read_text())
    missing = {
        name: digest
        for name, digest in manifest["files"].items()
        if not (directory / name).is_file() or sha256(directory / name) != digest
    }
    if missing:
        log(f"downloading {len(missing)} files into {directory}")
        with concurrent.futures.ThreadPoolExecutor(8) as pool:
            futures = [
                pool.submit(fetch, f"{manifest['base_url']}/{name}", directory / name, digest)
                for name, digest in missing.items()
            ]
            for future in futures:
                future.result()
    prepare_glyphs(manifest["glyphs"], directory / "basemap" / "glyphs")
    write_sprite(directory / "basemap")
    (directory / "version").write_text(manifest["version"] + "\n")


if __name__ == "__main__":
    main()
