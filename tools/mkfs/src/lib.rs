//! Build a Phase 6b filesystem image (see `kernel_common::fs`) from a set of
//! named files. Pure host code; the in-kernel reader parses what this writes.

use kernel_common::fs::{
    block_count, DirEntry, Superblock, BLOCK_SIZE, DATA_START_BLOCK, DIRENTS_PER_BLOCK,
    DIRENT_SIZE, DIR_BLOCK, FS_MAGIC, FS_VERSION,
};

/// Build a raw disk image: block 0 superblock, block 1 directory, block 2+
/// contiguous file data. Panics if more than one directory block is needed
/// (6b keeps a single directory block).
pub fn build_image(files: &[(&str, &[u8])]) -> Vec<u8> {
    assert!(files.len() <= DIRENTS_PER_BLOCK, "too many files for one directory block");
    let mut entries: Vec<(DirEntry, &[u8])> = Vec::new();
    let mut next = DATA_START_BLOCK;
    let mut data_blocks = 0u32;
    for (name, bytes) in files {
        let nb = block_count(bytes.len() as u32);
        entries.push((DirEntry::new(name, next, bytes.len() as u32), *bytes));
        next += nb;
        data_blocks += nb;
    }
    let total_blocks = DATA_START_BLOCK + data_blocks;
    let mut img = vec![0u8; total_blocks as usize * BLOCK_SIZE];

    let sb = Superblock {
        magic: FS_MAGIC,
        version: FS_VERSION,
        block_size: BLOCK_SIZE as u32,
        dir_block: DIR_BLOCK,
        dir_entries: files.len() as u32,
        total_blocks,
    };
    sb.encode(&mut img[0..BLOCK_SIZE]);

    let dir_off = DIR_BLOCK as usize * BLOCK_SIZE;
    for (i, (ent, _)) in entries.iter().enumerate() {
        let off = dir_off + i * DIRENT_SIZE;
        ent.encode(&mut img[off..off + DIRENT_SIZE]);
    }
    for (ent, bytes) in &entries {
        let off = ent.start_block as usize * BLOCK_SIZE;
        img[off..off + bytes.len()].copy_from_slice(bytes);
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel_common::fs::{lookup, Superblock};

    #[test]
    fn build_then_parse_round_trips_two_files() {
        let small = b"hi".to_vec();
        let big = vec![b'Z'; 600]; // spans two blocks
        let img = build_image(&[("alpha", &small), ("big", &big)]);

        let sb = Superblock::decode(&img[0..BLOCK_SIZE]).expect("valid superblock");
        assert_eq!(sb.dir_entries, 2);
        // total: superblock + dir + ceil(2/512)=1 + ceil(600/512)=2 = 5 blocks
        assert_eq!(sb.total_blocks, 5);

        let dir = &img[BLOCK_SIZE..2 * BLOCK_SIZE];
        let a = lookup(dir, sb.dir_entries, "alpha").unwrap();
        let b = lookup(dir, sb.dir_entries, "big").unwrap();
        assert_eq!(a.byte_len, 2);
        assert_eq!(b.byte_len, 600);
        // file bytes land at their start_block and read back intact
        let a_off = a.start_block as usize * BLOCK_SIZE;
        assert_eq!(&img[a_off..a_off + 2], b"hi");
        let b_off = b.start_block as usize * BLOCK_SIZE;
        assert_eq!(&img[b_off..b_off + 600], &big[..]);
    }
}
