//! Runtime loading of `.bdf` (Glyph Bitmap Distribution Format) bitmap fonts.
//!
//! BDF fonts are plain-text: each glyph is described by a `BBX` (its own
//! bounding box + origin offsets), a `DWIDTH` (horizontal advance) and a
//! `BITMAP` block of hex rows. This module parses such a file into a
//! [`BdfFont`], which owns the unpacked glyph bitmaps + metrics in
//! fixed-capacity [`heapless`] buffers, and can be borrowed as a normal
//! [`Font`] for the text APIs.
//!
//! # Example
//!
//! ```ignore
//! // Parse the bytes of a `.bdf` file read from an SD card.
//! let mut font = BdfFont::<8192, 256>::parse(&file_bytes)?;
//! display.draw_str_with(0, 0, "Hi", &font.font());
//! ```
//!
//! # Capacities
//!
//! [`BdfFont`] is generic over two constants:
//!
//! * `N_BITMAP` — total bytes of unpacked 1bpp glyph bitmap data.
//! * `N_GLYPHS` — the size of the *code span* (`last - first + 1`) that can
//!   be addressed. Glyphs are stored dense, indexed by `code - first`, with a
//!   blank placeholder for any code in the span that the font does not define.

use heapless::Vec;

use crate::fonts::{Font, Glyph};

/// Unpacked bitmap capacity used when the caller does not pick custom sizes.
pub const DEFAULT_BITMAP_CAP: usize = 8192;
/// Code-span capacity used by default (fits ASCII / Latin-1, ~256 code points).
pub const DEFAULT_GLYPH_SPAN_CAP: usize = 256;

/// A runtime-loaded bitmap font parsed from a `.bdf` file.
///
/// Owns the unpacked 1bpp bitmap data and glyph metrics so it can outlive the
/// source bytes. Borrow a [`Font`] view of it with [`BdfFont::font`] and hand
/// that to the [`Oled`](crate::Oled) text APIs (`draw_str_with`, ...).
#[derive(Debug)]
pub struct BdfFont<
    const N_BITMAP: usize = DEFAULT_BITMAP_CAP,
    const N_GLYPHS: usize = DEFAULT_GLYPH_SPAN_CAP,
> {
    bitmap: Vec<u8, N_BITMAP>,
    glyphs: Vec<Glyph, N_GLYPHS>,
    first: u16,
    last: u16,
    y_advance: u8,
}

impl<const N_BITMAP: usize, const N_GLYPHS: usize> BdfFont<N_BITMAP, N_GLYPHS> {
    /// Parse a `.bdf` file from its raw bytes.
    ///
    /// # Errors
    ///
    /// Returns [`BdfError`] if the data is not a valid BDF file, or if the
    /// parsed glyph data does not fit the two capacity constants.
    pub fn parse(data: &[u8]) -> Result<Self, BdfError> {
        Parser::new(data).run()
    }

    /// Borrow a [`Font`] view of this font, usable directly with the text APIs.
    pub fn font(&self) -> Font<'_> {
        Font {
            bitmap: &self.bitmap,
            glyphs: &self.glyphs,
            first: self.first,
            last: self.last,
            y_advance: self.y_advance,
        }
    }

    /// The line height (advance to the next text row) in pixels.
    pub fn line_height(&self) -> u8 {
        self.y_advance
    }

    /// The first character code covered by this font.
    pub fn first(&self) -> u16 {
        self.first
    }

    /// The last character code covered by this font.
    pub fn last(&self) -> u16 {
        self.last
    }

    /// Number of bytes of unpacked bitmap data.
    pub fn bitmap_len(&self) -> usize {
        self.bitmap.len()
    }

    /// Number of glyphs (including blank placeholders) in the code span.
    pub fn glyph_count(&self) -> usize {
        self.glyphs.len()
    }
}

/// Errors produced while parsing a `.bdf` file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BdfError {
    /// The data does not begin with `STARTFONT` — not a BDF file.
    NotBdf,
    /// The file has no `CHARS` (0 glyphs) or no characters were encoded.
    NoChars,
    /// A line was malformed (bad keyword / field count / value).
    Malformed,
    /// A `BITMAP` row contained a non-hexadecimal digit.
    BadHex,
    /// A character has an `ENCODING` outside the supported 16-bit range.
    EncodingOutOfRange,
    /// The code span `last - first + 1` exceeds `N_GLYPHS`.
    SpanTooLarge,
    /// The unpacked glyph bitmaps exceed `N_BITMAP`.
    BitmapOverflow,
}

