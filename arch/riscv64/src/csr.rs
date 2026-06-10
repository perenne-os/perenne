//! Control and Status Register (CSR) accessors, hand-rolled.
//!
//! CSRs are per-hart special registers read/written with dedicated
//! instructions (`csrr`, `csrw`, `csrs`). Only the ones Phase 2a
//! actually calls get accessors; the trap entry assembly reads
//! `sepc`/`scause`/`stval` directly into the [`crate::trap::TrapFrame`].

use core::arch::asm;

/// Read the `time` CSR: a wall-clock counter ticking at the platform's
/// timebase frequency (10 MHz on QEMU virt), independent of CPU speed.
#[inline]
pub fn time() -> u64 {
    let value: u64;
    unsafe { asm!("csrr {}, time", out(reg) value) };
    value
}

/// Install the trap vector: all traps jump to `addr`.
///
/// The low two bits select the mode; `00` = direct (one entry for every
/// trap), which `.align 2` on the entry symbol guarantees.
///
/// # Safety
/// `addr` must be the 4-byte-aligned address of a real trap entry that
/// saves/restores state and ends in `sret`. A bogus value turns every
/// trap into a wild jump.
#[inline]
pub unsafe fn stvec_write(addr: usize) {
    unsafe { asm!("csrw stvec, {}", in(reg) addr) };
}

/// Enable supervisor timer interrupts (`sie.STIE`, bit 5).
///
/// # Safety
/// A trap handler must be installed via [`stvec_write`] first.
#[inline]
pub unsafe fn sie_enable_timer() {
    unsafe { asm!("csrs sie, {}", in(reg) 1usize << 5) };
}

/// Globally enable supervisor interrupts (`sstatus.SIE`, bit 1).
///
/// # Safety
/// A trap handler must be installed via [`stvec_write`] first.
#[inline]
pub unsafe fn sstatus_enable_interrupts() {
    unsafe { asm!("csrsi sstatus, 0x2") };
}
