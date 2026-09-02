# oled-i2c

Minimal `no_std` I2C OLED driver for the **SSD1306** and **SH1106** controller
families (0.96" / 1.3" 1bpp OLED modules).

* Generic over any [`embedded_hal::i2c::I2c`] implementation — works on ESP32,
  STM32, RP2040, AVR, ... with no HAL-specific code.
* Raw **page-major** 1bpp frame buffers (`width * height / 8` bytes), so it is
  trivially fed by any 1bpp bitmap renderer (`.qgif` player, U8g2-style
  bitmaps, glyph fonts, ...).
* Handles the SH1106 132-column RAM window (column offset 2) automatically.

## Usage

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

## Text & fonts

The driver draws text into an internal frame buffer and pushes it with
`flush()` (Adafruit `display()` / U8g2 `sendBuffer()`):

* `set_cursor(x, y)` / `print("..")` — cursor based (Adafruit GFX style).
* `draw_str(x, y, "..")` — absolute position (U8g2 `drawStr` style).
* `draw_char(x, y, c)`, `print_char(c)` — single glyphs.
* `set_text_size(n)` — integer scale factor (1, 2, 3, ...).
* `set_text_color(on: bool)` — pixel value for glyph pixels.
* `set_text_wrap(bool)` — wrap long lines at the right edge.
* `measure_text("..") -> (w, h)` — size in pixels at the current size.
* `set_font(&'static Font)` — switch to a custom font (default `FONT_5X7`).

`FONT_5X7` is a built-in ASCII `0x20..=0x7E` font (95 glyphs). Custom fonts
use the same Adafruit GFX `GFXfont` layout (see `src/fonts.rs` for the struct
definition), so fonts produced by `fontconvert` or similar tools can be copied
into a `static Font` almost verbatim.

Please reach out if you need a `.qgif`-style font converter!

## Pixel / shape drawing

* `set_pixel(x, y, on)` — single pixel in the internal buffer.
* `fb()` / `fb_mut()` — read or directly edit the internal frame buffer.
* `draw_frame(&buf)` — push a caller-supplied page-major buffer.
* `clear()` / `clear_buffer()` / `flush()` — buffer management.

## Supported panels

| Config                | Controller | Resolution | Notes                  |
|-----------------------|-----------|------------|------------------------|
| `ssd1306_128x64()`    | SSD1306   | 128x64     | most common 0.96"      |
| `ssd1306_128x32()`    | SSD1306   | 128x32     | 0.96" / 0.91" half     |
| `sh1106_128x64()`     | SH1106    | 128x64     | 1.3" (column offset 2) |

## License

MIT OR Apache-2.0