//! Physical frame allocation: a bitmap over 4 KiB frames.
//!
//! One bit per frame (set = allocated); 128 MiB of RAM needs only a
//! 4 KiB bitmap. Chosen over the classic intrusive free-list because the
//! core is pure logic (host-testable) and misuse — double-free,
//! out-of-range free — panics loudly instead of corrupting silently.

/// Size of one physical frame (and one page): 4 KiB.
pub const FRAME_SIZE: usize = 4096;

/// Worst case the bitmap must cover: 128 MiB / 4 KiB.
const MAX_FRAMES: usize = 32_768;

/// A physical frame, identified by its 4 KiB-aligned base address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysFrame(pub usize);

/// Bitmap allocator over a contiguous range of physical frames.
/// Pure logic: no pointers, no CSRs — everything here runs on the host.
pub struct BitmapAllocator {
    /// Bit `i` set = frame `base + i` is allocated.
    bitmap: [u64; MAX_FRAMES / 64],
    /// Frame number (address / FRAME_SIZE) of the first managed frame.
    base: usize,
    /// Number of managed frames.
    count: usize,
    /// Number of currently free frames.
    free: usize,
}

impl BitmapAllocator {
    /// An empty allocator managing nothing; [`init`](Self::init) arms it.
    pub const fn new() -> Self {
        Self { bitmap: [0; MAX_FRAMES / 64], base: 0, count: 0, free: 0 }
    }

    /// Manage the frames in `[start_addr, end_addr)`. Both must be
    /// 4 KiB-aligned; the range must fit the bitmap.
    pub fn init(&mut self, start_addr: usize, end_addr: usize) {
        assert!(start_addr % FRAME_SIZE == 0, "unaligned start {start_addr:#x}");
        assert!(end_addr % FRAME_SIZE == 0, "unaligned end {end_addr:#x}");
        assert!(start_addr < end_addr, "empty range");
        let count = (end_addr - start_addr) / FRAME_SIZE;
        assert!(count <= MAX_FRAMES, "range exceeds bitmap capacity");
        self.base = start_addr / FRAME_SIZE;
        self.count = count;
        self.free = count;
    }

    /// Hand out the lowest free frame (first-fit), or `None` when empty.
    /// The O(n) scan is irrelevant at 32k frames; revisit only if
    /// profiling ever disagrees.
    pub fn alloc(&mut self) -> Option<PhysFrame> {
        for i in 0..self.count {
            let (word, bit) = (i / 64, i % 64);
            if self.bitmap[word] & (1 << bit) == 0 {
                self.bitmap[word] |= 1 << bit;
                self.free -= 1;
                return Some(PhysFrame((self.base + i) * FRAME_SIZE));
            }
        }
        None
    }

    /// Return a frame. Panics on misuse — an unaligned address, an
    /// unmanaged frame, or a double free is a kernel bug worth a loud
    /// stop, not silent corruption.
    pub fn free(&mut self, frame: PhysFrame) {
        assert!(frame.0 % FRAME_SIZE == 0, "free of unaligned address {:#x}", frame.0);
        let n = frame.0 / FRAME_SIZE;
        assert!(
            n >= self.base && n < self.base + self.count,
            "free of unmanaged frame {:#x}",
            frame.0
        );
        let i = n - self.base;
        let (word, bit) = (i / 64, i % 64);
        assert!(self.bitmap[word] & (1 << bit) != 0, "double free of frame {:#x}", frame.0);
        self.bitmap[word] &= !(1 << bit);
        self.free += 1;
    }

    /// Frames currently free.
    pub fn free_frames(&self) -> usize {
        self.free
    }

    /// Frames managed in total.
    pub fn total_frames(&self) -> usize {
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 256 managed frames starting where the kernel image would end.
    fn allocator() -> BitmapAllocator {
        let mut a = BitmapAllocator::new();
        a.init(0x8030_0000, 0x8040_0000);
        a
    }

    #[test]
    fn first_alloc_is_the_lowest_frame() {
        let mut a = allocator();
        assert_eq!(a.alloc(), Some(PhysFrame(0x8030_0000)));
        assert_eq!(a.alloc(), Some(PhysFrame(0x8030_1000)));
    }

    #[test]
    fn free_then_realloc_recycles_the_same_frame() {
        let mut a = allocator();
        let f = a.alloc().unwrap();
        a.alloc().unwrap();
        a.free(f);
        assert_eq!(a.alloc(), Some(f), "first-fit must reuse the freed hole");
    }

    #[test]
    fn exhaustion_returns_none() {
        let mut a = BitmapAllocator::new();
        a.init(0x8030_0000, 0x8030_2000); // exactly 2 frames
        assert!(a.alloc().is_some());
        assert!(a.alloc().is_some());
        assert_eq!(a.alloc(), None);
    }

    #[test]
    fn counts_track_alloc_and_free() {
        let mut a = allocator();
        assert_eq!(a.total_frames(), 256);
        assert_eq!(a.free_frames(), 256);
        let f = a.alloc().unwrap();
        assert_eq!(a.free_frames(), 255);
        a.free(f);
        assert_eq!(a.free_frames(), 256);
    }

    #[test]
    #[should_panic(expected = "double free")]
    fn double_free_panics() {
        let mut a = allocator();
        let f = a.alloc().unwrap();
        a.free(f);
        a.free(f);
    }

    #[test]
    #[should_panic(expected = "unmanaged")]
    fn out_of_range_free_panics() {
        let mut a = allocator();
        a.free(PhysFrame(0x8800_0000));
    }

    #[test]
    #[should_panic(expected = "unaligned")]
    fn unaligned_free_panics() {
        let mut a = allocator();
        a.free(PhysFrame(0x8030_0008));
    }

    #[test]
    fn allocation_crosses_word_boundaries() {
        let mut a = allocator();
        let mut last = None;
        for _ in 0..65 {
            last = a.alloc();
        }
        // Frame index 64 is the first bit of the second bitmap word.
        assert_eq!(last, Some(PhysFrame(0x8030_0000 + 64 * FRAME_SIZE)));
    }

    #[test]
    fn partial_last_word_exhausts_exactly() {
        let mut a = BitmapAllocator::new();
        // 100 frames: one full bitmap word plus 36 bits of the next.
        a.init(0x8030_0000, 0x8030_0000 + 100 * FRAME_SIZE);
        for i in 0..100 {
            assert!(a.alloc().is_some(), "alloc {i} should succeed");
        }
        assert_eq!(a.free_frames(), 0);
        assert_eq!(a.alloc(), None);
        let f = PhysFrame(0x8030_0000 + 99 * FRAME_SIZE);
        a.free(f);
        assert_eq!(a.alloc(), Some(f), "free after exhaustion restores a slot");
    }
}
