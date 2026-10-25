#!/usr/bin/env python3
"""Draw Oxide's mark and write every icon the app and the extension ship.

The mark is a diamond cut into two halves by a caret-wide seam: the left half is
rust (`#d9643a`) and the right half is cyan (`#5fd7ff`), each held by the ink rim
(`#2d2d3a`) the mark has always worn. The halves read as the two parts of the
name — `Ox`, the language it is written in (rust is iron oxide), and `IDE`, the
tool it is used as, a code bracket either side of a cursor.

Geometry, in a 24-unit box centred on (12, 12):

  F  = 7.5    half-diagonal of each coloured fill (the fills span 62.5%)
  RI = 8.625  half-diagonal of the ink rim (the mark spans 71.875%; the rim is
              0.795 units across, the weight it carried before the split)
  S  = 0.7    half-width of the seam (1.4 units: the caret between the halves)

`icon.icns` and `icon.ico` are built from the PNGs this script writes with the
tools macOS carries (`sips`, `iconutil`), so the checked-in assets are
reproducible from one place rather than by hand:

    python3 scripts/build-icons.py

The preview sheet it writes to `target/logo-preview.png` is not checked in.
"""

from __future__ import annotations

import shutil
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ICONS = ROOT / "crates" / "desktop" / "icons"
VSCODE = ROOT / "editors" / "vscode" / "media"

RUST = "#d9643a"
CYAN = "#5fd7ff"
INK = "#2d2d3a"
TILE = "#14141a"

C = 12.0
F = 7.5
RI = 8.625
S = 0.7
TILE_RADIUS = 0.1895 * 24

# `icon.iconset` names and the pixels each holds; iconutil refuses a set that
# does not match its names exactly.
ICONSET = {
    "icon_16x16.png": 16,
    "icon_16x16@2x.png": 32,
    "icon_32x32.png": 32,
    "icon_32x32@2x.png": 64,
    "icon_128x128.png": 128,
    "icon_128x128@2x.png": 256,
    "icon_256x256.png": 256,
    "icon_256x256@2x.png": 512,
    "icon_512x512.png": 512,
    "icon_512x512@2x.png": 1024,
}

ICO_SIZES = (16, 24, 32, 48, 64, 128, 256)


def left_half(half_diagonal: float) -> list[tuple[float, float]]:
    x = C - S
    return [
        (C - half_diagonal, C),
        (x, C - (x - (C - half_diagonal))),
        (x, C + (x - (C - half_diagonal))),
    ]


def right_half(half_diagonal: float) -> list[tuple[float, float]]:
    x = C + S
    return [
        (x, C - ((C + half_diagonal) - x)),
        (C + half_diagonal, C),
        (x, C + ((C + half_diagonal) - x)),
    ]


def polygon(points: list[tuple[float, float]], fill: str) -> str:
    coords = " ".join(f"{x:.4f},{y:.4f}" for x, y in points)
    return f'<polygon points="{coords}" fill="{fill}"/>'


def mark() -> str:
    """An ink half behind each coloured fill, with the seam left open between them."""
    return "".join(
        [
            polygon(left_half(RI), INK),
            polygon(left_half(F), RUST),
            polygon(right_half(RI), INK),
            polygon(right_half(F), CYAN),
        ]
    )


def tile(fill: str = TILE) -> str:
    return f'<rect width="24" height="24" rx="{TILE_RADIUS:.4f}" fill="{fill}"/>'


def svg(body: str, size: int | None) -> str:
    attributes = f' width="{size}" height="{size}"' if size else ' width="24" height="24"'
    return f'<svg{attributes} viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">{body}</svg>\n'


def rasterise(document: str, out: Path) -> None:
    """Let the SVG renderer scale the paths itself, so each size is drawn afresh."""
    out.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        source = Path(tmp) / "icon.svg"
        source.write_text(document)
        subprocess.run(
            ["sips", "-s", "format", "png", str(source), "--out", str(out)],
            check=True,
            capture_output=True,
        )


