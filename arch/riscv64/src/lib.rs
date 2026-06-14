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

/// Trap handling: pure decoding logic (no asm, host-testable); the gated parts (entry, dispatcher, init) live inside.
pub mod trap;

/// Memory management: bitmap frame allocator and Sv39 paging. Pure logic (bitmap and PTE math, host-testable); the gated parts (statics, page-table walker, satp) live inside.
pub mod mem;

/// Tasks and their saved register context. Pure types (host-testable);
/// the context-switch assembly and the scheduler statics live in `sched`.
pub mod task;

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
