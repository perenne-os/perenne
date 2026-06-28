//! Memory management: physical frame allocation and Sv39 paging.
//!
//! Same layout discipline as [`crate::trap`]: pure logic is ungated and
//! unit-tested on the host; everything touching real memory or CSRs is
//! gated to `target_arch = "riscv64"`.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

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

/// The master kernel `satp`, saved by [`init`] and handed to kernel
/// (S-mode) tasks via [`kernel_satp`]. Per-task user spaces clone the
/// kernel into their own trees (see [`build_user_space`]).
#[cfg(target_arch = "riscv64")]
static KERNEL_SATP: AtomicUsize = AtomicUsize::new(0);

/// MMIO base of the console UART (from the device tree), saved by [`init`]
/// so [`map_kernel_sections`] can map its page into the master table and
/// every per-task tree — the kernel prints from trap handlers while a user
/// task's `satp` is active, so the page must exist in all of them. Zero
/// before `init`.
#[cfg(target_arch = "riscv64")]
static UART_MMIO_BASE: AtomicUsize = AtomicUsize::new(0);

/// MMIO base of the PLIC (from the device tree), saved by [`init`] so
/// [`map_kernel_sections`] maps its pages into the master table and every
/// per-task tree — the external interrupt handler touches the PLIC while a
/// user task's `satp` is active. Zero before `init`.
#[cfg(target_arch = "riscv64")]
static PLIC_MMIO_BASE: AtomicUsize = AtomicUsize::new(0);

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

/// Identity-map the kernel *image* — `.text` R-X, `.rodata` R, `.data`/
/// `.bss` RW, the boot stack RW — all **global** (`PTE_G`), into `root`.
/// Reused by the master table and by every per-task user tree (the kernel
/// must be present in every address space because `PTE_G` is only a TLB
/// hint, not a substitute for the mapping existing in the walked tree).
/// Deliberately excludes free RAM and the user sections.
///
/// # Safety
/// `root` must point at a valid (zeroed/in-construction) page table; called
/// with free RAM identity-mapped (MMU off in `init`, or the master `satp`
/// when building a user tree) so the page-table writes land.
#[cfg(target_arch = "riscv64")]
unsafe fn map_kernel_sections(root: *mut paging::PageTable) {
    use paging::{PTE_G, PTE_R, PTE_W, PTE_X};
    // SAFETY: forwarded; all ranges are kernel.ld symbols, page-aligned.
    // The guard page between __data_end and __stack_start stays unmapped:
    // stack overflow faults instead of corrupting .bss.
    unsafe {
        paging::map_range(root, sym!(__text_start), sym!(__text_end), PTE_R | PTE_X | PTE_G);
        paging::map_range(root, sym!(__rodata_start), sym!(__rodata_end), PTE_R | PTE_G);
        paging::map_range(root, sym!(__data_start), sym!(__data_end), PTE_R | PTE_W | PTE_G);
        paging::map_range(root, sym!(__stack_start), sym!(__stack_top), PTE_R | PTE_W | PTE_G);
        // Device MMIO: the console UART, one page, R-W-G (kernel-only, not
        // executable). Mapped in every tree because the kernel prints from
        // trap handlers while a user task's satp is active. Unset (0) on the
        // earliest path means "nothing to map yet".
        let uart = UART_MMIO_BASE.load(Ordering::Acquire);
        if uart != 0 {
            paging::map_range(root, uart, uart + paging::PAGE_SIZE, PTE_R | PTE_W | PTE_G);
        }
        // The PLIC: priority/pending/enable around the base (3 pages) and the
        // context-1 threshold/claim page. R-W-G, kernel-only, in every tree.
        let plic = PLIC_MMIO_BASE.load(Ordering::Acquire);
        if plic != 0 {
            paging::map_range(root, plic, plic + 0x3000, PTE_R | PTE_W | PTE_G);
            paging::map_range(root, plic + 0x20_1000, plic + 0x20_2000, PTE_R | PTE_W | PTE_G);
        }
    }
}