def ico(pngs: dict[int, Path], out: Path) -> None:
    """An .ico holding PNG payloads, which every Windows this app ships to reads."""
    payloads = [(size, path.read_bytes()) for size, path in sorted(pngs.items())]
    header = struct.pack("<HHH", 0, 1, len(payloads))
    offset = len(header) + 16 * len(payloads)
    entries = b""
    for size, data in payloads:
        side = 0 if size >= 256 else size
        entries += struct.pack("<BBBBHHII", side, side, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    out.write_bytes(header + entries + b"".join(data for _, data in payloads))


def preview(out: Path) -> None:
    """A sheet of the mark at the sizes it is read at, for review rather than shipping."""
    def placed(x: float, y: float, scale: float, body: str) -> str:
        return f'<g transform="translate({x},{y}) scale({scale:.4f})">{body}</g>'

    def caption(x: int, y: int, text: str) -> str:
        return (
            f'<text x="{x}" y="{y}" font-family="-apple-system, Helvetica, sans-serif" '
            f'font-size="13" fill="#8b95a5">{text}</text>'
        )

    body = [
        '<rect width="880" height="660" fill="#101015"/>',
        placed(16, 16, 512 / 24, tile() + mark()),
        placed(560, 24, 128 / 24, tile() + mark()),
        placed(560, 192, 128 / 24, tile("#f3f3f3") + mark()),
        placed(728, 24, 32 / 24, tile() + mark()),
        placed(728, 96, 16 / 24, tile() + mark()),
        placed(728, 200, 32 / 24, mark()),
        placed(728, 272, 16 / 24, mark()),
        placed(560, 344, 5.3333, mark()),
        placed(700, 344, 1.3333, mark()),
        placed(740, 344, 0.6667, mark()),
        caption(560, 176, "128, dark and light tile"),
        caption(728, 176, "32 / 16 tiled"),
        caption(728, 316, "32 / 16 bare"),
        caption(16, 564, "Ox = rust (#d9643a): the language it is written in, iron oxide"),
        caption(16, 588, "IDE = cyan (#5fd7ff): a code bracket either side of the caret"),
        caption(16, 612, "the ink rim (#2d2d3a) holds both halves, and the diamond is the same one"),
        f'<text x="560" y="420" font-family="-apple-system, Helvetica, sans-serif" font-size="13" '
        f'fill="#8b95a5">bare mark: 128, 32, 16</text>',
    ]
    out.parent.mkdir(parents=True, exist_ok=True)
    rasterise(
        '<svg width="880" height="660" viewBox="0 0 880 660" xmlns="http://www.w3.org/2000/svg">'
        + "".join(body)
        + "</svg>",
        out,
    )


def main() -> int:
    for tool in ("sips", "iconutil"):
        if shutil.which(tool) is None:
            print(f"{tool} is missing: icon sets are built on macOS", file=sys.stderr)
            return 1

    with tempfile.TemporaryDirectory() as tmp:
        scratch = Path(tmp)
        sizes: dict[int, Path] = {}
        for size in sorted(set(ICONSET.values()) | set(ICO_SIZES) | {512}):
            path = scratch / f"tile-{size}.png"
            rasterise(svg(tile() + mark(), size), path)
            sizes[size] = path

        VSCODE.mkdir(parents=True, exist_ok=True)
        (VSCODE / "oxide.svg").write_text(svg(mark(), None))

        iconset = ICONS / "icon.iconset"
        if iconset.exists():
            shutil.rmtree(iconset)
        iconset.mkdir(parents=True)
        for name, size in ICONSET.items():
            shutil.copyfile(sizes[size], iconset / name)
        subprocess.run(
            ["iconutil", "-c", "icns", "-o", str(ICONS / "icon.icns"), str(iconset)],
            check=True,
            capture_output=True,
        )

        for name, size in {"icon.png": 512, "32x32.png": 32, "128x128.png": 128, "128x128@2x.png": 256}.items():
            shutil.copyfile(sizes[size], ICONS / name)
        shutil.copyfile(ICONS / "128x128.png", VSCODE / "icon.png")
        ico({size: sizes[size] for size in ICO_SIZES}, ICONS / "icon.ico")

    preview(ROOT / "target" / "logo-preview.png")
    print("wrote crates/desktop/icons/{icon.png,32x32.png,128x128.png,128x128@2x.png,icon.icns,icon.ico,icon.iconset}")
    print("wrote editors/vscode/media/{oxide.svg,icon.png}")
    print("preview: target/logo-preview.png")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
