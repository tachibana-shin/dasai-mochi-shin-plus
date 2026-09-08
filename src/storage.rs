//! Durable settings persistence (brightness, ...) stored in the on-board SPI
//! flash, mirroring the "FlashStorage" concept from the original project.
//!
//! We use `esp_storage::ll` (the low-level ROM SPI-flash read/write/erase
//! functions — the same mechanism `esp-storage`'s high-level `FlashStorage`
//! is built on). The config lives in a single dedicated 4 KB sector.
//!
//! # Storage location / safety
//!
//! `STORAGE_OFFSET` must be sector-aligned, above the end of the application
//! image, and within flash on both 2 MB and 4 MB modules. The current firmware
//! image ends around `0xDB000` (app base `0x10000` + ~824 KB), so `0x1F0000`
//! is far clear of it on either module.
//!
//! A full sector is erased on every save (the section's value is small), which
//! is fine given how rarely the user changes the brightness.

use esp_storage::ll;

use crate::config;
use crate::drivers::screen::{BURN_IN_DEFAULT_INDEX, BURN_IN_OPTIONS};
use crate::ui::clock::{
    FACES, INTERLUDE_DEFAULT_INDEX, INTERLUDE_OPTIONS, INTERLUDE_TIME_DEFAULT_INDEX,
    INTERLUDE_TIME_OPTIONS, ROTATE_DEFAULT_INDEX, ROTATE_OPTIONS,
};

/// Physical flash offset of the config sector (4 KB aligned).
const STORAGE_OFFSET: u32 = 0x001F_0000;
/// Flash sector size in bytes (ESP32-C3 NOR flash).
const SECTOR_SIZE: u32 = 4096;
/// Bytes we use in the sector:
/// `[magic: 4][brightness: 1][clock_style: 1][gmt_offset: i32][rotate: 1][burn_in: 1][interlude: 1][interlude_time: 1]`.
/// Brightness lives at byte 4 (unchanged from the original layout for
/// compatibility); the clock-face style previously sat in the reserved byte at
/// index 5; the UTC offset occupies bytes 6..10; the auto-rotate interval
/// (index into [`ROTATE_OPTIONS`]) occupies byte 10; the burn-in shift interval
/// (index into [`BURN_IN_OPTIONS`]) byte 11 (formerly padding); the interlude
/// interval (index into [`INTERLUDE_OPTIONS`]) and duration (index into
/// [`INTERLUDE_TIME_OPTIONS`]) bytes 12 and 13 (formerly erased `0xFF`, so any
/// out-of-range read clamps to the defaults). Total 16.
const STORAGE_LEN: usize = 16;
/// Byte index of the brightness value within the sector.
const BRIGHTNESS_BYTE: usize = 4;
/// Byte index of the clock-face style (`0` = analog, `1` = digital, `2` = palm).
const CLOCK_STYLE_BYTE: usize = 5;
/// Byte index of the UTC offset (i32, little-endian) within the sector.
const GMT_OFFSET_BYTE: usize = 6;
/// Byte index of the auto-rotate interval (index into [`ROTATE_OPTIONS`],
/// `0` = rotation off) within the sector.
const ROTATE_BYTE: usize = 10;
/// Byte index of the burn-in shift interval (index into [`BURN_IN_OPTIONS`])
/// within the sector. Was `0x00` padding in older firmware, which is why index
/// `0` in [`BURN_IN_OPTIONS`] is the default "30 min" and not "Off".
const BURN_IN_BYTE: usize = 11;
/// Byte index of the clock-interlude interval (index into [`INTERLUDE_OPTIONS`],
/// `0` = interlude off) within the sector.
const INTERLUDE_BYTE: usize = 12;
/// Byte index of the clock-interlude duration (index into
/// [`INTERLUDE_TIME_OPTIONS`]) within the sector.
const INTERLUDE_TIME_BYTE: usize = 13;

/// Magic identifier distinguishing "ours" from freshly-erased (`0xFF`) flash.
const MAGIC: u32 = 0xD45A_4A55;

/// Read 16 raw bytes from the config sector into a little-endian byte slice.
///
/// `raw` is a word-aligned `[u32; 4]` (required by the ROM read path); the
/// returned slice views the same memory as 16 consecutive bytes, which is the
/// exact byte order stored in flash (RISC-V is little-endian).
unsafe fn raw_bytes(raw: &[u32; 4]) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (i, w) in raw.iter().enumerate() {
        out[i * 4] = *w as u8;
        out[i * 4 + 1] = (*w >> 8) as u8;
        out[i * 4 + 2] = (*w >> 16) as u8;
        out[i * 4 + 3] = (*w >> 24) as u8;
    }
    out
}

