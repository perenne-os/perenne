//! ns16550 UART transmit driver (memory-mapped I/O).
//!
//! A 16550-compatible UART exposes byte registers; with `reg-shift = s`
//! they are spaced `1 << s` bytes apart. For output we use the transmit
//! holding register (THR, offset 0) and the line status register (LSR,
//! offset `5 << s`), whose THRE bit says THR can accept a byte. OpenSBI has
//! already configured the line (baud, 8N1); we only transmit.

/// LSR "transmit holding register empty" bit.
const LSR_THRE: u8 = 0x20;

/// Transmit one byte on the ns16550 at MMIO `base` (registers spaced by
/// `reg_shift`), spinning until the holding register is empty.
///
/// # Safety
/// `base` must be the MMIO base of an ns16550 UART that is mapped readable
/// and writable in the current address space.
#[cfg(target_arch = "riscv64")]
pub unsafe fn put(base: usize, reg_shift: u32, byte: u8) {
    let lsr = (base + (5usize << reg_shift)) as *const u8;
    let thr = base as *mut u8;
    // SAFETY: caller guarantees `base` is a mapped ns16550 register window;
    // THR/LSR are valid byte registers within it.
    unsafe {
        while core::ptr::read_volatile(lsr) & LSR_THRE == 0 {
            core::hint::spin_loop();
        }
        core::ptr::write_volatile(thr, byte);
    }
}

/// LSR "data ready" bit — a received byte is waiting in the RBR.
const LSR_DR: u8 = 0x01;
/// IER "received-data-available interrupt enable" bit.
const IER_RDA: u8 = 0x01;

/// Read one received byte from the ns16550 at `base` (registers spaced by
/// `reg_shift`) iff the line status register reports data ready; `None`
/// otherwise. Reading the receive holding register (RBR, offset 0) deasserts
/// the device's RX interrupt.
///
/// # Safety
/// `base` must be the MMIO base of an ns16550 UART mapped readable/writable in
/// the current address space.
#[cfg(target_arch = "riscv64")]
pub unsafe fn get(base: usize, reg_shift: u32) -> Option<u8> {
    let lsr = (base + (5usize << reg_shift)) as *const u8;
    let rbr = base as *const u8;
    // SAFETY: caller guarantees a mapped ns16550 register window.
    unsafe {
        if core::ptr::read_volatile(lsr) & LSR_DR == 0 {
            return None;
        }
        Some(core::ptr::read_volatile(rbr))
    }
}

/// Enable the received-data-available interrupt (IER bit 0, offset `1 << shift`).
/// OpenSBI already configured the line; we only turn on RX interrupts.
///
/// # Safety
/// As [`get`]: `base` must be a mapped ns16550 register window.
#[cfg(target_arch = "riscv64")]
pub unsafe fn enable_rx_interrupt(base: usize, reg_shift: u32) {
    let ier = (base + (1usize << reg_shift)) as *mut u8;
    // SAFETY: caller guarantees a mapped ns16550 register window.
    unsafe {
        core::ptr::write_volatile(ier, IER_RDA);
    }
}
