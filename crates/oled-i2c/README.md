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

```rust
use oled_i2c::{Oled, OledConfig};

let mut display = Oled::new(
    i2c,                      // any embedded_hal::i2c::I2c
    0x3C,                     // 7-bit I2C address
    OledConfig::sh1106_128x64().with_contrast(150),
)
.expect("OLED init failed");

let mut frame = [0u8; 1024];  // 128x64
frame[0] = 0b1010_1010;
display.draw_frame(&frame).ok();
```

## Supported panels

| Config                | Controller | Resolution | Notes                  |
|-----------------------|-----------|------------|------------------------|
| `ssd1306_128x64()`    | SSD1306   | 128x64     | most common 0.96"      |
| `ssd1306_128x32()`    | SSD1306   | 128x32     | 0.96" / 0.91" half     |
| `sh1106_128x64()`     | SH1106    | 128x64     | 1.3" (column offset 2) |

## License

MIT OR Apache-2.0