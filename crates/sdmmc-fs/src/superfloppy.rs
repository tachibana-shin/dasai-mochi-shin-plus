//! The [`SuperFloppy`] block-device wrapper and its MBR patching.

use embedded_sdmmc::{Block, BlockCount, BlockDevice, BlockIdx};

/// A block device wrapper that makes a "superfloppy" SD card (FAT directly on
/// block 0, no MBR partition table) look like an MBR-partitioned card.
///
/// `embedded-sdmmc` 0.9 only supports MBR-partitioned volumes.  Many SD cards
/// (especially ones formatted by phones / cameras / some OSes) are
/// "superfloppies": the FAT boot sector is in block 0 with no partition table.
/// This wrapper patches block 0 on read so that a synthetic partition table
/// entry points at LBA 0 (the superfloppy data), making embedded-sdmmc able to
/// open it as `VolumeIdx(0)`.
pub struct SuperFloppy<D: BlockDevice> {
    inner: D,
}

impl<D: BlockDevice> SuperFloppy<D> {
    /// Wrap `inner`, patching block 0 reads so the card looks MBR-partitioned.
    pub fn new(inner: D) -> SuperFloppy<D> {
        SuperFloppy { inner }
    }
}

impl<D: BlockDevice> BlockDevice for SuperFloppy<D> {
    type Error = D::Error;

    fn read(&self, blocks: &mut [Block], start_block_idx: BlockIdx) -> Result<(), Self::Error> {
        self.inner.read(blocks, start_block_idx)?;
        // Patch block 0 with a synthetic MBR partition table.
        if start_block_idx.0 == 0 && !blocks.is_empty() {
            patch_superfloppy_mbr(&mut blocks[0], self.inner.num_blocks()?);
        }
        Ok(())
    }

    fn write(&self, blocks: &[Block], start_block_idx: BlockIdx) -> Result<(), Self::Error> {
        self.inner.write(blocks, start_block_idx)
    }

    fn num_blocks(&self) -> Result<BlockCount, Self::Error> {
        self.inner.num_blocks()
    }
}