/// Read the whole config sector into a byte slice (`None` if magic is missing).
fn read_sector() -> Option<[u8; STORAGE_LEN]> {
    let mut raw = [0u32; STORAGE_LEN / 4];
    // SAFETY: `raw` is word-aligned and `STORAGE_OFFSET + STORAGE_LEN` is within
    // bounds (verified offset far below the flash top on 2 MB / 4 MB modules).
    let ok = unsafe { ll::spiflash_read(STORAGE_OFFSET, raw.as_mut_ptr(), STORAGE_LEN as u32) }.is_ok();
    if !ok {
        return None;
    }
    let bytes = unsafe { raw_bytes(&raw) };
    let magic = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if magic != MAGIC {
        return None;
    }
    Some(bytes)
}

/// All persisted user settings, mirroring the config-sector byte layout. Every
/// save operation read-modify-writes a full `Settings` so no other field is
/// ever lost (the whole sector is erased on each save).
#[derive(Clone, Copy)]
struct Settings {
    brightness: u8,
    gmt: i32,
    style: u8,
    rotate: u8,
    burn_in: u8,
    interlude: u8,
    interlude_time: u8,
}

/// Clamp-and-decode the raw byte at `index` against an option-table length,
/// falling back to `default` when nothing valid is stored (fresh flash or an
/// out-of-range byte, e.g. erased `0xFF`).
fn decode(len: usize, bytes: &[u8; STORAGE_LEN], index: usize, default: u8) -> u8 {
    let v = bytes[index];
    if v >= len as u8 {
        default
    } else {
        v
    }
}

/// Read the whole config sector and decode every setting, or return all
/// defaults if the magic is missing (fresh flash).
fn load_settings() -> Settings {
    let Some(bytes) = read_sector() else {
        return Settings {
            brightness: config::BRIGHT.default,
            gmt: config::TIME.gmt_offset_hours,
            style: config::CLOCK_DEFAULT_STYLE,
            rotate: ROTATE_DEFAULT_INDEX,
            burn_in: BURN_IN_DEFAULT_INDEX,
            interlude: INTERLUDE_DEFAULT_INDEX,
            interlude_time: INTERLUDE_TIME_DEFAULT_INDEX,
        };
    };
    let decode_at = |index: usize, len: usize, default: u8| decode(len, &bytes, index, default);
    let gmt = i32::from_le_bytes([
        bytes[GMT_OFFSET_BYTE],
        bytes[GMT_OFFSET_BYTE + 1],
        bytes[GMT_OFFSET_BYTE + 2],
        bytes[GMT_OFFSET_BYTE + 3],
    ]);
    Settings {
        brightness: bytes[BRIGHTNESS_BYTE].clamp(config::BRIGHT.min, config::BRIGHT.max),
        gmt: if gmt < crate::time::GMT_MIN || gmt > crate::time::GMT_MAX {
            config::TIME.gmt_offset_hours
        } else {
            gmt
        },
        style: decode_at(CLOCK_STYLE_BYTE, FACES.len(), config::CLOCK_DEFAULT_STYLE),
        rotate: decode_at(ROTATE_BYTE, ROTATE_OPTIONS.len(), ROTATE_DEFAULT_INDEX),
        burn_in: decode_at(BURN_IN_BYTE, BURN_IN_OPTIONS.len(), BURN_IN_DEFAULT_INDEX),
        interlude: decode_at(INTERLUDE_BYTE, INTERLUDE_OPTIONS.len(), INTERLUDE_DEFAULT_INDEX),
        interlude_time: decode_at(
            INTERLUDE_TIME_BYTE,
            INTERLUDE_TIME_OPTIONS.len(),
            INTERLUDE_TIME_DEFAULT_INDEX,
        ),
    }
}

