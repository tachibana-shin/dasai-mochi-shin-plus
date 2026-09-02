//! Bitmap fonts and text rendering support.
//!
//! Fonts follow the classic Adafruit GFX `GFXfont` layout so fonts produced
//! by tools like `fontconvert` can be dropped in almost verbatim:
//!
//! * `Font.bitmap` — glyph bitmaps, concatenated. Each glyph is **row-major**
//!   1bpp, `ceil(width/8) * height` bytes, bit 7 = leftmost pixel (the same
//!   packing as `.qgif` frames).
//! * `Font.glyphs` — one [`Glyph`] per character, indexed by
//!   `ch - Font.first`.

/// Metrics for a single character glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyph {
    /// Byte offset of this glyph's bitmap inside [`Font::bitmap`].
    pub bitmap_offset: u16,
    /// Glyph width in pixels.
    pub width: u8,
    /// Glyph height in pixels.
    pub height: u8,
    /// Horizontal advance to the next character (glyph width + spacing).
    pub x_advance: u8,
    /// Horizontal offset of the glyph from the character origin.
    pub x_offset: i8,
    /// Vertical offset of the glyph from the character origin (up = negative).
    pub y_offset: i8,
}

/// A 1bpp bitmap font (GFXfont-compatible).
#[derive(Debug, Clone, Copy)]
pub struct Font {
    /// All glyph bitmaps concatenated, row-major 1bpp, bit 7 = leftmost.
    pub bitmap: &'static [u8],
    /// Glyph metrics, indexed by `char - first`.
    pub glyphs: &'static [Glyph],
    /// First character code covered by this font.
    pub first: u16,
    /// Last character code covered by this font.
    pub last: u16,
    /// Line height in pixels (advance to the next text row).
    pub y_advance: u8,
}

impl Font {
    /// Look up the glyph for a character, or `None` if out of range.
    pub fn glyph_for(&self, c: char) -> Option<&'static Glyph> {
        let code = c as u32;
        if code < self.first as u32 || code > self.last as u32 {
            return None;
        }
        self.glyphs.get((code - self.first as u32) as usize)
    }

    /// Read one pixel of a glyph (`row`/`col` are glyph-local).
    pub fn glyph_bit(&self, g: &Glyph, row: u8, col: u8) -> bool {
        let row_bytes = (g.width as u16 + 7) / 8;
        let byte = self.bitmap
            [g.bitmap_offset as usize + row as usize * row_bytes as usize + col as usize / 8];
        (byte >> (7 - (col % 8))) & 1 == 1
    }

    /// Horizontal advance for a character (0 if out of range).
    pub fn advance(&self, c: char) -> u8 {
        self.glyph_for(c).map_or(0, |g| g.x_advance)
    }
}