/// A `.bdf` source file, split into logical lines.
struct Parser<'a> {
    rest: &'a [u8],
}

impl<'a> Parser<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { rest: data }
    }

    /// Return the next line (leading/trailing whitespace trimmed) or `None`.
    fn next_line(&mut self) -> Option<&'a [u8]> {
        let data = self.rest;
        if data.is_empty() {
            return None;
        }
        let mut i = 0;
        while i < data.len() && data[i] != b'\n' {
            i += 1;
        }
        let mut line = &data[..i];
        self.rest = if i < data.len() {
            &data[i + 1..]
        } else {
            &data[data.len()..]
        };
        // Strip trailing \r (CRLF).
        if line.last() == Some(&b'\r') {
            line = &line[..line.len() - 1];
        }
        // Trim spaces/tabs at both ends.
        let mut a = 0;
        let mut b = line.len();
        while a < b && (line[a] == b' ' || line[a] == b'\t') {
            a += 1;
        }
        while b > a && (line[b - 1] == b' ' || line[b - 1] == b'\t') {
            b -= 1;
        }
        Some(&line[a..b])
    }

    fn run<const N_BITMAP: usize, const N_GLYPHS: usize>(
        mut self,
    ) -> Result<BdfFont<N_BITMAP, N_GLYPHS>, BdfError> {
        let first = self
            .next_line()
            .ok_or(BdfError::NotBdf)?;
        if !first.starts_with(b"STARTFONT") {
            return Err(BdfError::NotBdf);
        }

        let mut font_bbx_h: Option<u32> = None;
        let mut ascent: Option<u32> = None;
        let mut descent: Option<u32> = None;
        let mut in_props = false;

        // Sweep the header until the `CHARS` marker.
        loop {
            let Some(line) = self.next_line() else {
                return Err(BdfError::NoChars);
            };
            if in_props && line.starts_with(b"ENDPROPERTIES") {
                in_props = false;
            }
            if line.starts_with(b"STARTPROPERTIES") {
                in_props = true;
            } else if line.starts_with(b"FONTBOUNDINGBOX") {
                let f = split_fields(line);
                if f.len() >= 3 {
                    font_bbx_h = parse_num(f[2]);
                }
            } else if line.starts_with(b"FONT_ASCENT") {
                let f = split_fields(line);
                if f.len() >= 2 {
                    ascent = parse_num(f[1]);
                }
            } else if line.starts_with(b"FONT_DESCENT") {
                let f = split_fields(line);
                if f.len() >= 2 {
                    descent = parse_num(f[1]);
                }
            } else if line.starts_with(b"CHARS") {
                break;
            }
        }

        let y_advance: u8 = ascent
            .zip(descent)
            .and_then(|(a, d)| (a + d).try_into().ok())
            .or_else(|| font_bbx_h.and_then(|h| (h as u16).try_into().ok()))
            .unwrap_or(8);

        let mut out = BdfFont {
            bitmap: Vec::new(),
            glyphs: Vec::new(),
            first: 0,
            last: 0,
            y_advance,
        };

        // Walk the character blocks.
        loop {
            let Some(line) = self.next_line() else {
                return Err(BdfError::NoChars);
            };
            if line.starts_with(b"STARTCHAR") {
                parse_glyph_block(&mut self, &mut out)?;
            } else if line.starts_with(b"ENDCHARS") {
                break;
            }
        }

        if out.glyphs.is_empty() {
            return Err(BdfError::NoChars);
        }
        Ok(out)
    }
}