/// Patch the first block of a superfloppy card so embedded-sdmmc sees a valid
/// MBR partition table entry pointing at LBA 0 with the correct FAT type.
pub fn patch_superfloppy_mbr(block: &mut Block, total_blocks: BlockCount) {
    // Sniff FAT16 vs FAT32 from the BPB:
    //   FAT16 stores sectors-per-FAT in bytes 22..24 (non-zero).
    //   FAT32 stores 0 there and uses the 32-bit field at bytes 36..40.
    let sectors_per_fat_16 = u16::from_le_bytes([block.contents[22], block.contents[23]]);
    let part_type: u8 = if sectors_per_fat_16 == 0 {
        0x0C // FAT32 LBA
    } else {
        0x0E // FAT16 LBA
    };

    const PT_START: usize = 446; // first MBR partition entry
    // 16-byte entry:
    //   0: status (0x80 bootable, 0x00 non-bootable)
    //   1..3: CHS start (ignore, set 0)
    //   4: partition type
    //   5..7: CHS end (ignore)
    //   8..11: LBA start (little-endian u32)
    //   12..15: number of sectors (little-endian u32)
    block.contents[PT_START] = 0x00; // non-bootable
    block.contents[PT_START + 1..PT_START + 4].fill(0);
    block.contents[PT_START + 4] = part_type;
    block.contents[PT_START + 5..PT_START + 8].fill(0);
    // LBA start = 0 (superfloppy data starts at block 0)
    block.contents[PT_START + 8..PT_START + 12].copy_from_slice(&0u32.to_le_bytes());
    block.contents[PT_START + 12..PT_START + 16]
        .copy_from_slice(&total_blocks.0.to_le_bytes());

    // Zero out the other three partition entries so they don't confuse things.
    for e in 1..4 {
        let base = PT_START + e * 16;
        block.contents[base..base + 16].fill(0);
    }
    // Keep the 0x55 0xAA boot signature at bytes 510..512 (already there).
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_sdmmc::{Block, BlockCount, BlockDevice, BlockIdx};
    use heapless::Vec;

    /// A tiny in-memory `BlockDevice` holding a fixed number of 512-byte blocks.
    struct RamDisk {
        blocks: Vec<Block, 8>,
    }

    impl RamDisk {
        /// Build a FAT-looking card: block 0 gets a boot-sector signature
        /// (0x55 0xAA) and, if `fat16` is true, a non-zero sectors-per-FAT16
        /// field (byte 22) so the MBR patcher picks FAT16 instead of FAT32.
        fn new(count: usize, fat16: bool) -> Self {
            let mut v = Vec::new();
            for _ in 0..count {
                v.push(Block::new()).ok();
            }
            v[0].contents[510] = 0x55;
            v[0].contents[511] = 0xAA;
            if fat16 {
                v[0].contents[22] = 0x09;
            }
            RamDisk { blocks: v }
        }
    }

    impl BlockDevice for RamDisk {
        type Error = ();
        fn read(&self, blocks: &mut [Block], start: BlockIdx) -> Result<(), ()> {
            for (i, b) in blocks.iter_mut().enumerate() {
                let idx = start.0 as usize + i;
                if idx >= self.blocks.len() {
                    return Err(());
                }
                *b = self.blocks[idx].clone();
            }
            Ok(())
        }
        fn write(&self, blocks: &[Block], start: BlockIdx) -> Result<(), ()> {
            for (i, _b) in blocks.iter().enumerate() {
                let idx = start.0 as usize + i;
                if idx >= self.blocks.len() {
                    return Err(());
                }
            }
            Ok(())
        }
        fn num_blocks(&self) -> Result<BlockCount, ()> {
            Ok(BlockCount(self.blocks.len() as u32))
        }
    }

    #[test]
    fn patches_fat32_style_superfloppy_mbr() {
        // fat16 = false -> byte 22 stays 0 -> patcher chooses FAT32 (0x0C).
        let disk = RamDisk::new(4, false);
        let sf = SuperFloppy::new(disk);

        let mut blocks = [Block::new()];
        sf.read(&mut blocks, BlockIdx(0)).unwrap();

        let b = &blocks[0].contents;
        // Boot signature preserved (from the card's block 0).
        assert_eq!(b[510], 0x55);
        assert_eq!(b[511], 0xAA);
        // First partition entry at 446: status 0, type 0x0C (FAT32 LBA).
        assert_eq!(b[446], 0x00);
        assert_eq!(b[450], 0x0C);
        // LBA start == 0.
        assert_eq!(u32::from_le_bytes([b[454], b[455], b[456], b[457]]), 0);
        // Number of sectors == total blocks.
        assert_eq!(u32::from_le_bytes([b[458], b[459], b[460], b[461]]), 4);
        // Other partition entries zeroed.
        for p in 1..4 {
            let base = 446 + p * 16;
            assert_eq!(b[base], 0x00);
            assert_eq!(b[base + 4], 0x00);
            assert_eq!(u32::from_le_bytes([b[base + 8], b[base + 9], b[base + 10], b[base + 11]]), 0);
        }
    }

    #[test]
    fn patches_fat16_style_superfloppy_mbr() {
        // fat16 = true -> byte 22 non-zero -> patcher chooses FAT16 (0x0E).
        let disk = RamDisk::new(4, true);
        let sf = SuperFloppy::new(disk);

        let mut blocks = [Block::new()];
        sf.read(&mut blocks, BlockIdx(0)).unwrap();

        let b = &blocks[0].contents;
        assert_eq!(b[450], 0x0E); // FAT16 LBA
        assert_eq!(b[446], 0x00); // non-bootable
        assert_eq!(b[510], 0x55); // boot signature preserved
    }

    #[test]
    fn passes_through_block1_unchanged() {
        let disk = RamDisk::new(4, false);
        let sf = SuperFloppy::new(disk);

        let mut blocks = [Block::new()];
        sf.read(&mut blocks, BlockIdx(1)).unwrap();

        let b = &blocks[0].contents;
        // Block 1 is NOT patched: the whole MBR area stays untouched zeroes.
        for p in 0..4 {
            let base = 446 + p * 16;
            assert_eq!(b[base], 0x00);
            assert_eq!(b[base + 4], 0x00);
        }
        // And it does not gain a boot signature either (that lives on block 0).
        assert_eq!(b[510], 0x00);
    }
}
