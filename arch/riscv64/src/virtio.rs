//! virtio-mmio (modern, Version 2) constants and the RNG probe — the kernel
//! side of the user-space entropy driver.
//!
//! The kernel only *discovers* the RNG (probes the transport slots for the
//! entropy DeviceID) and hands the component its mapped MMIO + DMA. The
//! component does the handshake/virtqueue itself (in U-mode inline asm), so it
//! references these `const`s directly (each folds to an immediate — no call
//! into kernel `.text`, no `.rodata` read). All values are spike-verified
//! against QEMU's `virtio-rng-device` on the `virt` machine.

// --- virtio-mmio register offsets (modern, Version 2) ---
pub const MAGIC: usize = 0x000;
pub const VERSION: usize = 0x004;
pub const DEVICE_ID: usize = 0x008;
pub const DEVICE_FEATURES: usize = 0x010;
pub const DEVICE_FEATURES_SEL: usize = 0x014;
pub const DRIVER_FEATURES: usize = 0x020;
pub const DRIVER_FEATURES_SEL: usize = 0x024;
pub const QUEUE_SEL: usize = 0x030;
pub const QUEUE_NUM: usize = 0x038;
pub const QUEUE_READY: usize = 0x044;
pub const QUEUE_NOTIFY: usize = 0x050;
pub const STATUS: usize = 0x070;
pub const QUEUE_DESC_LOW: usize = 0x080;
pub const QUEUE_DESC_HIGH: usize = 0x084;
pub const QUEUE_DRIVER_LOW: usize = 0x090;
pub const QUEUE_DRIVER_HIGH: usize = 0x094;
pub const QUEUE_DEVICE_LOW: usize = 0x0a0;
pub const QUEUE_DEVICE_HIGH: usize = 0x0a4;

// --- magic / device id / status bits / feature / descriptor flag ---
pub const MAGIC_VALUE: u32 = 0x7472_6976; // "virt"
pub const DEVICE_ID_RNG: u32 = 4;
pub const STATUS_ACK: u32 = 1;
pub const STATUS_DRIVER: u32 = 2;
pub const STATUS_DRIVER_OK: u32 = 4;
pub const STATUS_FEATURES_OK: u32 = 8;
/// `VIRTIO_F_VERSION_1` is feature bit 32 — i.e. bit 0 of the high word
/// (DeviceFeaturesSel = 1).
pub const F_VERSION_1_HI: u32 = 1;
pub const VIRTQ_DESC_F_WRITE: u16 = 2;

// --- split-virtqueue + DMA layout within one zeroed 4 KiB frame (QSIZE=8) ---
pub const VQ_SIZE: u32 = 8;
pub const VQ_DESC_OFF: usize = 0;
pub const VQ_AVAIL_OFF: usize = 128;
pub const VQ_USED_OFF: usize = 160;
pub const VQ_BUF_OFF: usize = 256;

/// Is this DeviceID the entropy source?
pub fn is_rng(device_id: u32) -> bool {
    device_id == DEVICE_ID_RNG
}

/// Probe `bases` (the discovered virtio-mmio slots) for the RNG: the first
/// slot whose Magic is "virt" and DeviceID is 4. Called once in early boot
/// (MMU off), like reading the device tree.
///
/// # Safety
/// Each non-zero base must address a valid virtio-mmio register page.
#[cfg(target_arch = "riscv64")]
pub unsafe fn find_rng(bases: &[usize]) -> Option<usize> {
    for &b in bases {
        if b != 0
            && core::ptr::read_volatile((b + MAGIC) as *const u32) == MAGIC_VALUE
            && core::ptr::read_volatile((b + DEVICE_ID) as *const u32) == DEVICE_ID_RNG
        {
            return Some(b);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_rng_matches_only_the_entropy_id() {
        assert!(is_rng(4));
        assert!(!is_rng(0)); // empty slot
        assert!(!is_rng(2)); // block device
    }

    #[test]
    fn dma_layout_is_aligned_and_non_overlapping() {
        // desc table is QSIZE*16 bytes; avail must follow it.
        assert!(VQ_AVAIL_OFF >= VQ_DESC_OFF + VQ_SIZE as usize * 16);
        // used ring (16-aligned) must follow the avail ring
        // (flags 2 + idx 2 + ring 2*QSIZE + used_event 2).
        assert_eq!(VQ_USED_OFF % 16, 0, "used ring 16-aligned");
        assert!(VQ_USED_OFF >= VQ_AVAIL_OFF + 4 + 2 * VQ_SIZE as usize + 2);
        // buffer must follow the used ring (flags 2 + idx 2 + ring 8*QSIZE + 2)
        // and stay within the 4 KiB DMA frame.
        assert!(VQ_BUF_OFF >= VQ_USED_OFF + 4 + 8 * VQ_SIZE as usize + 2);
        assert!(VQ_BUF_OFF + 32 <= 4096, "32-byte buffer fits in the frame");
    }
}