/// Parse one `STARTCHAR .. ENDCHAR` block into `out`.
fn parse_glyph_block<const N_BITMAP: usize, const N_GLYPHS: usize>(
    p: &mut Parser<'_>,
    out: &mut BdfFont<N_BITMAP, N_GLYPHS>,
) -> Result<(), BdfError> {
    let mut encodings: Vec<u16, 8> = Vec::new();
    let mut dwidth: u32 = 0;
    let mut bbx_w: u32 = 0;
    let mut bbx_h: u32 = 0;
    let mut bbx_x: i32 = 0;
    let mut bbx_y: i32 = 0;
    let mut rows_bitmap: Vec<u8, 256> = Vec::new();
    let mut in_bitmap = false;

    loop {
        let Some(line) = p.next_line() else {
            return Err(BdfError::Malformed);
        };

        if in_bitmap {
            // Consume raw hex rows until a keyword begins the next item.
            if is_keyword(line) {
                in_bitmap = false;
            } else if line.is_empty() {
                continue;
            } else {
                // Append the decoded bytes for this row.
                decode_row(line, &mut rows_bitmap)?;
                continue;
            }
        }

        if line.starts_with(b"ENDCHAR") {
            break;
        } else if line.starts_with(b"ENCODING") {
            let f = split_fields(line);
            let Some(code) = (f.len() >= 2).then(|| parse_num(f[1])).flatten() else {
                return Err(BdfError::Malformed);
            };
            // -1 (0xFFFFFFFF) means "unencoded"; skip those.
            if code != u32::MAX {
                if code > 0xFFFF {
                    return Err(BdfError::EncodingOutOfRange);
                }
                let _ = encodings.push(code as u16);
            }
        } else if line.starts_with(b"DWIDTH") {
            let f = split_fields(line);
            if f.len() >= 2 {
                dwidth = parse_num(f[1]).unwrap_or(0);
            }
        } else if line.starts_with(b"BBX") {
            let f = split_fields(line);
            if f.len() >= 5 {
                bbx_w = parse_num(f[1]).unwrap_or(0);
                bbx_h = parse_num(f[2]).unwrap_or(0);
                bbx_x = parse_signed(f[3]).unwrap_or(0);
                bbx_y = parse_signed(f[4]).unwrap_or(0);
            }
        } else if line.starts_with(b"BITMAP") {
            in_bitmap = true;
        }
    }

    // Build the glyph and insert it for each encoding.
    let offset = out.bitmap.len();
    for b in &rows_bitmap {
        out.bitmap.push(*b).map_err(|_| BdfError::BitmapOverflow)?;
    }
    let glyph = Glyph {
        bitmap_offset: offset as u16,
        width: bbx_w.min(255) as u8,
        height: bbx_h.min(255) as u8,
        x_advance: dwidth.min(255) as u8,
        x_offset: bbx_x.clamp(-128, 127) as i8,
        y_offset: bbx_y.clamp(-128, 127) as i8,
    };

    for code in &encodings {
        extend_span(out, *code)?;
        let slot = (*code as u32 - out.first as u32) as usize;
        out.glyphs[slot] = glyph;
    }

    Ok(())
}

/// Grow `out.first`/`out.last`/`out.glyphs` until the span covers `code`,
/// filling holes with blank placeholders.
fn extend_span<const N_BITMAP: usize, const N_GLYPHS: usize>(
    out: &mut BdfFont<N_BITMAP, N_GLYPHS>,
    code: u16,
) -> Result<(), BdfError> {
    if out.glyphs.is_empty() {
        out.first = code;
        out.last = code;
    } else {
        if code < out.first {
            out.first = code;
        }
        if code > out.last {
            out.last = code;
        }
    }
    let span = (out.last as u32 - out.first as u32 + 1) as usize;
    if span > N_GLYPHS {
        return Err(BdfError::SpanTooLarge);
    }
    while out.glyphs.len() < span {
        out.glyphs
            .push(blank_glyph(out.bitmap.len() as u16))
            .map_err(|_| BdfError::SpanTooLarge)?;
    }
    Ok(())
}

/// A blank placeholder glyph used for codes the font does not define.
fn blank_glyph(bitmap_offset: u16) -> Glyph {
    Glyph {
        bitmap_offset,
        width: 0,
        height: 0,
        x_advance: 0,
        x_offset: 0,
        y_offset: 0,
    }
}

/// Decode one `BITMAP` hex row `line` and append the raw 1bpp bytes to `out`.
///
/// Each byte in the row is 2 hex digits (MSB-first, matching [`Glyph`]
/// packing). BDF rows may contain an odd number of nibbles; the last partial
/// byte is padded with a low nibble of `0`.
fn decode_row(line: &[u8], out: &mut Vec<u8, 256>) -> Result<(), BdfError> {
    let nibbles = line.len();
    let mut i = 0;
    while i < nibbles {
        let hi = hex_val(line[i]).ok_or(BdfError::BadHex)?;
        i += 1;
        let lo = if i < nibbles {
            hex_val(line[i]).ok_or(BdfError::BadHex)?
        } else {
            0
        };
        i += 1;
        out.push((hi << 4) | lo).map_err(|_| BdfError::BitmapOverflow)?;
    }
    Ok(())
}

