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
// 1 << 4 is U (user-accessible) — deliberately absent until Phase 3.
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
pub fn pte_to_pa(pte: u64) -> usize {
    ((pte >> 10) << 12) as usize
}

pub fn pte_is_valid(pte: u64) -> bool {
    pte & PTE_V != 0
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
    fn zero_pte_is_invalid() {
        assert!(!pte_is_valid(0));
    }
}