/// Erase the config sector and write every setting in one pass. `raw` is
/// word-aligned and the erase/write offsets are verified safe (see module
/// docs); the ROM functions suspend the flash cache around the operation,
/// exactly as esp-storage does.
fn write_settings(s: &Settings) {
    let magic = MAGIC.to_le_bytes();
    let mut bytes = [0u8; STORAGE_LEN];
    bytes[0..4].copy_from_slice(&magic);
    bytes[BRIGHTNESS_BYTE] = s.brightness;
    bytes[CLOCK_STYLE_BYTE] = s.style;
    bytes[GMT_OFFSET_BYTE..GMT_OFFSET_BYTE + 4].copy_from_slice(&s.gmt.to_le_bytes());
    bytes[ROTATE_BYTE] = s.rotate;
    bytes[BURN_IN_BYTE] = s.burn_in;
    bytes[INTERLUDE_BYTE] = s.interlude;
    bytes[INTERLUDE_TIME_BYTE] = s.interlude_time;
    let mut raw = [0u32; STORAGE_LEN / 4];
    for (i, block) in bytes.chunks_exact(4).enumerate() {
        raw[i] = u32::from_le_bytes([block[0], block[1], block[2], block[3]]);
    }
    // SAFETY: same bounds rationale as `read_sector`.
    unsafe {
        let _ = ll::spiflash_unlock();
        let _ = ll::spiflash_erase_sector(STORAGE_OFFSET / SECTOR_SIZE);
        let _ = ll::spiflash_write(STORAGE_OFFSET, raw.as_ptr(), STORAGE_LEN as u32);
    }
}

/// Load the stored brightness, or the default if nothing valid is stored.
pub fn load_brightness() -> u8 {
    load_settings().brightness
}

/// Persist the brightness value across reboots.
pub fn save_brightness(value: u8) {
    let mut s = load_settings();
    s.brightness = value;
    write_settings(&s);
}

/// Public clock-style loader, used at boot to restore the chosen face.
pub fn load_clock_style() -> u8 {
    load_settings().style
}

/// Persist the clock-face style across reboots.
pub fn save_clock_style(value: u8) {
    let mut s = load_settings();
    s.style = value.min((FACES.len() - 1) as u8);
    write_settings(&s);
}

/// Public auto-rotate loader, used at boot to restore the interval setting.
pub fn load_rotate_interval() -> u8 {
    load_settings().rotate
}

/// Persist the auto-rotate interval across reboots.
pub fn save_rotate_interval(value: u8) {
    let mut s = load_settings();
    s.rotate = value.min((ROTATE_OPTIONS.len() - 1) as u8);
    write_settings(&s);
}

/// Public burn-in interval loader, used at boot to restore the setting.
pub fn load_burn_in_interval() -> u8 {
    load_settings().burn_in
}

/// Persist the burn-in shift interval across reboots.
pub fn save_burn_in_interval(value: u8) {
    let mut s = load_settings();
    s.burn_in = value.min((BURN_IN_OPTIONS.len() - 1) as u8);
    write_settings(&s);
}

/// Public UTC-offset loader, used at boot to restore the persisted timezone.
pub fn load_gmt_offset() -> i32 {
    load_settings().gmt
}

/// Persist the UTC offset across reboots.
pub fn save_gmt_offset(value: i32) {
    let mut s = load_settings();
    s.gmt = value.clamp(crate::time::GMT_MIN, crate::time::GMT_MAX);
    write_settings(&s);
}

/// Public interlude-interval loader, used at boot to restore the setting.
pub fn load_interlude() -> u8 {
    load_settings().interlude
}

/// Persist the interlude interval across reboots.
pub fn save_interlude(value: u8) {
    let mut s = load_settings();
    s.interlude = value.min((INTERLUDE_OPTIONS.len() - 1) as u8);
    write_settings(&s);
}

/// Public interlude-duration loader, used at boot to restore the setting.
pub fn load_interlude_time() -> u8 {
    load_settings().interlude_time
}

/// Persist the interlude duration across reboots.
pub fn save_interlude_time(value: u8) {
    let mut s = load_settings();
    s.interlude_time = value.min((INTERLUDE_TIME_OPTIONS.len() - 1) as u8);
    write_settings(&s);
}

// ---------------------------------------------------------------------------
// Wi-Fi credentials (SSID + password) for the SoftAP provisioning flow.
// Stored in their own dedicated sector (`0x1F1000`), one sector above the
// brightness config, with the same safety rationale (offset is far below the
// flash top on 2 MB / 4 MB modules and sectors are erased+rewritten atomically).
// ---------------------------------------------------------------------------