/// Bring memory management up: arm the frame allocator over free RAM,
/// build the master kernel page table (kernel image + free RAM, all
/// global; the user sections belong to per-task trees now — see
/// [`build_user_space`]), enable Sv39, and save the kernel `satp`.
///
/// `ram_end` is the (exclusive) end of physical RAM and `uart_base` the
/// console UART's MMIO base — both discovered from the device tree (see
/// [`crate::dt`]). The UART page is mapped into the master table and every
/// per-task tree (via [`map_kernel_sections`]).
///
/// Call exactly once, after `trap::init()` (a paging mistake should
/// fault loudly, not hang) and before `timer::start()` (no interrupts
/// while the world is being remapped).
#[cfg(target_arch = "riscv64")]
pub fn init(ram_end: usize, uart_base: usize, plic_base: usize) {
    use paging::{PTE_G, PTE_R, PTE_W};

    // SAFETY: all sym! calls read linker-script symbol addresses (not
    // their contents) from kernel.ld; the ranges are 4 KiB-aligned by
    // the linker script. The MMU is still off, so writes land in the
    // physical addresses we own.
    unsafe {
        // Record the UART base BEFORE building any page table, so
        // map_kernel_sections maps its page into the master table (and,
        // later, every per-task tree built by build_user_space).
        UART_MMIO_BASE.store(uart_base, Ordering::Release);
        PLIC_MMIO_BASE.store(plic_base, Ordering::Release);

        let free_ram = (sym!(__kernel_end), ram_end);
        frame::ALLOCATOR.with(|a| a.init(free_ram.0, free_ram.1));

        let root = frame::alloc_zeroed().expect("no frame for root page table").0
            as *mut paging::PageTable;
        map_kernel_sections(root);
        // Free RAM mapped eagerly so allocated frames are immediately
        // usable — no fault-and-map machinery in 2b. The master table
        // (used by the kernel and at boot) needs it; per-task user trees
        // deliberately do NOT map free RAM (see build_user_space).
        paging::map_range(root, free_ram.0, free_ram.1, PTE_R | PTE_W | PTE_G);

        // SAFETY: everything the kernel touches is now identity-mapped.
        let satp = paging::make_satp(root as usize);
        KERNEL_SATP.store(satp, Ordering::Release);
        crate::csr::satp_write(satp);
    }
}

/// The master kernel `satp`, for kernel (S-mode) tasks. Valid after
/// [`init`].
#[cfg(target_arch = "riscv64")]
pub fn kernel_satp() -> usize {
    KERNEL_SATP.load(Ordering::Acquire)
}

/// Build a private address space for one U-mode task and return its `satp`.
/// The tree clones the kernel image (global), maps the shared `.user_text`
/// (R-X-U) code, this task's own page-aligned `stack` (RW-U), and one
/// per-component `device` page (R-U) — a read-only data page, or a device's
/// MMIO that this component exclusively owns (mapped here and nowhere else,
/// so it is the component's device "capability"). Other tasks' pages are
/// absent → a cross-task or cross-device access faults. Both regions are
/// half-open `(start, end)`, page-aligned; pass `(0, 0)` for none.
///
/// Call at spawn time, while the master `satp` is active (the new tree's
/// frames come from free RAM, which only the master table maps).
#[cfg(target_arch = "riscv64")]
pub fn build_user_space(stack: (usize, usize), device: (usize, usize)) -> usize {
    use paging::{PTE_R, PTE_U, PTE_W, PTE_X};
    // SAFETY: a fresh zeroed root; map_kernel_sections + the user ranges
    // are valid (linker symbols / page-aligned statics); built on the
    // master satp so the page-table writes (in free RAM) land.
    unsafe {
        let root = frame::alloc_zeroed()
            .expect("no frame for user root page table")
            .0 as *mut paging::PageTable;
        map_kernel_sections(root);
        paging::map_range(root, sym!(__user_text_start), sym!(__user_text_end), PTE_R | PTE_X | PTE_U);
        paging::map_range(root, stack.0, stack.1, PTE_R | PTE_W | PTE_U);
        paging::map_range(root, device.0, device.1, PTE_R | PTE_U);
        paging::make_satp(root as usize)
    }
}

/// Like [`build_user_space`], but for a virtio component: maps the device's
/// MMIO register page **and** a DMA region **RW-U, identity** (VA=PA) into the
/// component, in addition to `.user_text` (R-X-U) and the `stack` (RW-U). The
/// device DMAs to the physical addresses the component writes into
/// descriptors, so VA must equal PA — both regions are identity-mapped, and
/// mapped only here (the component's exclusive device capability). All three
/// region tuples are half-open `(start, end)`, page-aligned.
#[cfg(target_arch = "riscv64")]
pub fn build_virtio_space(
    stack: (usize, usize),
    mmio: (usize, usize),
    dma: (usize, usize),
) -> usize {
    use paging::{PTE_R, PTE_U, PTE_W, PTE_X};
    // SAFETY: as in build_user_space — fresh zeroed root, valid page-aligned
    // ranges, built on the master satp. MMIO + DMA are RW-U (the device
    // registers are written; the DMA rings are read/written by both sides).
    unsafe {
        let root = frame::alloc_zeroed()
            .expect("no frame for virtio component root page table")
            .0 as *mut paging::PageTable;
        map_kernel_sections(root);
        paging::map_range(root, sym!(__user_text_start), sym!(__user_text_end), PTE_R | PTE_X | PTE_U);
        paging::map_range(root, stack.0, stack.1, PTE_R | PTE_W | PTE_U);
        paging::map_range(root, mmio.0, mmio.1, PTE_R | PTE_W | PTE_U);
        paging::map_range(root, dma.0, dma.1, PTE_R | PTE_W | PTE_U);
        paging::make_satp(root as usize)
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
    // Both are linker-script symbol addresses (taken via addr_of!, never
    // dereferenced here); the region is defined by kernel.ld. No unsafe
    // needed: addr_of! forms an address without reading the static.
    (sym!(__user_data_start), sym!(__user_data_end))
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
