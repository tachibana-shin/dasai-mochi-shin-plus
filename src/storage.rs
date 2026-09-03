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

/// Physical flash offset of the config sector (4 KB aligned).
const STORAGE_OFFSET: u32 = 0x001F_0000;
/// Flash sector size in bytes (ESP32-C3 NOR flash).
const SECTOR_SIZE: u32 = 4096;
/// Bytes we use in the sector: [magic: 4][brightness: 1][pad: 3].
const STORAGE_LEN: usize = 8;

/// Magic identifier distinguishing "ours" from freshly-erased (`0xFF`) flash.
const MAGIC: u32 = 0xD45A_4A55;

/// Read 8 raw bytes from the config sector into a little-endian byte slice.
///
/// `raw` is a word-aligned `[u32; 2]` (required by the ROM read path); the
/// returned slice views the same memory as 8 consecutive bytes, which is the
/// exact byte order stored in flash (RISC-V is little-endian).
unsafe fn raw_bytes(raw: &[u32; 2]) -> [u8; 8] {
    let mut out = [0u8; 8];
    for (i, w) in raw.iter().enumerate() {
        out[i * 4] = *w as u8;
        out[i * 4 + 1] = (*w >> 8) as u8;
        out[i * 4 + 2] = (*w >> 16) as u8;
        out[i * 4 + 3] = (*w >> 24) as u8;
    }
    out
}

/// Load the stored brightness, or the default if nothing valid is stored.
pub fn load_brightness() -> u8 {
    let mut raw = [0u32; STORAGE_LEN / 4];
    // SAFETY: `raw` is word-aligned and `STORAGE_OFFSET + STORAGE_LEN` is within
    // bounds (verified offset far below the flash top on 2 MB / 4 MB modules).
    let ok = unsafe { ll::spiflash_read(STORAGE_OFFSET, raw.as_mut_ptr(), STORAGE_LEN as u32) }.is_ok();
    if !ok {
        return config::BRIGHT.default;
    }
    let bytes = unsafe { raw_bytes(&raw) };
    let magic = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if magic != MAGIC {
        return config::BRIGHT.default;
    }
    bytes[4].clamp(config::BRIGHT.min, config::BRIGHT.max)
}

/// Persist the brightness value across reboots (erases + rewrites the sector).
pub fn save_brightness(value: u8) {
    let magic = MAGIC.to_le_bytes();
    let bytes = [magic[0], magic[1], magic[2], magic[3], value, 0, 0, 0];
    let raw = [
        u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
    ];
    // SAFETY: same bounds rationale as `load_brightness`; `raw` is word-aligned
    // and the ROM functions are safe to call from flash-resident code (they
    // suspend the cache around the operation, exactly as esp-storage does).
    unsafe {
        let _ = ll::spiflash_unlock();
        let _ = ll::spiflash_erase_sector(STORAGE_OFFSET / SECTOR_SIZE);
        let _ = ll::spiflash_write(STORAGE_OFFSET, raw.as_ptr(), STORAGE_LEN as u32);
    }
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