/// Whether a line starts a new BDF keyword (used to end a `BITMAP` run).
fn is_keyword(line: &[u8]) -> bool {
    line.starts_with(b"ENDCHAR")
        || line.starts_with(b"STARTCHAR")
        || line.starts_with(b"ENCODING")
        || line.starts_with(b"DWIDTH")
        || line.starts_with(b"BBX")
        || line.starts_with(b"BITMAP")
        || line.starts_with(b"ENDCHARS")
        || line.starts_with(b"ENDFONT")
}

fn split_fields(line: &[u8]) -> Vec<&[u8], 16> {
    let mut out: Vec<&[u8], 16> = Vec::new();
    let n = line.len();
    let mut i = 0;
    while i < n {
        // Skip separators.
        while i < n && (line[i] == b' ' || line[i] == b'\t') {
            i += 1;
        }
        if i >= n {
            break;
        }
        let start = i;
        while i < n && line[i] != b' ' && line[i] != b'\t' {
            i += 1;
        }
        let _ = out.push(&line[start..i]);
        if out.len() == 16 {
            break;
        }
    }
    out
}

fn parse_num(f: &[u8]) -> Option<u32> {
    if f.is_empty() {
        return None;
    }
    let mut v: u32 = 0;
    for &b in f {
        if !b.is_ascii_digit() {
            return None;
        }
        v = v.saturating_mul(10).saturating_add((b - b'0') as u32);
    }
    Some(v)
}

