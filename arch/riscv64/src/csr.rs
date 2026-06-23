//! Control and Status Register (CSR) accessors, hand-rolled.
//!
//! CSRs are per-hart special registers read/written with dedicated
//! instructions (`csrr`, `csrw`, `csrs`). Only the CSRs the kernel
//! actually uses get accessors; the trap entry assembly reads
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

/// Enable supervisor external interrupts (`sie.SEIE`, bit 9).
///
/// # Safety
/// Only call once a trap handler and the PLIC are set up to service them.
#[inline]
pub unsafe fn sie_enable_external() {
    unsafe { asm!("csrs sie, {}", in(reg) 1usize << 9, options(nostack, nomem)) };
}

/// Globally enable supervisor interrupts (`sstatus.SIE`, bit 1).
///
/// # Safety
/// A trap handler must be installed via [`stvec_write`] first.
#[inline]
pub unsafe fn sstatus_enable_interrupts() {
    unsafe { asm!("csrsi sstatus, 0x2", options(nostack, nomem)) };
}

/// Point address translation at a root page table and switch it on:
/// `value` = mode bits | root-table PPN (physical address >> 12).
/// The trailing `sfence.vma` makes the new root visible; the leading
/// one only flushes translations from any *prior* satp regime (inert at
/// first boot, cheap insurance on a re-load). Ordering of the caller's
/// page-table stores vs. `csrw` needs no fence: same-hart stores are
/// program-ordered, and omitting `nomem` stops compiler reordering.
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

/// Disable supervisor interrupts (`sstatus.SIE`, bit 1), returning
/// whether they were previously enabled. The scheduler uses the return
/// value to restore the caller's prior interrupt state after a context
/// switch, so a task resumed from inside a trap handler does not
/// accidentally run with interrupts unmasked mid-trap-return.
///
/// `csrrci` atomically reads the old `sstatus` and clears the bit.
///
/// # Safety
/// Disabling interrupts is always memory-safe, but the caller owns the
/// obligation to re-enable them (or restore the returned state) so the
/// hart does not stay deaf to the timer forever.
#[inline]
pub unsafe fn sstatus_disable_interrupts() -> bool {
    let prev: usize;
    unsafe {
        asm!("csrrci {}, sstatus, 0x2", out(reg) prev, options(nostack, nomem));
    }
    prev & 0x2 != 0
}

/// Read the whole `sstatus` register. The user-context forge reads it to
/// derive the value to load before `sret`ing into U-mode (clearing SPP,
/// setting SPIE) without disturbing unrelated bits.
#[inline]
pub fn sstatus_read() -> usize {
    let value: usize;
    // SAFETY: a plain CSR read has no memory effects.
    unsafe { asm!("csrr {}, sstatus", out(reg) value, options(nostack, nomem)) };
    value
}

/// Write `sscratch`. The trap entry uses `sscratch` as a privilege-aware
/// stack pointer: the kernel trap-stack top while a user task runs, and
/// `0` (the sentinel meaning "already on a kernel stack") while the kernel
/// runs.
///
/// # Safety
/// A wrong value corrupts the trap-entry stack swap: a non-zero value
/// while the kernel runs would make the next S-mode trap try to swap to a
/// bogus stack. Callers must restore `0` before the kernel resumes.
#[inline]
pub unsafe fn sscratch_write(value: usize) {
    unsafe { asm!("csrw sscratch, {}", in(reg) value, options(nostack, nomem)) };
}

/// Set `sstatus.SUM` (bit 18), permitting S-mode to read/write U-mode
/// pages. Opened only around a validated copy in the `print` syscall.
///
/// # Safety
/// While SUM is set the kernel can dereference user pages, so the caller
/// must have validated the pointer first and must clear SUM immediately
/// after the copy (see [`sstatus_clear_sum`]).
#[inline]
pub unsafe fn sstatus_set_sum() {
    unsafe { asm!("csrs sstatus, {}", in(reg) 1usize << 18, options(nostack, nomem)) };
}

/// Clear `sstatus.SUM` (bit 18): S-mode accesses to U-mode pages fault
/// again. The default state — only the `print` copy window deviates.
///
/// # Safety
/// Always memory-safe; pairs with [`sstatus_set_sum`].
#[inline]
pub unsafe fn sstatus_clear_sum() {
    unsafe { asm!("csrc sstatus, {}", in(reg) 1usize << 18, options(nostack, nomem)) };
}
