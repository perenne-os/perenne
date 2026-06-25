//! A minimal, read-only on-disk filesystem format shared by the in-kernel
//! reader (Phase 6b) and the host `mkfs` tool: a superblock, a flat directory
//! of fixed-size entries, and contiguous file extents. 512-byte blocks.
//!
//! Pure layout logic — no I/O, no arch code — so it is host-tested and used
//! identically on both sides (define the format once).

/// Bytes per block; equals the virtio-blk sector size, so one FS block is one
/// device request (no block/sector translation).
pub const BLOCK_SIZE: usize = 512;
/// Superblock magic — the ASCII bytes "6BFS" little-endian.
pub const FS_MAGIC: u32 = 0x5346_4236;
/// On-disk format version.
pub const FS_VERSION: u32 = 1;
/// The directory occupies block 1 (block 0 is the superblock).
pub const DIR_BLOCK: u32 = 1;
/// First block available for file data.
pub const DATA_START_BLOCK: u32 = 2;
/// Bytes reserved for a NUL-padded name in a directory entry.
pub const NAME_LEN: usize = 48;
/// On-disk size of one directory entry.
pub const DIRENT_SIZE: usize = 64;
/// Directory entries that fit in one block.
pub const DIRENTS_PER_BLOCK: usize = BLOCK_SIZE / DIRENT_SIZE;

/// The first block of the image: locates the directory and bounds the volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Superblock {
    pub magic: u32,
    pub version: u32,
    pub block_size: u32,
    pub dir_block: u32,
    pub dir_entries: u32,
    pub total_blocks: u32,
}

impl Superblock {
    /// Encode into the start of a block buffer (first 24 bytes; rest untouched).
    pub fn encode(&self, block: &mut [u8]) {
        block[0..4].copy_from_slice(&self.magic.to_le_bytes());
        block[4..8].copy_from_slice(&self.version.to_le_bytes());
        block[8..12].copy_from_slice(&self.block_size.to_le_bytes());
        block[12..16].copy_from_slice(&self.dir_block.to_le_bytes());
        block[16..20].copy_from_slice(&self.dir_entries.to_le_bytes());
        block[20..24].copy_from_slice(&self.total_blocks.to_le_bytes());
    }

    /// Decode and validate. Returns `None` if magic or version mismatch.
    pub fn decode(block: &[u8]) -> Option<Superblock> {
        if block.len() < 24 {
            return None;
        }
        let rd = |o: usize| u32::from_le_bytes([block[o], block[o + 1], block[o + 2], block[o + 3]]);
        let sb = Superblock {
            magic: rd(0),
            version: rd(4),
            block_size: rd(8),
            dir_block: rd(12),
            dir_entries: rd(16),
            total_blocks: rd(20),
        };
        if sb.magic != FS_MAGIC || sb.version != FS_VERSION {
            return None;
        }
        Some(sb)
    }
}

/// One directory entry: maps a name to a contiguous file extent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirEntry {
    pub name: [u8; NAME_LEN],
    pub start_block: u32,
    pub byte_len: u32,
}

impl DirEntry {
    /// Build from a name (truncated to `NAME_LEN`, NUL-padded).
    pub fn new(name: &str, start_block: u32, byte_len: u32) -> DirEntry {
        let mut n = [0u8; NAME_LEN];
        let b = name.as_bytes();
        let k = core::cmp::min(b.len(), NAME_LEN);
        n[..k].copy_from_slice(&b[..k]);
        DirEntry { name: n, start_block, byte_len }
    }

    /// Encode into a `DIRENT_SIZE`-byte slice (trailing 8 bytes reserved/zero).
    pub fn encode(&self, e: &mut [u8]) {
        e[..NAME_LEN].copy_from_slice(&self.name);
        e[NAME_LEN..NAME_LEN + 4].copy_from_slice(&self.start_block.to_le_bytes());
        e[NAME_LEN + 4..NAME_LEN + 8].copy_from_slice(&self.byte_len.to_le_bytes());
    }

    /// Decode from a `DIRENT_SIZE`-byte slice.
    pub fn decode(e: &[u8]) -> DirEntry {
        let mut name = [0u8; NAME_LEN];
        name.copy_from_slice(&e[..NAME_LEN]);
        let start_block = u32::from_le_bytes([e[NAME_LEN], e[NAME_LEN + 1], e[NAME_LEN + 2], e[NAME_LEN + 3]]);
        let byte_len = u32::from_le_bytes([e[NAME_LEN + 4], e[NAME_LEN + 5], e[NAME_LEN + 6], e[NAME_LEN + 7]]);
        DirEntry { name, start_block, byte_len }
    }

    /// The entry's name as a `&str` (NUL padding trimmed).
    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&c| c == 0).unwrap_or(NAME_LEN);
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }

    /// Does this entry's (NUL-padded) name equal `want`?
    pub fn name_is(&self, want: &str) -> bool {
        let w = want.as_bytes();
        if w.len() > NAME_LEN {
            return false;
        }
        let end = self.name.iter().position(|&c| c == 0).unwrap_or(NAME_LEN);
        &self.name[..end] == w
    }
}

