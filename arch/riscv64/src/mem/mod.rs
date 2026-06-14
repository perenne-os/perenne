//! Memory management: physical frame allocation and Sv39 paging.
//!
//! Same layout discipline as [`crate::trap`]: pure logic is ungated and
//! unit-tested on the host; everything touching real memory or CSRs is
//! gated to `target_arch = "riscv64"`.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

pub mod frame;
pub mod paging;

/// Interior mutability for a single-hart kernel: gives `&mut` access to
/// a static without a real lock. `timer.rs` gets away with a bare
/// `AtomicU64`, but a bitmap-plus-counters struct can't be a single
/// atomic, so the exclusivity argument moves here and is enforced:
/// re-entrant access (e.g. a trap handler interrupting a call in
/// progress) panics. In 2b trap context never allocates; the panic
/// keeps that invariant honest if a later phase forgets.
pub struct SingleHartCell<T> {
    inner: UnsafeCell<T>,
    in_use: AtomicBool,
}

// SAFETY: one hart, and `with` panics on re-entry, so the `&mut` handed
// to the closure is never aliased.
unsafe impl<T> Sync for SingleHartCell<T> {}

impl<T> SingleHartCell<T> {
    pub const fn new(value: T) -> Self {
        Self { inner: UnsafeCell::new(value), in_use: AtomicBool::new(false) }
    }

    /// Run `f` with exclusive access to the value.
    ///
    /// If `f` panics the cell stays locked and every later call panics
    /// as "re-entrant" — acceptable because a kernel panic is already
    /// fatal (abort, no unwinding), so the stuck flag is unreachable.
    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        assert!(
            !self.in_use.swap(true, Ordering::Acquire),
            "re-entrant access to single-hart cell"
        );
        // SAFETY: the in_use flag guarantees no other &mut exists (see
        // the Sync impl note).
        let result = f(unsafe { &mut *self.inner.get() });
        self.in_use.store(false, Ordering::Release);
        result
    }
}

/// End of RAM on QEMU virt with `-m 128M` (pinned by the run/test
/// scripts). **QEMU-specific constant** like timer.rs's TIMEBASE_HZ —
/// real hardware (Phase 4) must read the memory map from the device
/// tree instead.
#[cfg(target_arch = "riscv64")]
const RAM_END: usize = 0x8800_0000;

#[cfg(target_arch = "riscv64")]
extern "C" {
    static __text_start: u8;
    static __text_end: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __data_start: u8;
    static __data_end: u8;
    static __stack_start: u8;
    static __stack_top: u8;
    static __kernel_end: u8;
    static __user_text_start: u8;
    static __user_text_end: u8;
    static __user_data_start: u8;
    static __user_data_end: u8;
}

/// The address of a linker-script symbol. Only the address is
/// meaningful; the "value" must never be read. Must be called from
/// an `unsafe` context (the caller vouches that the symbol is defined
/// by `kernel.ld` and the address is valid).
#[cfg(target_arch = "riscv64")]
macro_rules! sym {
    ($name:ident) => {
        // addr_of! takes the symbol's address without dereferencing it.
        core::ptr::addr_of!($name) as usize
    };
}

/// Bring memory management up: arm the frame allocator over free RAM,
/// identity-map the kernel with W^X section permissions, enable Sv39.
///
/// Call exactly once, after `trap::init()` (a paging mistake should
/// fault loudly, not hang) and before `timer::start()` (no interrupts
/// while the world is being remapped).
#[cfg(target_arch = "riscv64")]
pub fn init() {
    use paging::{PTE_G, PTE_R, PTE_U, PTE_W, PTE_X};

    // SAFETY: all sym! calls read linker-script symbol addresses (not
    // their contents) from kernel.ld; the ranges are 4 KiB-aligned by
    // the linker script. The MMU is still off, so writes land in the
    // physical addresses we own.
    unsafe {
        let free_ram = (sym!(__kernel_end), RAM_END);
        frame::ALLOCATOR.with(|a| a.init(free_ram.0, free_ram.1));

        let root = frame::alloc_zeroed().expect("no frame for root page table").0
            as *mut paging::PageTable;
        paging::map_range(root, sym!(__text_start), sym!(__text_end), PTE_R | PTE_X | PTE_G);
        paging::map_range(root, sym!(__rodata_start), sym!(__rodata_end), PTE_R | PTE_G);
        paging::map_range(root, sym!(__data_start), sym!(__data_end), PTE_R | PTE_W | PTE_G);
        // The guard page between __data_end and __stack_start stays
        // unmapped: stack overflow faults instead of corrupting .bss.
        paging::map_range(root, sym!(__stack_start), sym!(__stack_top), PTE_R | PTE_W | PTE_G);
        // Free RAM mapped eagerly so allocated frames are immediately
        // usable — no fault-and-map machinery in 2b.
        paging::map_range(root, free_ram.0, free_ram.1, PTE_R | PTE_W | PTE_G);
        // Phase 3a: the embedded user image gets the U bit so a U-mode
        // task can fetch/read/write it. NOT global (G): user mappings are
        // not shared across address spaces. text R-X-U, data RW-U. Empty
        // until kmain places code/data here (start == end maps nothing).
        paging::map_range(root, sym!(__user_text_start), sym!(__user_text_end), PTE_R | PTE_X | PTE_U);
        paging::map_range(root, sym!(__user_data_start), sym!(__user_data_end), PTE_R | PTE_W | PTE_U);
        // SAFETY: everything the kernel touches is now identity-mapped.
        crate::csr::satp_write(crate::csr::SATP_MODE_SV39 | (root as usize >> 12));
    }
}

/// Frames currently free (for boot diagnostics).
#[cfg(target_arch = "riscv64")]
pub fn free_frames() -> usize {
    frame::ALLOCATOR.with(|a| a.free_frames())
}

/// Frames managed in total (for boot diagnostics).
#[cfg(target_arch = "riscv64")]
pub fn total_frames() -> usize {
    frame::ALLOCATOR.with(|a| a.total_frames())
}

/// Half-open bounds `[start, end)` of the `.user_data` region — the only
/// memory a `U`-mode task may legitimately ask the kernel to read in a
/// `print` syscall. Used by the confused-deputy guard.
#[cfg(target_arch = "riscv64")]
pub fn user_data_bounds() -> (usize, usize) {
    // SAFETY: both are linker-script symbol addresses (read as addresses,
    // never dereferenced here); the region is defined by kernel.ld.
    unsafe { (sym!(__user_data_start), sym!(__user_data_end)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_gives_mutable_access() {
        static CELL: SingleHartCell<u32> = SingleHartCell::new(7);
        let seen = CELL.with(|v| {
            *v += 1;
            *v
        });
        assert_eq!(seen, 8);
    }

    #[test]
    #[should_panic(expected = "re-entrant")]
    fn reentrant_access_panics() {
        static CELL: SingleHartCell<u32> = SingleHartCell::new(0);
        CELL.with(|_| CELL.with(|_| {}));
    }
}
