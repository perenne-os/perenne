//! Trap handling: the kernel's reflexes.
//!
//! A *trap* is the hart's reaction to an exceptional event — either an
//! **exception** (synchronous, caused by the current instruction, e.g.
//! `ebreak`) or an **interrupt** (asynchronous, e.g. the timer). The CPU
//! jumps to the address in `stvec`, and `scause` says why.
//!
//! Pure decoding logic lives ungated in this module so it tests on the
//! host. The assembly entry, the dispatcher, and `init()` are gated to
//! `target_arch = "riscv64"` (added in a later task).

/// Snapshot of the interrupted hart, pushed by the trap entry assembly
/// and restored on the way out. Full (all 31 GPRs) rather than
/// caller-saved-only: this is exactly the structure context switching
/// (Phase 2c) needs, and saving it now avoids a rewrite.
///
/// Layout contract with the entry assembly: `regs[n-1]` holds `x_n`
/// at byte offset `(n-1) * 8`; then sepc, sstatus, scause, stval.
/// `x0` is hardwired to zero and not stored.
#[derive(Debug)]
#[repr(C)]
pub struct TrapFrame {
    /// General-purpose registers x1..=x31; `regs[n-1]` = `x_n`.
    pub regs: [usize; 31],
    /// PC of the trapping/interrupted instruction; `sret` resumes here.
    pub sepc: usize,
    /// Privilege/interrupt state at trap time; restored by `sret`.
    pub sstatus: usize,
    /// Why the trap happened (interrupt bit + cause code).
    pub scause: usize,
    /// Trap-specific extra value (e.g. the faulting address).
    pub stval: usize,
}

/// Decoded `scause`. Only the causes Phase 2a handles get variants;
/// everything else is `Unknown` and treated as fatal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    /// `ebreak`/`c.ebreak` executed (exception code 3).
    Breakpoint,
    /// Supervisor timer interrupt (interrupt code 5).
    SupervisorTimer,
    /// Anything we don't handle yet.
    Unknown { interrupt: bool, code: usize },
}

/// In `scause`, the top bit distinguishes interrupts from exceptions.
const INTERRUPT_BIT: usize = 1 << (usize::BITS - 1);

/// Decode a raw `scause` value.
/// Reads the interrupt bit (MSB) and strips it from the cause code before matching.
pub fn decode(scause: usize) -> Cause {
    let interrupt = scause & INTERRUPT_BIT != 0;
    let code = scause & !INTERRUPT_BIT;
    match (interrupt, code) {
        (false, 3) => Cause::Breakpoint,
        (true, 5) => Cause::SupervisorTimer,
        _ => Cause::Unknown { interrupt, code },
    }
}

/// Length in bytes of the instruction starting with this 16-bit parcel.
/// RISC-V encoding rule: standard 4-byte instructions have the two low
/// bits `11`; compressed (C-extension) 2-byte instructions do not.
/// Sufficient for RV64GC: instructions are 2 or 4 bytes; longer encodings
/// (reserved by the spec when more low bits are set) do not occur in the
/// extensions we build for.
pub fn instruction_len(parcel: u16) -> usize {
    if parcel & 0b11 == 0b11 { 4 } else { 2 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_breakpoint_exception() {
        assert_eq!(decode(3), Cause::Breakpoint);
    }

    #[test]
    fn decodes_supervisor_timer_interrupt() {
        assert_eq!(decode(INTERRUPT_BIT | 5), Cause::SupervisorTimer);
    }

    #[test]
    fn unknown_exception_is_not_fatal_to_decode() {
        // Exception code 2 = illegal instruction; unhandled in 2a.
        assert_eq!(decode(2), Cause::Unknown { interrupt: false, code: 2 });
    }

    #[test]
    fn unknown_interrupt_keeps_interrupt_flag() {
        // Interrupt code 9 = supervisor external; unhandled in 2a.
        assert_eq!(
            decode(INTERRUPT_BIT | 9),
            Cause::Unknown { interrupt: true, code: 9 }
        );
    }

    #[test]
    fn ebreak_is_four_bytes() {
        // ebreak = 0x00100073; its low parcel 0x0073 ends in 0b11.
        assert_eq!(instruction_len(0x0073), 4);
    }

    #[test]
    fn compressed_ebreak_is_two_bytes() {
        // c.ebreak = 0x9002; anything NOT ending in 0b11 is compressed/2-byte.
        assert_eq!(instruction_len(0x9002), 2);
    }

    #[test]
    fn low_bits_00_is_also_two_bytes() {
        // Any parcel not ending in 0b11 is a compressed instruction.
        assert_eq!(instruction_len(0x0000), 2);
    }

    #[test]
    fn trap_frame_layout_matches_entry_asm() {
        // The entry assembly allocates 288 bytes (280 rounded up to 16)
        // and stores stval at offset 272. If this changes, trap.rs's
        // assembly (added later) must change with it.
        assert_eq!(core::mem::size_of::<TrapFrame>(), 280);
    }
}