/// Number of blocks a file of `byte_len` bytes occupies (ceil division).
pub fn block_count(byte_len: u32) -> u32 {
    (byte_len + BLOCK_SIZE as u32 - 1) / BLOCK_SIZE as u32
}

/// Decode the `i`-th directory entry packed in one directory block.
/// `None` if `i` is past the block.
pub fn dir_entry_at(dir_bytes: &[u8], i: usize) -> Option<DirEntry> {
    let off = i * DIRENT_SIZE;
    if i >= DIRENTS_PER_BLOCK || off + DIRENT_SIZE > dir_bytes.len() {
        return None;
    }
    Some(DirEntry::decode(&dir_bytes[off..off + DIRENT_SIZE]))
}

/// Find the entry named `name` among the first `count` entries packed in one
/// directory block `dir_bytes`. `None` if absent.
pub fn lookup(dir_bytes: &[u8], count: u32, name: &str) -> Option<DirEntry> {
    let n = core::cmp::min(count as usize, DIRENTS_PER_BLOCK);
    for i in 0..n {
        let off = i * DIRENT_SIZE;
        if off + DIRENT_SIZE > dir_bytes.len() {
            break;
        }
        let e = DirEntry::decode(&dir_bytes[off..off + DIRENT_SIZE]);
        if e.name_is(name) {
            return Some(e);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn superblock_round_trips() {
        let sb = Superblock {
            magic: FS_MAGIC, version: FS_VERSION, block_size: BLOCK_SIZE as u32,
            dir_block: DIR_BLOCK, dir_entries: 2, total_blocks: 5,
        };
        let mut block = [0u8; BLOCK_SIZE];
        sb.encode(&mut block);
        assert_eq!(Superblock::decode(&block), Some(sb));
    }

    #[test]
    fn superblock_decode_rejects_bad_magic_or_version() {
        let mut block = [0u8; BLOCK_SIZE];
        Superblock { magic: 0xdead, version: FS_VERSION, block_size: 512,
            dir_block: 1, dir_entries: 0, total_blocks: 2 }.encode(&mut block);
        assert_eq!(Superblock::decode(&block), None);
        let mut block2 = [0u8; BLOCK_SIZE];
        Superblock { magic: FS_MAGIC, version: 99, block_size: 512,
            dir_block: 1, dir_entries: 0, total_blocks: 2 }.encode(&mut block2);
        assert_eq!(Superblock::decode(&block2), None);
    }

    #[test]
    fn dirent_round_trips_and_name_matches() {
        let e = DirEntry::new("KB-0005", 2, 574);
        let mut buf = [0u8; DIRENT_SIZE];
        e.encode(&mut buf);
        let d = DirEntry::decode(&buf);
        assert_eq!(d.start_block, 2);
        assert_eq!(d.byte_len, 574);
        assert!(d.name_is("KB-0005"));
        assert!(!d.name_is("KB-0004"));
        assert!(!d.name_is("KB-0005x"));
    }

    #[test]
    fn block_count_rounds_up() {
        assert_eq!(block_count(0), 0);
        assert_eq!(block_count(1), 1);
        assert_eq!(block_count(512), 1);
        assert_eq!(block_count(513), 2);
        assert_eq!(block_count(1024), 2);
    }

    #[test]
    fn dir_entry_at_and_name_str_enumerate() {
        let mut dir = [0u8; BLOCK_SIZE];
        DirEntry::new("KB-0005", 2, 10).encode(&mut dir[0..DIRENT_SIZE]);
        DirEntry::new("KB-0004", 3, 20).encode(&mut dir[DIRENT_SIZE..2 * DIRENT_SIZE]);
        assert_eq!(dir_entry_at(&dir, 0).unwrap().name_str(), "KB-0005");
        assert_eq!(dir_entry_at(&dir, 1).unwrap().name_str(), "KB-0004");
        assert!(dir_entry_at(&dir, DIRENTS_PER_BLOCK).is_none()); // out of block
    }

    #[test]
    fn lookup_finds_present_and_rejects_absent() {
        let mut dir = [0u8; BLOCK_SIZE];
        DirEntry::new("alpha", 2, 10).encode(&mut dir[0..DIRENT_SIZE]);
        DirEntry::new("beta", 3, 20).encode(&mut dir[DIRENT_SIZE..2 * DIRENT_SIZE]);
        assert_eq!(lookup(&dir, 2, "alpha").unwrap().start_block, 2);
        assert_eq!(lookup(&dir, 2, "beta").unwrap().byte_len, 20);
        assert!(lookup(&dir, 2, "gamma").is_none());
        assert!(lookup(&dir, 0, "alpha").is_none()); // count gates the scan
    }
}
