//! Sv39 paging: three-level page tables mapping 39-bit virtual
//! addresses, 4 KiB leaf pages only (megapages are a deferred
//! optimization — one code path for now).
//!
//! Pure here: PTE encoding and virtual-address index math, host-tested.
//! Gated below (later task): the `PageTable` walker that touches real
//! memory.

/// Size of one page (= one frame): 4 KiB.
pub const PAGE_SIZE: usize = 4096;

/// PTE flag bits (Sv39 leaf and non-leaf entries share the layout).
pub const PTE_V: u64 = 1 << 0; // valid
pub const PTE_R: u64 = 1 << 1; // readable
pub const PTE_W: u64 = 1 << 2; // writable
pub const PTE_X: u64 = 1 << 3; // executable
pub const PTE_U: u64 = 1 << 4; // user-accessible (Phase 3a)
pub const PTE_G: u64 = 1 << 5; // global: valid in every address space
pub const PTE_A: u64 = 1 << 6; // accessed
pub const PTE_D: u64 = 1 << 7; // dirty

/// The 9-bit page-table index for `va` at `level` (2 = root, 0 = leaf).
pub fn vpn(va: usize, level: usize) -> usize {
    (va >> (12 + 9 * level)) & 0x1ff
}

/// Build a PTE pointing at physical address `pa` (4 KiB-aligned) with
/// `flags`; V is always set. PTE layout: PPN in bits 53:10, flags 9:0.
pub fn pte_for(pa: usize, flags: u64) -> u64 {
    ((pa as u64 >> 12) << 10) | flags | PTE_V
}

/// The physical address a PTE points at.
/// Masks the 44-bit PPN field so reserved bits 63:54 can't leak in.
pub fn pte_to_pa(pte: u64) -> usize {
    (((pte >> 10) & 0xFFF_FFFF_FFFF) << 12) as usize
}

pub fn pte_is_valid(pte: u64) -> bool {
    pte & PTE_V != 0
}

/// One Sv39 page table: 512 PTEs, exactly one 4 KiB frame.
#[cfg(target_arch = "riscv64")]
#[repr(C, align(4096))]
pub struct PageTable {
    pub entries: [u64; 512],
}

/// Map the 4 KiB page at virtual `va` to physical `pa` in the tree
/// rooted at `root`, creating intermediate tables from the frame
/// allocator as needed. Panics if `va` is already mapped: in 2b every
/// mapping is made exactly once, so a remap is a bug.
///
/// Leaf PTEs get `A` always and `D` when writable, set up front: the
/// spec lets an MMU fault instead of setting them in hardware, and we
/// have no swapping that would want the information.
///
/// # Safety
/// `root` must point at a valid (zero-initialized or in-construction)
/// page table, and `pa` at memory the caller may map. Called with the
/// MMU off (mem::init) or on identity-mapped table frames.
#[cfg(target_arch = "riscv64")]
pub unsafe fn map_page(root: *mut PageTable, va: usize, pa: usize, flags: u64) {
    assert!(va % PAGE_SIZE == 0 && pa % PAGE_SIZE == 0, "map_page: unaligned va={va:#x} pa={pa:#x}");
    let mut table = root;
    for level in [2, 1] {
        let idx = vpn(va, level);
        // SAFETY: `table` is a valid table per the contract; idx < 512.
        let pte = unsafe { (*table).entries[idx] };
        table = if pte_is_valid(pte) {
            pte_to_pa(pte) as *mut PageTable
        } else {
            let frame = super::frame::alloc_zeroed().expect("out of frames for page table");
            // Non-leaf PTE: V only (R/W/X all zero means "next level").
            // SAFETY: same as the read above.
            unsafe { (*table).entries[idx] = pte_for(frame.0, 0) };
            frame.0 as *mut PageTable
        };
    }
    let idx = vpn(va, 0);
    // SAFETY: `table` now points at the leaf-level table.
    unsafe {
        assert!(!pte_is_valid((*table).entries[idx]), "remap of va {va:#x}");
        let dirty = if flags & PTE_W != 0 { PTE_D } else { 0 };
        (*table).entries[idx] = pte_for(pa, flags | PTE_A | dirty);
    }
}

/// Identity-map `[start, end)` (4 KiB-aligned) with `flags`.
///
/// # Safety
/// Same contract as [`map_page`].
#[cfg(target_arch = "riscv64")]
pub unsafe fn map_range(root: *mut PageTable, start: usize, end: usize, flags: u64) {
    let mut addr = start;
    while addr < end {
        // SAFETY: forwarded from the caller.
        unsafe { map_page(root, addr, addr, flags) };
        addr += PAGE_SIZE;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vpn_extracts_all_three_levels() {
        // 0x8020_0000: bits 38:30 = 2, bits 29:21 = 1, bits 20:12 = 0.
        let va = 0x8020_0000;
        assert_eq!(vpn(va, 2), 2);
        assert_eq!(vpn(va, 1), 1);
        assert_eq!(vpn(va, 0), 0);
    }

    #[test]
    fn vpn_isolates_nine_bits() {
        // All-ones VA: every level reads 0x1ff, nothing bleeds across.
        let va = usize::MAX;
        for level in 0..3 {
            assert_eq!(vpn(va, level), 0x1ff);
        }
    }

    #[test]
    fn pte_round_trips_the_physical_address() {
        let pa = 0x8020_1000;
        let pte = pte_for(pa, PTE_R | PTE_X);
        assert_eq!(pte_to_pa(pte), pa);
    }

    #[test]
    fn pte_for_sets_valid_and_keeps_flags() {
        let pte = pte_for(0x8030_0000, PTE_R | PTE_W);
        assert!(pte_is_valid(pte));
        assert_ne!(pte & PTE_R, 0);
        assert_ne!(pte & PTE_W, 0);
        assert_eq!(pte & PTE_X, 0);
    }

    #[test]
    fn pte_for_carries_the_user_bit() {
        let pte = pte_for(0x8030_0000, PTE_R | PTE_X | PTE_U);
        assert!(pte_is_valid(pte));
        assert_ne!(pte & PTE_U, 0);
        assert_ne!(pte & PTE_X, 0);
    }

    #[test]
    fn zero_pte_is_invalid() {
        assert!(!pte_is_valid(0));
    }

    #[test]
    fn pte_to_pa_ignores_reserved_high_bits() {
        let pa = 0x8020_1000;
        let pte = pte_for(pa, PTE_R) | (1 << 62); // poison a reserved bit
        assert_eq!(pte_to_pa(pte), pa);
    }
}
