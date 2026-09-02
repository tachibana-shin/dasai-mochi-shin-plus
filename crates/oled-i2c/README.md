# oled-i2c

[![Crates.io](https://img.shields.io/crates/v/oled-i2c)](https://crates.io/crates/oled-i2c)
[![docs.rs](https://img.shields.io/docsrs/oled-i2c)](https://docs.rs/oled-i2c)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

Minimal `no_std` I2C OLED driver for the **SSD1306** and **SH1106** controller
families (0.96" / 1.3" 1bpp OLED modules).

* Generic over any [`embedded_hal::i2c::I2c`] implementation — works on ESP32,
  STM32, RP2040, AVR, ... with no HAL-specific code.
* Raw **page-major** 1bpp frame buffers (`width * height / 8` bytes), so it is
  trivially fed by any 1bpp bitmap renderer (`.qgif` player, U8g2-style
  bitmaps, glyph fonts, ...).
* Handles the SH1106 132-column RAM window (column offset 2) automatically.
* Optional [`embedded-graphics`] `DrawTarget` implementation for drawing e-g
  primitives and fonts directly on the framebuffer.
* **no_std** (no allocator required), works in bare-metal / `#[no_main]`
  embedded programs.

MSRV: Rust **1.81**.

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oled-i2c = "0.2"
embedded-hal = "1.0"   # provides the I2C trait (or your chip HAL re-exports it)
```

```rust,ignore
use oled_i2c::{Oled, OledConfig, FONT_5X7};

let mut display = Oled::new(
    i2c,                      // any embedded_hal::i2c::I2c
    0x3C,                     // 7-bit I2C address
    OledConfig::sh1106_128x64().with_contrast(150),
)
.expect("OLED init failed");

let mut frame = [0u8; 1024];  // 128x64
frame[0] = 0b1010_1010;
display.draw_frame(&frame).ok();

// Text output (internal framebuffer, flush to display):
display.clear().ok();
display.set_cursor(0, 8);
display.print("Hello!");
display.set_text_size(2);
display.set_text_color(false); // draw dark-on-bright text
display.draw_str(0, 32, "2x text");
display.flush().ok();
```

### Draw with `embedded-graphics`

Enable the `embedded-graphics` feature to draw e-g primitives and fonts
directly on the internal framebuffer:

```toml
[dependencies]
oled-i2c = { version = "0.2", features = ["embedded-graphics"] }
embedded-graphics = "0.8"
```

```rust,ignore
use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Circle, PrimitiveStyle},
    mono_font::{ascii::FONT_6X10, MonoTextStyle},
    text::Text,
};

// Oled<impl I2c> implements DrawTarget<Color = BinaryColor>.
Circle::new(Point::new(20, 10), 20)
    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
    .draw(&mut display)?;
Text::new("hi", Point::new(8, 40), MonoTextStyle::new(&FONT_6X10, BinaryColor::On))
    .draw(&mut display)?;
display.flush()?; // push the framebuffer to the panel
```


## Text & fonts

The driver draws text into an internal frame buffer and pushes it with
`flush()` (Adafruit `display()` / U8g2 `sendBuffer()`):

* `set_cursor(x, y)` / `print("..")` — cursor based (Adafruit GFX style).
* `draw_str(x, y, "..")` — absolute position (U8g2 `drawStr` style).
* `draw_char(x, y, c)`, `print_char(c)` — single glyphs.
* `set_text_size(n)` — integer scale factor (1, 2, 3, ...), i.e.
  `set_text_scale(n, 1)`.
* `set_text_scale(num, den)` — rational scale factor (e.g. `3/2` = 1.5x,
  `7/10` = 0.7x) for fractional sizes between the integer steps.
* `set_text_color(on: bool)` — pixel value for glyph pixels.
* `set_text_overflow(mode)` — how lines are handled at the right edge (see the
  [`TextOverflow`] modes below). `set_text_wrap(bool)` is kept as a shorthand
  for `Wrap` / `None`.
* `measure_text("..") -> (w, h)` — size in pixels at the current size.
* `set_font(&'static Font)` — switch to a custom font (default `FONT_5X7`).

#### Text overflow modes

`Oled::set_text_overflow` picks how `print` / `print_char` / `draw_str` deal
with text that would run past the panel's right edge:

| Mode | Behavior |
|------|----------|
| `None` (default) | Draw as-is; overflow is clipped by the panel edge. |
| `Wrap` | Break the line at the right edge and continue on the next line. |
| `Ellipsis` | Truncate each overflowing line and append an ellipsis marker (default `"..."`, changeable with `set_ellipsis`), then wrap the remainder. |
| `Marquee` | Scroll the text horizontally so nothing is cut off. This one is animated: render one frame per call with `scroll_text_frame(x, y, s, width, offset_px, font)`, advancing `offset_px` over time and flushing between frames. |

The `Ellipsis` marker counts toward the line width, so a longer marker leaves
less room for the visible text.

`FONT_5X7` is a built-in ASCII `0x20..=0x7E` font (95 glyphs). Custom fonts
use the same Adafruit GFX `GFXfont` layout (see `src/fonts.rs` for the struct
definition), so fonts produced by `fontconvert` or similar tools can be copied
into a `static Font` almost verbatim.

### Runtime `.bdf` fonts

If you need to load a font at runtime (e.g. from an SD card), the driver also
parses the plain-text **`.bdf` (Glyph Bitmap Distribution Format)**:
a `<BdfFont>::parse(&bytes)` produces an owned font that borrows as a normal
`Font` for the one-shot `draw_str_with` / `draw_char_with` / `measure_text_with`
APIs:

```rust,ignore
use oled_i2c::bdf::BdfFont;

// Read a .bdf file's bytes (from SD / flash) into `data`.
let font = BdfFont::<8192, 256>::parse(&data)?;   // capacities are type params
let (w, h) = display.measure_text_with("Hi", &font.font());
display.draw_str_with(x, y, "Hi", &font.font());
```

- `BdfFont<BITMAP_CAP, SPAN_CAP>` — `BITMAP_CAP` total bitmap bytes,
  `SPAN_CAP` = max code span (`last - first + 1`) it can address. Codes the
  font doesn't define get blank placeholders.
- Metrics come from the file: height from `FONT_ASCENT + FONT_DESCENT`
  (falling back to the global `FONTBOUNDINGBOX` height), advances from
  `DWIDTH`, and origin offsets from `BBX`.
- Helpers: `crates/oled-i2c/tools/make_sample_bdf.py` writes a small 5x7
  `FONT.BDF`, and `crates/oled-i2c/tools/convert_bdf.py` embeds any `.bdf`
  font as a compiled-in `static Font<'static>`.

## Pixel / shape drawing

* `set_pixel(x, y, on)` — single pixel in the internal buffer.
* `draw_hline(x, y, w, on)` / `draw_vline(x, y, h, on)` — fast axis-aligned
  lines (GFX `drawFastHLine`/`drawFastVLine`).
* `fill_rect(x, y, w, h, on)` / `draw_rect(x, y, w, h, on)` — filled and
  1-px outline rectangles.
* `draw_line(x0, y0, x1, y1, on)` — Bresenham line between two points.
* `fill_screen(on)` — fill the whole panel (`fillScreen`).
* `draw_circle(x, y, r, on)` / `fill_circle(x, y, r, on)` — circle outline /
  filled disc (`drawCircle`/`fillCircle`).
* `draw_ellipse(x, y, rw, rh, on)` / `fill_ellipse(...)` — ellipse outline /
  filled (`drawEllipse`/`fillEllipse`).
* `draw_round_rect(x, y, w, h, r, on)` / `fill_round_rect(...)` — rounded
  rectangle outline / filled (`drawRoundRect`/`fillRoundRect`).
* `draw_triangle(x0,y0, x1,y1, x2,y2, on)` / `fill_triangle(...)` — triangle
  outline / filled (`drawTriangle`/`fillTriangle`).
* `draw_bitmap(x, y, data, w, h, on)` — 1bpp bitmap, MSB-first rows,
  row-padded to whole bytes (GFX `drawBitmap`).
* `draw_xbitmap(x, y, data, w, h, on)` — 1bpp X11 `.xbm` bitmap, LSB-first
  rows (GFX `drawXBitmap`).
* `fb()` / `fb_mut()` — read or directly edit the internal frame buffer.
* `draw_frame(&buf)` — push a caller-supplied page-major buffer.
* `clear()` / `clear_buffer()` / `flush()` — buffer management.

## Orientation: rotation, flip, invert

Three independent, orthogonal controls change how the panel is oriented:

* **Rotation** — `set_rotation(0..=3)` (GFX `setRotation`) rotates the
  *drawing APIs* 0° / 90° / 180° / 270° by remapping logical coordinates
  before they touch the frame buffer. `get_rotation()` reads it back, and
  `width()` / `height()` are swapped for the 90°/270° cases.
* **Hardware flip (mirror)** — `flip_horizontal(bool)` and
  `flip_vertical(bool)` mirror the panel on the *controller itself* using the
  SSD1306/SH1106 **segment remap** (`0xA1`/`0xA0`) and **COM output scan
  direction** (`0xC8`/`0xC0`) registers. This is useful to compensate for how
  a panel is physically mounted, and a 180° rotation can be done purely on the
  controller by enabling both. The state is remembered and reapplied by a
  re-init. Getters: `flipped_horizontal()` / `flipped_vertical()`.
* **Invert** — `invert(bool)` flips the pixel polarity (dark ↔ bright) via the
  `0xA7`/`0xA6` registers.

## Supported panels

| Config                | Controller | Resolution | Notes                  |
|-----------------------|-----------|------------|------------------------|
| `ssd1306_128x64()`    | SSD1306   | 128x64     | most common 0.96"      |
| `ssd1306_128x32()`    | SSD1306   | 128x32     | 0.96" / 0.91" half     |
| `sh1106_128x64()`     | SH1106    | 128x64     | 1.3" (column offset 2) |

## License

MIT OR Apache-2.0