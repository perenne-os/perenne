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
    unsafe { asm!("csrr {}, time", out(reg) value, options(nostack, nomem)) };
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
    unsafe { asm!("csrw stvec, {}", in(reg) addr, options(nostack, nomem)) };
}

/// Enable supervisor timer interrupts (`sie.STIE`, bit 5).
///
/// # Safety
/// A trap handler must be installed via [`stvec_write`] first.
///
/// We use `csrs` (register form) instead of `csrsi` because bit 5 = 32
/// exceeds `csrsi`'s 5-bit immediate range (0–31).
#[inline]
pub unsafe fn sie_enable_timer() {
    unsafe { asm!("csrs sie, {}", in(reg) 1usize << 5, options(nostack, nomem)) };
}

/// Globally enable supervisor interrupts (`sstatus.SIE`, bit 1).
///
/// # Safety
/// A trap handler must be installed via [`stvec_write`] first.
#[inline]
pub unsafe fn sstatus_enable_interrupts() {
    unsafe { asm!("csrsi sstatus, 0x2", options(nostack, nomem)) };
}

/// `satp` mode field (bits 63:60) selecting Sv39 translation.
pub const SATP_MODE_SV39: usize = 8 << 60;

/// Point address translation at a root page table and switch it on:
/// `value` = mode bits | root-table PPN (physical address >> 12).
/// Fenced on both sides so no stale translations straddle the switch.
///
/// # Safety
/// The table must identity-map every address the kernel touches —
/// the code executing this function, its stack, and all statics —
/// with correct permissions. Anything less turns the instruction
/// after `csrw` into a page fault (or a silent wild fetch).
// (No `nomem` on satp_write, unlike the other accessors: this changes how
// all memory is addressed, and the compiler must not cache across it.)
#[inline]
pub unsafe fn satp_write(value: usize) {
    unsafe {
        asm!(
            "sfence.vma",
            "csrw satp, {}",
            "sfence.vma",
            in(reg) value,
            options(nostack),
        )
    };
}
