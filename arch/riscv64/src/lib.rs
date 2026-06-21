#![cfg_attr(not(test), no_std)]
//! RISC-V (riscv64) architecture-specific code — the first target.
//!
//! Phase 1: the SBI call wrappers and the kernel console used by the
//! freestanding kernel binary. Bare-metal modules are gated to
//! `target_arch = "riscv64"` so this crate still builds and tests on
//! the host. Other architectures (x86-64, ARM64) get sibling crates
//! later; the HAL keeps them interchangeable.

#[cfg(target_arch = "riscv64")]
pub mod console;
#[cfg(target_arch = "riscv64")]
pub mod csr;
#[cfg(target_arch = "riscv64")]
pub mod sbi;
#[cfg(target_arch = "riscv64")]
pub mod timer;
#[cfg(target_arch = "riscv64")]
pub mod uart;

/// Trap handling: pure decoding logic (no asm, host-testable); the gated parts (entry, dispatcher, init) live inside.
pub mod trap;

/// Memory management: bitmap frame allocator and Sv39 paging. Pure logic (bitmap and PTE math, host-testable); the gated parts (statics, page-table walker, satp) live inside.
pub mod mem;

/// Tasks and their saved register context. Pure types (host-testable);
/// the context-switch assembly and the scheduler statics live in `sched`.
pub mod task;

/// Scheduling: the round-robin run queue (pure, host-testable) plus the
/// gated context-switch assembly and the static scheduler instance.
pub mod sched;

/// System calls: the U-mode → kernel entry surface. Pure decoding and
/// the confused-deputy pointer guard are host-tested here; the gated
/// dispatcher (which reads user memory and writes the console) lives
/// inside.
pub mod syscall;

/// Capabilities: unforgeable per-task authority tokens (pure types and the
/// lookup, host-tested). The tables live on tasks; the IPC rendezvous that
/// consumes them lives in `sched`.
pub mod cap;

/// Device tree (FDT) parsing: discover RAM and the timer frequency from the
/// firmware-provided blob (pure parsing host-tested; `from_ptr` gated).
pub mod dt;

/// The self-healing knowledge organism: the deterministic, host-tested rule
/// engine that diagnoses a contained crash against compiled-in knowledge
/// (Phase 5a — diagnosis only; the caged healer that acts is 5b).
pub mod heal;

/// virtio-mmio constants + the RNG probe (the kernel side of the user-space
/// entropy driver). Pure constants/helpers host-tested; the gated probe reads
/// device registers.
pub mod virtio;

/// The architecture identifier this crate targets.
pub const ARCH: &str = "riscv64";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arch_is_riscv64() {
        assert_eq!(ARCH, "riscv64");
    }
}
