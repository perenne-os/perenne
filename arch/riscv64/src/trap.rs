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

/// Decoded `scause`. Only the causes Phases 2a/2b handles get variants;
/// everything else is `Unknown` and treated as fatal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    /// `ebreak`/`c.ebreak` executed (exception code 3).
    Breakpoint,
    /// Supervisor timer interrupt (interrupt code 5).
    SupervisorTimer,
    /// Instruction fetch from an unmapped/non-executable page (code 12).
    InstructionPageFault,
    /// Load from an unmapped/unreadable page (code 13).
    LoadPageFault,
    /// Store to an unmapped/unwritable page (code 15) — what the W^X
    /// probe deliberately triggers.
    StorePageFault,
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
        (false, 12) => Cause::InstructionPageFault,
        (false, 13) => Cause::LoadPageFault,
        (false, 15) => Cause::StorePageFault,
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

    #[test]
    fn decodes_page_faults() {
        assert_eq!(decode(12), Cause::InstructionPageFault);
        assert_eq!(decode(13), Cause::LoadPageFault);
        assert_eq!(decode(15), Cause::StorePageFault);
    }

    #[test]
    fn page_fault_codes_as_interrupts_stay_unknown() {
        // Interrupt bit + code 13 is NOT a load page fault.
        assert_eq!(
            decode(INTERRUPT_BIT | 13),
            Cause::Unknown { interrupt: true, code: 13 }
        );
    }
}

// The assembly trap entry. Layout contract: see [`TrapFrame`].
// 288 = size_of::<TrapFrame>() (280) rounded up to keep `sp` 16-aligned
// per the RISC-V ABI. `t0` (= x5) is used as scratch only *after* its
// slot is saved, and on the way out only *before* its slot is restored.
// `x2` (sp) is restored last — that load releases the frame.
#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(
    r#"
    .section .text
    .align 2                # stvec requires 4-byte alignment (mode bits = 00, direct)
    .global __trap_entry
__trap_entry:
    addi sp, sp, -288
    sd x1,  0(sp)
    sd x3,  16(sp)
    sd x4,  24(sp)
    sd x5,  32(sp)
    sd x6,  40(sp)
    sd x7,  48(sp)
    sd x8,  56(sp)
    sd x9,  64(sp)
    sd x10, 72(sp)
    sd x11, 80(sp)
    sd x12, 88(sp)
    sd x13, 96(sp)
    sd x14, 104(sp)
    sd x15, 112(sp)
    sd x16, 120(sp)
    sd x17, 128(sp)
    sd x18, 136(sp)
    sd x19, 144(sp)
    sd x20, 152(sp)
    sd x21, 160(sp)
    sd x22, 168(sp)
    sd x23, 176(sp)
    sd x24, 184(sp)
    sd x25, 192(sp)
    sd x26, 200(sp)
    sd x27, 208(sp)
    sd x28, 216(sp)
    sd x29, 224(sp)
    sd x30, 232(sp)
    sd x31, 240(sp)
    addi t0, sp, 288        # reconstruct the pre-trap sp (x2)
    sd t0, 8(sp)
    csrr t0, sepc
    sd t0, 248(sp)
    csrr t0, sstatus
    sd t0, 256(sp)
    csrr t0, scause
    sd t0, 264(sp)
    csrr t0, stval
    sd t0, 272(sp)
    mv a0, sp               # &mut TrapFrame
    call trap_handler
    ld t0, 248(sp)          # handler may have advanced sepc
    csrw sepc, t0
    ld t0, 256(sp)
    csrw sstatus, t0
    ld x1,  0(sp)
    ld x3,  16(sp)
    ld x4,  24(sp)
    ld x5,  32(sp)
    ld x6,  40(sp)
    ld x7,  48(sp)
    ld x8,  56(sp)
    ld x9,  64(sp)
    ld x10, 72(sp)
    ld x11, 80(sp)
    ld x12, 88(sp)
    ld x13, 96(sp)
    ld x14, 104(sp)
    ld x15, 112(sp)
    ld x16, 120(sp)
    ld x17, 128(sp)
    ld x18, 136(sp)
    ld x19, 144(sp)
    ld x20, 152(sp)
    ld x21, 160(sp)
    ld x22, 168(sp)
    ld x23, 176(sp)
    ld x24, 184(sp)
    ld x25, 192(sp)
    ld x26, 200(sp)
    ld x27, 208(sp)
    ld x28, 216(sp)
    ld x29, 224(sp)
    ld x30, 232(sp)
    ld x31, 240(sp)
    ld x2,  8(sp)           # restore original sp LAST; frame is gone
    sret
"#
);

/// Install [`__trap_entry`] as the trap vector (direct mode). Call once,
/// early in kmain, before anything can fault and before interrupts are
/// enabled.
#[cfg(target_arch = "riscv64")]
pub fn init() {
    extern "C" {
        fn __trap_entry();
    }
    // SAFETY: __trap_entry is the real entry defined above; .align 2
    // gives the required 4-byte alignment.
    unsafe { crate::csr::stvec_write(__trap_entry as *const () as usize) };
}

/// Length of the instruction at `addr`, for advancing `sepc` past it.
#[cfg(target_arch = "riscv64")]
fn instruction_len_at(addr: usize) -> usize {
    // SAFETY: addr is the sepc of a just-executed instruction, so it
    // points at readable, physically-addressed kernel code (no paging yet).
    let parcel = unsafe { core::ptr::read_volatile(addr as *const u16) };
    instruction_len(parcel)
}

/// Unrecoverable trap: print everything we know, then panic. `stval`
/// holds the faulting address for page faults.
#[cfg(target_arch = "riscv64")]
fn fatal(kind: &str, frame: &TrapFrame) -> ! {
    crate::println!(
        "FATAL TRAP ({kind}): sepc={:#x} stval={:#x}",
        frame.sepc, frame.stval
    );
    crate::println!("{frame:#x?}");
    panic!("unhandled trap");
}

/// Rust side of every trap; called by the entry assembly with the saved
/// frame. Returning resumes at `frame.sepc` via `sret`.
///
/// The entry assembly guarantees `a0` points at a fully-initialized, uniquely-owned
/// `TrapFrame` on the current stack — that is what makes the `&mut` reference sound.
/// Phase 2c's context switch must preserve this invariant.
#[cfg(target_arch = "riscv64")]
#[no_mangle]
extern "C" fn trap_handler(frame: &mut TrapFrame) {
    match decode(frame.scause) {
        Cause::Breakpoint => {
            crate::println!("trap: breakpoint at {:#x}", frame.sepc);
            // ebreak doesn't advance the PC itself; without this, sret
            // would re-execute it forever.
            frame.sepc += instruction_len_at(frame.sepc);
        }
        Cause::SupervisorTimer => crate::timer::on_tick(),
        Cause::InstructionPageFault => fatal("instruction page fault", frame),
        Cause::LoadPageFault => fatal("load page fault", frame),
        Cause::StorePageFault => fatal("store page fault", frame),
        Cause::Unknown { interrupt, code } => {
            crate::println!("trap: unknown cause interrupt={interrupt} code={code}");
            fatal("unknown", frame);
        }
    }
}
