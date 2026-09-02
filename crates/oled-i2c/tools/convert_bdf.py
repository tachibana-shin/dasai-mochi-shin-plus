#!/usr/bin/env python3
"""Convert a `.bdf` (Glyph Bitmap Distribution Format) font into an
`oled-i2c` `Font` static that is compiled into the firmware.

This is the compile-time counterpart of the runtime loader in
`src/bdf.rs` (`BdfFont::parse`). It flattens a `.bdf` file into the same
row-major 1bpp bitmap + `Glyph` metrics table that `FONT_5X7` uses, so you
can embed a font with no SD card / no runtime parser.

Usage:
    python3 convert_bdf.py path/to/font.bdf > my_font.rs

Output is a Rust `Font<'static>` literal (printed to stdout).
"""

import re
import sys


def split(line: str) -> list[str]:
    return line.split()


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)

    header = open(sys.argv[1]).read()
    lines = header.splitlines()

    # Global metrics.
    y_advance = 8
    glyphs: dict[int, tuple[int, int, int, int, int, list[int]]] = {}
    # (width, height, x_advance, x_offset, y_offset, bitmap_rows)

    def parse_hex_byte(row: str) -> int:
        row = row.replace(" ", "")
        return int(row, 16)

    i = 0
    in_glyph = False
    while i < len(lines):
        line = lines[i].strip()
        if line.startswith("FONT_ASCENT") or line.startswith("FONT_DESCENT"):
            i += 1
            continue
        if line.startswith("STARTPROPERTIES"):
            # collect FONT_ASCENT / FONT_DESCENT
            ascent = descent = None
            j = i
            while j < len(lines):
                lj = lines[j].strip()
                if lj.startswith("FONT_ASCENT"):
                    ascent = int(split(lj)[1])
                elif lj.startswith("FONT_DESCENT"):
                    descent = int(split(lj)[1])
                elif lj == "ENDPROPERTIES":
                    break
                j += 1
            if ascent is not None and descent is not None:
                y_advance = ascent + descent
            i = j + 1
            continue

        if line.startswith("STARTCHAR"):
            in_glyph = True
            glyph_enc: list[int] = []
            dwidth = 0
            width = height = xoff = yoff = 0
            bitmap: list[int] = []
            i += 1
            while i < len(lines):
                l = lines[i].strip()
                if l.startswith("ENCODING"):
                    enc = int(split(l)[1])
                    if enc != -1:
                        glyph_enc.append(enc)
                elif l.startswith("DWIDTH"):
                    dwidth = int(split(l)[1])
                elif l.startswith("BBX"):
                    _, w, h, xo, yo = split(l)[:5]
                    width, height = int(w), int(h)
                    xoff, yoff = int(xo), int(yo)
                elif l.startswith("BITMAP"):
                    i += 1
                    while i < len(lines) and lines[i].strip() not in (
                        "ENDCHAR",
                        "ENDCHARS",
                    ):
                        row = lines[i].strip()
                        if row:
                            bitmap.append(parse_hex_byte(row))
                        i += 1
                    continue
                elif l.startswith("ENDCHAR"):
                    break
                i += 1
            for enc in glyph_enc:
                glyphs[enc] = (width, height, dwidth, xoff, yoff, list(bitmap))
            i += 1
            continue
        i += 1

    if not glyphs:
        raise SystemExit("no glyphs parsed")

    first = min(glyphs)
    last = max(glyphs)

    bitmap: list[int] = []
    tables: list[str] = []
    off = 0
    for code in range(first, last + 1):
        g = glyphs.get(code)
        if g is None:
            # Blank placeholder.
            tables.append(
                f"        Glyph {{ bitmap_offset: {off}, width: 0, height: 0, "
                f"x_advance: 0, x_offset: 0, y_offset: 0 }}, // U+{code:04X}"
            )
            continue
        width, height, dwidth, xoff, yoff, rows = g
        bitmap.extend(rows)
        try:
            ch = chr(code)
        except ValueError:
            ch = "?"
        tables.append(
            f"        Glyph {{ bitmap_offset: {off}, width: {width}, "
            f"height: {height}, x_advance: {dwidth}, x_offset: {xoff}, "
            f"y_offset: {yoff} }}, // U+{code:04X} '{ch}'"
        )
        off += len(rows)

    out: list[str] = [
        f"pub static MY_FONT: Font<'static> = Font {{",
        "    bitmap: &[",
    ]
    for k in range(0, len(bitmap), 12):
        chunk = ", ".join(f"0x{b:02X}" for b in bitmap[k : k + 12])
        out.append(f"        {chunk},")
    out.append("    ],")
    out.append("    glyphs: &[")
    out.extend(tables)
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
