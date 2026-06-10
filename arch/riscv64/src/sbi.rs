//! Minimal Supervisor Binary Interface (SBI) wrappers.
//!
//! Our kernel runs in S-mode (supervisor); OpenSBI runs below it in
//! M-mode (machine). An `ecall` instruction from S-mode traps into
//! OpenSBI, which performs the requested service and returns. The
//! extension id goes in register `a7`, arguments in `a0`/`a1`/...
//!
//! Phase 1 needed exactly one call: legacy `console_putchar` (EID 0x01),
//! deprecated but universally supported (the DBCN replacement can land
//! in a later phase). Phase 2a adds the modern TIME extension —
//! see [`set_timer`] for the v2 calling convention.

use core::arch::asm;

/// Print one byte to the firmware console (legacy SBI EID 0x01).
pub fn console_putchar(c: u8) {
    unsafe {
        asm!(
            "ecall",
            in("a7") 0x01usize,
            inout("a0") c as usize => _, // a0 carries the argument in and the SBI return value out
            out("a1") _,
        );
    }
}

/// Program the next timer interrupt (SBI TIME extension, EID
/// 0x54494D45 = "TIME", FID 0).
///
/// Unlike Phase 1's legacy console call, modern SBI extensions take the
/// extension id in `a7` *and* a function id in `a6` — this is the SBI
/// v2 calling convention. The timer is **one-shot**: firing at absolute
/// time `stime_value` (in `time`-CSR ticks) also clears the pending
/// bit, and the handler must call this again for the next tick.
pub fn set_timer(stime_value: u64) {
    unsafe {
        asm!(
            "ecall",
            in("a7") 0x5449_4D45usize,
            in("a6") 0usize,
            inout("a0") stime_value as usize => _, // in: deadline; out: SBI error code (ignored)
            out("a1") _,
        );
    }
}