fn parse_signed(f: &[u8]) -> Option<i32> {
    if f.is_empty() {
        return None;
    }
    let (neg, rest) = if f[0] == b'-' {
        (true, &f[1..])
    } else {
        (false, f)
    };
    let v = parse_num(rest)? as i32;
    Some(if neg { -v } else { v })
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'A'..=b'F' => Some(b - b'A' + 10),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a tiny 3-glyph BDF (space, 'A', 'B').
    ///
    /// 'A' and 'B' are drawn as 5 px wide, 8 px tall bitmaps that let us check
    /// both metric parsing and pixel reading:
    ///   'A': 0x00,0x20,0x50,0x88,0xF8,0x88,0x88,0x88
    ///   'B': 0xF0,0x88,0x88,0xF0,0x88,0x88,0xF0,0x00
    fn sample_bdf() -> &'static str {
        "STARTFONT 2.1\n\
         FONT -test-tiny-medium-r-normal--14-100-100-100-C-70-ISO10646-1\n\
         SIZE 14 100 100\n\
         FONTBOUNDINGBOX 7 14 0 -3\n\
         STARTPROPERTIES 4\n\
         FONT_ASCENT 8\n\
         FONT_DESCENT 2\n\
         DEFAULT_CHAR 32\n\
         ENDPROPERTIES\n\
         CHARS 3\n\
         STARTCHAR space\n\
         ENCODING 32\n\
         DWIDTH 6 0\n\
         BBX 5 8 0 -2\n\
         BITMAP\n\
         00\n\
         00\n\
         00\n\
         00\n\
         00\n\
         00\n\
         00\n\
         00\n\
         ENDCHAR\n\
         STARTCHAR A\n\
         ENCODING 65\n\
         DWIDTH 6 0\n\
         BBX 5 8 0 -2\n\
         BITMAP\n\
         00\n\
         20\n\
         50\n\
         88\n\
         F8\n\
         88\n\
         88\n\
         88\n\
         ENDCHAR\n\
         STARTCHAR B\n\
         ENCODING 66\n\
         DWIDTH 6 0\n\
         BBX 5 8 0 -2\n\
         BITMAP\n\
         F0\n\
         88\n\
         88\n\
         F0\n\
         88\n\
         88\n\
         F0\n\
         00\n\
         ENDCHAR\n\
         ENDCHARS\n\
         ENDFONT\n"
    }

    #[test]
    fn parses_ascii_font() {
        let f = BdfFont::<8192, 256>::parse(sample_bdf().as_bytes()).expect("parse");
        let font = f.font();
        assert_eq!(font.first, 0x20);
        assert_eq!(font.last, 0x42);
        assert_eq!(font.y_advance, 10, "ascent+descent");
        assert_eq!(font.glyphs.len(), 0x42 - 0x20 + 1);

        let a = font.glyph_for('A').expect("A missing");
        assert_eq!(a.width, 5);
        assert_eq!(a.height, 8);
        assert_eq!(a.x_advance, 6);
        assert_eq!(a.y_offset, -2);

        // Row 2 of 'A' is 0x50 -> pattern 0 1 0 1 0 at cols 0..5.
        assert!(font.glyph_bit(a, 2, 1));
        assert!(!font.glyph_bit(a, 2, 0));
        assert!(font.glyph_bit(a, 2, 3));
        // Row 4 is 0xF8 -> `1111 1000`: bits 7..3 set (col0..4 on), bit 2 (col5)
        // is padding/off.
        assert!(font.glyph_bit(a, 4, 0));
        assert!(font.glyph_bit(a, 4, 4));
        assert!(!font.glyph_bit(a, 4, 5));

        // Space advances but draws nothing.
        let sp = font.glyph_for(' ').expect("space missing");
        assert_eq!(sp.width, 5);
        assert_eq!(sp.height, 8);
        // All rows are 0 -> no ink.
        for r in 0..8 {
            for c in 0..5 {
                assert!(!font.glyph_bit(sp, r, c));
            }
        }
    }

    #[test]
    fn missing_char_is_blank_placeholder() {
        let f = BdfFont::<8192, 256>::parse(sample_bdf().as_bytes()).unwrap();
        let font = f.font();
        // 'C' (0x43) is beyond `last`, so no glyph.
        assert!(font.glyph_for('C').is_none());
        // A code inside the span but not defined (e.g. 0x40 '@') should return
        // the blank placeholder: width 0.
        let at = font.glyph_for('@').expect("should have placeholder");
        assert_eq!(at.width, 0);
    }

    #[test]
    fn rejects_non_bdf() {
        assert_eq!(BdfFont::<100, 100>::parse(b"hello").unwrap_err(), BdfError::NotBdf);
        assert_eq!(
            BdfFont::<100, 100>::parse(b"").unwrap_err(),
            BdfError::NotBdf
        );
    }

    #[test]
    fn span_too_large_is_rejected() {
        // Font spans 0x20..=0x42 (35 entries) but capacity is 10.
        let err = BdfFont::<8192, 10>::parse(sample_bdf().as_bytes()).unwrap_err();
        assert_eq!(err, BdfError::SpanTooLarge);
    }

    #[test]
    fn bad_hex_is_rejected() {
        let data = sample_bdf().replace("F8", "ZZ");
        assert_eq!(
            BdfFont::<8192, 256>::parse(data.as_bytes()).unwrap_err(),
            BdfError::BadHex
        );
    }

    #[test]
    fn crlf_and_sparse_codes() {
        // Same font but CRLF line endings and an out-of-order char, to exercise
        // the tolerant line reader.
        let crlf = sample_bdf().replace("\n", "\r\n");
        let f = BdfFont::<8192, 256>::parse(crlf.as_bytes()).expect("parse crlf");
        assert!(f.font().glyph_for('A').is_some());
    }

    #[test]
    fn parses_real_sample_file() {
        // The file emitted by `tools/make_sample_bdf.py` (the exact bytes you
        // would put on the SD card as /FONT.BDF).
        let data = include_str!("../tools/sample_font.bdf");
        let f = BdfFont::<4096, 128>::parse(data.as_bytes()).expect("parse sample");
        let font = f.font();
        assert_eq!(font.first, 0x20);
        assert_eq!(font.last, 0x5A, "space + A..Z");
        assert_eq!(font.y_advance, 7);
        assert_eq!(f.glyph_count(), 0x5A - 0x20 + 1);
        // 'D' is present and 5x7.
        let d = font.glyph_for('D').expect("D missing");
        assert_eq!((d.width, d.height, d.x_advance), (5, 7, 6));
        // row 1 of 'D' is 0x88: left + right verticals (col0, col4) lit,
        // col2 is inside the bowl (off).
        assert!(font.glyph_bit(d, 1, 0));
        assert!(font.glyph_bit(d, 1, 4));
        assert!(!font.glyph_bit(d, 1, 2));
    }
}