/// Built-in 5x7 LCD font: printable ASCII `0x20..=0x7E` (95 glyphs).
///
/// Same shapes as Adafruit GFX's classic `glcdfont` (MIT). Each glyph is 5
/// pixels wide, 7 rows of ink inside an 8-tall cell, `x_advance = 6`,
/// `y_advance = 8` — two rows naturally leave a 1 px gap.
pub static FONT_5X7: Font = Font {
    bitmap: &[

        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20, 0x20, 0x20, 0x20,
        0x20, 0x00, 0x20, 0x00, 0x50, 0x50, 0x50, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x50, 0x50, 0xF8, 0x50, 0xF8, 0x50, 0x50, 0x00, 0x20, 0x78, 0xA0, 0x70,
        0x28, 0xF0, 0x20, 0x00, 0xC0, 0xC8, 0x10, 0x20, 0x40, 0x98, 0x18, 0x00,
        0x40, 0xA0, 0xA0, 0x40, 0xA8, 0x90, 0x68, 0x00, 0x30, 0x30, 0x20, 0x40,
        0x00, 0x00, 0x00, 0x00, 0x10, 0x20, 0x40, 0x40, 0x40, 0x20, 0x10, 0x00,
        0x40, 0x20, 0x10, 0x10, 0x10, 0x20, 0x40, 0x00, 0x20, 0xA8, 0x70, 0xF8,
        0x70, 0xA8, 0x20, 0x00, 0x00, 0x20, 0x20, 0xF8, 0x20, 0x20, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x30, 0x30, 0x20, 0x40, 0x00, 0x00, 0x00, 0xF8,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x30, 0x30, 0x00,
        0x00, 0x08, 0x10, 0x20, 0x40, 0x80, 0x00, 0x00, 0x70, 0x88, 0x98, 0xA8,
        0xC8, 0x88, 0x70, 0x00, 0x20, 0x60, 0x20, 0x20, 0x20, 0x20, 0x70, 0x00,
        0x70, 0x88, 0x08, 0x70, 0x80, 0x80, 0xF8, 0x00, 0xF8, 0x08, 0x10, 0x30,
        0x08, 0x88, 0x70, 0x00, 0x10, 0x30, 0x50, 0x90, 0xF8, 0x10, 0x10, 0x00,
        0xF8, 0x80, 0xF0, 0x08, 0x08, 0x88, 0x70, 0x00, 0x38, 0x40, 0x80, 0xF0,
        0x88, 0x88, 0x70, 0x00, 0xF8, 0x08, 0x08, 0x10, 0x20, 0x40, 0x80, 0x00,
        0x70, 0x88, 0x88, 0x70, 0x88, 0x88, 0x70, 0x00, 0x70, 0x88, 0x88, 0x78,
        0x08, 0x10, 0xE0, 0x00, 0x00, 0x00, 0x20, 0x00, 0x20, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x20, 0x00, 0x20, 0x20, 0x40, 0x00, 0x08, 0x10, 0x20, 0x40,
        0x20, 0x10, 0x08, 0x00, 0x00, 0x00, 0xF8, 0x00, 0xF8, 0x00, 0x00, 0x00,
        0x40, 0x20, 0x10, 0x08, 0x10, 0x20, 0x40, 0x00, 0x70, 0x88, 0x08, 0x30,
        0x20, 0x00, 0x20, 0x00, 0x70, 0x88, 0xA8, 0xB8, 0xB0, 0x80, 0x78, 0x00,
        0x20, 0x50, 0x88, 0x88, 0xF8, 0x88, 0x88, 0x00, 0xF0, 0x88, 0x88, 0xF0,
        0x88, 0x88, 0xF0, 0x00, 0x70, 0x88, 0x80, 0x80, 0x80, 0x88, 0x70, 0x00,
        0xF0, 0x88, 0x88, 0x88, 0x88, 0x88, 0xF0, 0x00, 0xF8, 0x80, 0x80, 0xF0,
        0x80, 0x80, 0xF8, 0x00, 0xF8, 0x80, 0x80, 0xF0, 0x80, 0x80, 0x80, 0x00,
        0x78, 0x88, 0x80, 0x80, 0x98, 0x88, 0x78, 0x00, 0x88, 0x88, 0x88, 0xF8,
        0x88, 0x88, 0x88, 0x00, 0x70, 0x20, 0x20, 0x20, 0x20, 0x20, 0x70, 0x00,
        0x38, 0x10, 0x10, 0x10, 0x10, 0x90, 0x60, 0x00, 0x88, 0x90, 0xA0, 0xC0,
        0xA0, 0x90, 0x88, 0x00, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0xF8, 0x00,
        0x88, 0xD8, 0xA8, 0xA8, 0xA8, 0x88, 0x88, 0x00, 0x88, 0x88, 0xC8, 0xA8,
        0x98, 0x88, 0x88, 0x00, 0x70, 0x88, 0x88, 0x88, 0x88, 0x88, 0x70, 0x00,
        0xF0, 0x88, 0x88, 0xF0, 0x80, 0x80, 0x80, 0x00, 0x70, 0x88, 0x88, 0x88,
        0xA8, 0x90, 0x68, 0x00, 0xF0, 0x88, 0x88, 0xF0, 0xA0, 0x90, 0x88, 0x00,
        0x70, 0x88, 0x80, 0x70, 0x08, 0x88, 0x70, 0x00, 0xF8, 0xA8, 0x20, 0x20,
        0x20, 0x20, 0x20, 0x00, 0x88, 0x88, 0x88, 0x88, 0x88, 0x88, 0x70, 0x00,
        0x88, 0x88, 0x88, 0x88, 0x88, 0x50, 0x20, 0x00, 0x88, 0x88, 0x88, 0xA8,
        0xA8, 0xA8, 0x50, 0x00, 0x88, 0x88, 0x50, 0x20, 0x50, 0x88, 0x88, 0x00,
        0x88, 0x88, 0x50, 0x20, 0x20, 0x20, 0x20, 0x00, 0xF8, 0x08, 0x10, 0x70,
        0x40, 0x80, 0xF8, 0x00, 0x78, 0x40, 0x40, 0x40, 0x40, 0x40, 0x78, 0x00,
        0x00, 0x80, 0x40, 0x20, 0x10, 0x08, 0x00, 0x00, 0x78, 0x08, 0x08, 0x08,
        0x08, 0x08, 0x78, 0x00, 0x20, 0x50, 0x88, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xF8, 0x00, 0x60, 0x60, 0x20, 0x10,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x60, 0x10, 0x70, 0x90, 0x78, 0x00,
        0x80, 0x80, 0xB0, 0xC8, 0x88, 0xC8, 0xB0, 0x00, 0x00, 0x00, 0x70, 0x88,
        0x80, 0x88, 0x70, 0x00, 0x08, 0x08, 0x68, 0x98, 0x88, 0x98, 0x68, 0x00,
        0x00, 0x00, 0x70, 0x88, 0xF8, 0x80, 0x70, 0x00, 0x10, 0x28, 0x20, 0x70,
        0x20, 0x20, 0x20, 0x00, 0x00, 0x00, 0x70, 0x98, 0x98, 0x68, 0x08, 0x70,
        0x80, 0x80, 0xB0, 0xC8, 0x88, 0x88, 0x88, 0x00, 0x20, 0x00, 0x60, 0x20,
        0x20, 0x20, 0x70, 0x00, 0x10, 0x00, 0x10, 0x10, 0x10, 0x90, 0x60, 0x00,
        0x80, 0x80, 0x90, 0xA0, 0xC0, 0xA0, 0x90, 0x00, 0x60, 0x20, 0x20, 0x20,
        0x20, 0x20, 0x70, 0x00, 0x00, 0x00, 0xD0, 0xA8, 0xA8, 0xA8, 0xA8, 0x00,
        0x00, 0x00, 0xB0, 0xC8, 0x88, 0x88, 0x88, 0x00, 0x00, 0x00, 0x70, 0x88,
        0x88, 0x88, 0x70, 0x00, 0x00, 0x00, 0xB0, 0xC8, 0xC8, 0xB0, 0x80, 0x80,
        0x00, 0x00, 0x68, 0x98, 0x98, 0x68, 0x08, 0x08, 0x00, 0x00, 0xB0, 0xC8,
        0x80, 0x80, 0x80, 0x00, 0x00, 0x00, 0x78, 0x80, 0x70, 0x08, 0xF0, 0x00,
        0x20, 0x20, 0xF8, 0x20, 0x20, 0x28, 0x10, 0x00, 0x00, 0x00, 0x88, 0x88,
        0x88, 0x98, 0x68, 0x00, 0x00, 0x00, 0x88, 0x88, 0x88, 0x50, 0x20, 0x00,
        0x00, 0x00, 0x88, 0x88, 0xA8, 0xA8, 0x50, 0x00, 0x00, 0x00, 0x88, 0x50,
        0x20, 0x50, 0x88, 0x00, 0x00, 0x00, 0x88, 0x88, 0x78, 0x08, 0x88, 0x70,
        0x00, 0x00, 0xF8, 0x10, 0x20, 0x40, 0xF8, 0x00, 0x10, 0x20, 0x20, 0x40,
        0x20, 0x20, 0x10, 0x00, 0x20, 0x20, 0x20, 0x00, 0x20, 0x20, 0x20, 0x00,
        0x40, 0x20, 0x20, 0x10, 0x20, 0x20, 0x40, 0x00, 0x40, 0xA8, 0x10, 0x00,
        0x00, 0x00, 0x00, 0x00,
    ],
    glyphs: &[
            Glyph { bitmap_offset: 0, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0020 ' '
            Glyph { bitmap_offset: 8, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0021 '!'
            Glyph { bitmap_offset: 16, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0022 '"'
            Glyph { bitmap_offset: 24, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0023 '#'
            Glyph { bitmap_offset: 32, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0024 '$'
            Glyph { bitmap_offset: 40, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0025 '%'
            Glyph { bitmap_offset: 48, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0026 '&'
            Glyph { bitmap_offset: 56, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0027 '''
            Glyph { bitmap_offset: 64, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0028 '('
            Glyph { bitmap_offset: 72, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0029 ')'
            Glyph { bitmap_offset: 80, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+002A '*'
            Glyph { bitmap_offset: 88, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+002B '+'
            Glyph { bitmap_offset: 96, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+002C ','
            Glyph { bitmap_offset: 104, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+002D '-'
            Glyph { bitmap_offset: 112, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+002E '.'
            Glyph { bitmap_offset: 120, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+002F '/'
            Glyph { bitmap_offset: 128, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0030 '0'
            Glyph { bitmap_offset: 136, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0031 '1'
            Glyph { bitmap_offset: 144, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0032 '2'
            Glyph { bitmap_offset: 152, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0033 '3'
            Glyph { bitmap_offset: 160, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0034 '4'
            Glyph { bitmap_offset: 168, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0035 '5'
            Glyph { bitmap_offset: 176, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0036 '6'
            Glyph { bitmap_offset: 184, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0037 '7'
            Glyph { bitmap_offset: 192, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0038 '8'
            Glyph { bitmap_offset: 200, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0039 '9'
            Glyph { bitmap_offset: 208, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+003A ':'
            Glyph { bitmap_offset: 216, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+003B ';'
            Glyph { bitmap_offset: 224, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+003C '<'
            Glyph { bitmap_offset: 232, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+003D '='
            Glyph { bitmap_offset: 240, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+003E '>'
            Glyph { bitmap_offset: 248, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+003F '?'
            Glyph { bitmap_offset: 256, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0040 '@'
            Glyph { bitmap_offset: 264, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0041 'A'
            Glyph { bitmap_offset: 272, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0042 'B'
            Glyph { bitmap_offset: 280, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0043 'C'
            Glyph { bitmap_offset: 288, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0044 'D'
            Glyph { bitmap_offset: 296, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0045 'E'
            Glyph { bitmap_offset: 304, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0046 'F'
            Glyph { bitmap_offset: 312, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0047 'G'
            Glyph { bitmap_offset: 320, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0048 'H'
            Glyph { bitmap_offset: 328, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0049 'I'
            Glyph { bitmap_offset: 336, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+004A 'J'
            Glyph { bitmap_offset: 344, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+004B 'K'
            Glyph { bitmap_offset: 352, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+004C 'L'
            Glyph { bitmap_offset: 360, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+004D 'M'
            Glyph { bitmap_offset: 368, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+004E 'N'
            Glyph { bitmap_offset: 376, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+004F 'O'
            Glyph { bitmap_offset: 384, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0050 'P'
            Glyph { bitmap_offset: 392, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0051 'Q'
            Glyph { bitmap_offset: 400, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0052 'R'
            Glyph { bitmap_offset: 408, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0053 'S'
            Glyph { bitmap_offset: 416, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0054 'T'
            Glyph { bitmap_offset: 424, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0055 'U'
            Glyph { bitmap_offset: 432, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0056 'V'
            Glyph { bitmap_offset: 440, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0057 'W'
            Glyph { bitmap_offset: 448, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0058 'X'
            Glyph { bitmap_offset: 456, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0059 'Y'
            Glyph { bitmap_offset: 464, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+005A 'Z'
            Glyph { bitmap_offset: 472, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+005B '['
            Glyph { bitmap_offset: 480, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+005C '\'
            Glyph { bitmap_offset: 488, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+005D ']'
            Glyph { bitmap_offset: 496, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+005E '^'
            Glyph { bitmap_offset: 504, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+005F '_'
            Glyph { bitmap_offset: 512, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0060 '`'
            Glyph { bitmap_offset: 520, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0061 'a'
            Glyph { bitmap_offset: 528, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0062 'b'
            Glyph { bitmap_offset: 536, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0063 'c'
            Glyph { bitmap_offset: 544, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0064 'd'
            Glyph { bitmap_offset: 552, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0065 'e'
            Glyph { bitmap_offset: 560, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0066 'f'
            Glyph { bitmap_offset: 568, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0067 'g'
            Glyph { bitmap_offset: 576, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0068 'h'
            Glyph { bitmap_offset: 584, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0069 'i'
            Glyph { bitmap_offset: 592, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+006A 'j'
            Glyph { bitmap_offset: 600, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+006B 'k'
            Glyph { bitmap_offset: 608, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+006C 'l'
            Glyph { bitmap_offset: 616, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+006D 'm'
            Glyph { bitmap_offset: 624, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+006E 'n'
            Glyph { bitmap_offset: 632, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+006F 'o'
            Glyph { bitmap_offset: 640, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0070 'p'
            Glyph { bitmap_offset: 648, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0071 'q'
            Glyph { bitmap_offset: 656, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0072 'r'
            Glyph { bitmap_offset: 664, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0073 's'
            Glyph { bitmap_offset: 672, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0074 't'
            Glyph { bitmap_offset: 680, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0075 'u'
            Glyph { bitmap_offset: 688, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0076 'v'
            Glyph { bitmap_offset: 696, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0077 'w'
            Glyph { bitmap_offset: 704, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0078 'x'
            Glyph { bitmap_offset: 712, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+0079 'y'
            Glyph { bitmap_offset: 720, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+007A 'z'
            Glyph { bitmap_offset: 728, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+007B '{'
            Glyph { bitmap_offset: 736, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+007C '|'
            Glyph { bitmap_offset: 744, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+007D '}'
            Glyph { bitmap_offset: 752, width: 5, height: 8, x_advance: 6, x_offset: 0, y_offset: 0 }, // U+007E '~'
    ],
    first: 0x20,
    last: 0x7E,
    y_advance: 8,
};
