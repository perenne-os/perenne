//! Minimal Supervisor Binary Interface (SBI) wrappers.
//!
//! Our kernel runs in S-mode (supervisor); OpenSBI runs below it in
//! M-mode (machine). An `ecall` instruction from S-mode traps into
//! OpenSBI, which performs the requested service and returns. The
//! extension id goes in register `a7`, arguments in `a0`/`a1`/...
//!
//! Phase 1 needs exactly one call: legacy `console_putchar` (EID 0x01).
//! It is deprecated in the SBI spec but universally supported by
//! OpenSBI; the modern replacement (the DBCN extension) can land in a
//! later phase.

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
