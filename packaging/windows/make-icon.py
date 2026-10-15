"""Writes beardictate.ico from design/logo/mark.svg: PNG-compressed entries
(Windows Vista and later read them), so it stays small. Needs rsvg-convert.

    python3 packaging/windows/make-icon.py
"""
import pathlib
import struct
import subprocess

here = pathlib.Path(__file__).resolve().parent
svg = here.parent.parent / "design" / "logo" / "mark.svg"
sizes = [16, 20, 24, 32, 40, 48, 64, 256]
images = [subprocess.run(["rsvg-convert", "-w", str(s), "-h", str(s), str(svg)], check=True, capture_output=True).stdout for s in sizes]
offset = 6 + 16 * len(sizes)
out = struct.pack("<HHH", 0, 1, len(sizes))
for size, png in zip(sizes, images):
    out += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(png), offset)
    offset += len(png)
(here / "beardictate.ico").write_bytes(out + b"".join(images))