/// Physical flash offset of the Wi-Fi credential sector (4 KB aligned).
const WIFI_OFFSET: u32 = 0x001F_1000;
/// Magic distinguishing saved Wi-Fi credentials from erased (`0xFF`) flash.
const WIFI_MAGIC: u32 = 0x5749_4649; // "WIFI"
/// Max SSID byte length (Wi-Fi standard upper bound).
const WIFI_SSID_MAX: usize = 32;
/// Max WPA2 password byte length.
const WIFI_PASS_MAX: usize = 64;
/// Bytes used in each wifi sector:
/// `[magic:4][ssid_len:1][ssid:32][pass_len:1][pass:64]` = 102,
/// rounded up to a 4-byte-aligned read length of 104 for the ROM read path.
const WIFI_STORAGE_LEN: usize = 4 + 1 + WIFI_SSID_MAX + 1 + WIFI_PASS_MAX;
const WIFI_READ_LEN: usize = (WIFI_STORAGE_LEN + 3) & !3; // 104
/// Max number of distinct Wi-Fi networks that can be saved (one per slot).
pub const MAX_WIFI_NETWORKS: usize = 8;

/// Saved Wi-Fi credentials (network SSID + password).
#[derive(Clone)]
pub struct WifiCreds {
    pub ssid: heapless::String<WIFI_SSID_MAX>,
    pub password: heapless::String<WIFI_PASS_MAX>,
}

/// Read a single credential slot at `offset`, returning `None` if it is empty
/// (erased) or corrupt / out-of-range.
fn load_creds_slot(offset: u32) -> Option<WifiCreds> {
    let mut raw = [0u32; WIFI_READ_LEN / 4];
    // SAFETY: `raw` is word-aligned and every slot `offset + WIFI_READ_LEN` is
    // within flash bounds (all slots sit in the reserved `0x1F1xxx` block,
    // far below the flash top on 2 MB / 4 MB modules).
    let ok = unsafe { ll::spiflash_read(offset, raw.as_mut_ptr(), WIFI_READ_LEN as u32) }.is_ok();
    if !ok {
        return None;
    }
    let mut bytes = [0u8; WIFI_READ_LEN];
    for (i, w) in raw.iter().enumerate() {
        bytes[i * 4] = *w as u8;
        bytes[i * 4 + 1] = (*w >> 8) as u8;
        bytes[i * 4 + 2] = (*w >> 16) as u8;
        bytes[i * 4 + 3] = (*w >> 24) as u8;
    }
    let magic = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if magic != WIFI_MAGIC {
        return None;
    }
    let ssid_len = bytes[4] as usize;
    let pass_len = bytes[4 + 1 + WIFI_SSID_MAX] as usize;
    if ssid_len == 0 || ssid_len > WIFI_SSID_MAX || pass_len > WIFI_PASS_MAX {
        return None;
    }
    let ssid = core::str::from_utf8(&bytes[5..5 + ssid_len]).ok()?;
    let pass =
        core::str::from_utf8(&bytes[5 + 1 + WIFI_SSID_MAX..5 + 1 + WIFI_SSID_MAX + pass_len]).ok()?;
    Some(WifiCreds {
        ssid: heapless::String::try_from(ssid).ok()?,
        password: heapless::String::try_from(pass).ok()?,
    })
}

/// Read every saved Wi-Fi credential, in slot order (later slots are preferred
/// for matching, see [`save_wifi_creds`]).
pub fn load_all_wifi_creds() -> heapless::Vec<WifiCreds, MAX_WIFI_NETWORKS> {
    let mut list: heapless::Vec<WifiCreds, MAX_WIFI_NETWORKS> = heapless::Vec::new();
    for i in 0..MAX_WIFI_NETWORKS {
        let offset = WIFI_OFFSET + (i as u32) * SECTOR_SIZE;
        if let Some(creds) = load_creds_slot(offset) {
            let _ = list.push(creds);
        }
    }
    list
}

/// Read the first saved Wi-Fi credential, or `None` if none are stored.
#[allow(dead_code)]
pub fn load_wifi_creds() -> Option<WifiCreds> {
    load_all_wifi_creds().first().cloned()
}

