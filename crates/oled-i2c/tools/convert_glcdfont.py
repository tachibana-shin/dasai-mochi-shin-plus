#!/usr/bin/env python3
"""Convert an Adafruit GFX `glcdfont.c` (column-major 5x7 LCD font) into an
`oled-i2c` `Font` static.

This is the script that generated `src/fonts.rs` (`FONT_5X7`). Use it to
regenerate that file, or adapt it to convert other column-major 5x7 fonts
(e.g. from U8g2/Hid... pretty much any "5 bytes per glyph, byte = column,
bit0 = top row" font).

Usage:
    python3 convert_glcdfont.py path/to/glcdfont.c

Output is printed to stdout as a Rust `Font` static ready to paste into your
firmware (or into `crates/oled-i2c/src/fonts.rs`).
"""

import re
import sys


def parse_glcdfont(path: str) -> list[int]:
    txt = open(path).read()
    vals = [int(x, 16) for x in re.findall(r"0x([0-9A-Fa-f]{1,2})", txt)]
    # glcdfont.c holds 256 glyphs x 5 bytes.
    if len(vals) != 256 * 5:
        raise SystemExit(f"expected 1280 values, got {len(vals)}")
    return vals


def glyph_rows(cols: list[int], width: int, height: int) -> list[int]:
    """Column-major glyph -> row-major bytes (bit7 = leftmost pixel)."""
    rows = []
    for r in range(height):
        byte = 0
        for c in range(width):
            bit = (cols[c] >> r) & 1
            byte |= bit << (7 - c)
        rows.append(byte)
    return rows


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    vals = parse_glcdfont(sys.argv[1])

    first, last = 0x20, 0x7E
    width, height, x_advance, y_advance = 5, 8, 6, 8
    row_bytes = (width + 7) // 8

    bitmap: list[int] = []
    glyphs: list[str] = []
    off = 0
    for code in range(first, last + 1):
        cols = vals[code * 5 : code * 5 + 5]
        bitmap.extend(glyph_rows(cols, width, height))
        ch = chr(code)
        glyphs.append(
            f"        Glyph {{ bitmap_offset: {off}, width: {width}, "
            f"height: {height}, x_advance: {x_advance}, x_offset: 0, "
            f"y_offset: 0 }}, // U+{code:04X} '{ch}'"
        )
        off += row_bytes * height

    out: list[str] = [
        "pub static MY_FONT: Font = Font {",
        "    bitmap: &[",
    ]
    for i in range(0, len(bitmap), 12):
        chunk = ", ".join(f"0x{b:02X}" for b in bitmap[i : i + 12])
        out.append(f"        {chunk},")
    out.append("    ],")
    out.append("    glyphs: &[")
    out.extend(glyphs)
    out += [
        "    ],",
        f"    first: 0x{first:02X},",
        f"    last: 0x{last:02X},",
        f"    y_advance: {y_advance},",
        "};",
        "",
    ]
    print("\n".join(out))


if __name__ == "__main__":
    main()