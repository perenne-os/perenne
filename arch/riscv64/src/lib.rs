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
pub mod sbi;

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