fn write_creds_slot(offset: u32, creds: &WifiCreds) {
    let ssid = creds.ssid.as_bytes();
    let pass = creds.password.as_bytes();
    let ssid_len = ssid.len();
    let pass_len = pass.len();
    let mut bytes = [0u8; WIFI_READ_LEN];
    let magic = WIFI_MAGIC.to_le_bytes();
    bytes[0..4].copy_from_slice(&magic);
    bytes[4] = ssid_len as u8;
    bytes[5..5 + ssid_len].copy_from_slice(ssid);
    bytes[4 + 1 + WIFI_SSID_MAX] = pass_len as u8;
    bytes[5 + 1 + WIFI_SSID_MAX..5 + 1 + WIFI_SSID_MAX + pass_len].copy_from_slice(pass);
    let mut raw = [0u32; WIFI_READ_LEN / 4];
    for (i, block) in bytes.chunks_exact(4).enumerate() {
        raw[i] = u32::from_le_bytes([block[0], block[1], block[2], block[3]]);
    }
    // SAFETY: slot `offset + WIFI_READ_LEN` is in bounds (same rationale as
    // `load_creds_slot`); `raw` is word-aligned and the ROM functions suspend
    // the flash cache around the operation.
    unsafe {
        let _ = ll::spiflash_unlock();
        let _ = ll::spiflash_erase_sector(offset / SECTOR_SIZE);
        let _ = ll::spiflash_write(offset, raw.as_ptr(), WIFI_READ_LEN as u32);
    }
}

/// Persist Wi-Fi credentials, updating the matching SSID in place or appending
/// it as a new network (up to [`MAX_WIFI_NETWORKS`]).  Other saved networks are
/// left untouched, so the device can remember several networks and pick the
/// one that is available at any given time.
pub fn save_wifi_creds(creds: &WifiCreds) {
    let list = load_all_wifi_creds();

    // If this SSID is already known, update its password in the slot it lives in.
    if let Some(idx) = list.iter().position(|e| e.ssid == creds.ssid) {
        // Write the updated password into its existing slot.
        let mut updated = list[idx].clone();
        updated.password = creds.password.clone();
        write_creds_slot(WIFI_OFFSET + (idx as u32) * SECTOR_SIZE, &updated);
        // Rotate the just-updated network to slot 0 (preferred) by swapping
        // slot 0 and slot `idx`'s contents.
        if idx != 0 {
            let prev = load_creds_slot(WIFI_OFFSET);
            write_creds_slot(WIFI_OFFSET, &updated);
            if let Some(p) = prev {
                write_creds_slot(WIFI_OFFSET + (idx as u32) * SECTOR_SIZE, &p);
            }
        }
        return;
    }

    // New network: write to the first free slot (or shift out the oldest if full).
    let mut slot = list.len();
    if slot >= MAX_WIFI_NETWORKS {
        slot = MAX_WIFI_NETWORKS - 1;
        for i in 0..MAX_WIFI_NETWORKS - 1 {
            if let Some(c) = load_creds_slot(WIFI_OFFSET + (i as u32 + 1) * SECTOR_SIZE) {
                write_creds_slot(WIFI_OFFSET + (i as u32) * SECTOR_SIZE, &c);
            }
        }
    }
    write_creds_slot(WIFI_OFFSET + (slot as u32) * SECTOR_SIZE, creds);
}

/// Remove the saved network whose SSID matches `ssid`, collapsing the remaining
/// slots so the saved list stays contiguous from slot 0.  Returns `true` if a
/// network was actually removed.
pub fn remove_wifi_creds(ssid: &str) -> bool {
    let list = load_all_wifi_creds();
    // Collect the SSIDs we want to keep (everything except `ssid`).
    let mut keep: heapless::Vec<heapless::String<WIFI_SSID_MAX>, MAX_WIFI_NETWORKS> =
        heapless::Vec::new();
    for e in &list {
        if e.ssid.as_str() != ssid {
            let _ = keep.push(e.ssid.clone());
        }
    }
    if keep.len() == list.len() {
        // Nothing matched (or list was already empty).
        return false;
    }
    // Erase all slots, then rewrite the kept networks contiguously from slot 0.
    for i in 0..MAX_WIFI_NETWORKS {
        let offset = WIFI_OFFSET + (i as u32) * SECTOR_SIZE;
        unsafe {
            let _ = ll::spiflash_unlock();
            let _ = ll::spiflash_erase_sector(offset / SECTOR_SIZE);
        }
    }
    for (i, saved_ssid) in keep.iter().enumerate() {
        if let Some(creds) = list.iter().find(|e| e.ssid == *saved_ssid) {
            write_creds_slot(WIFI_OFFSET + (i as u32) * SECTOR_SIZE, creds);
        }
    }
    true
}

