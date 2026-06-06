//! RISC-V (riscv64) architecture-specific code — the first target.
//!
//! Phase 0 placeholder. Boot, trap handling, and CPU-specific logic
//! arrive in Phase 1+. Other architectures (x86-64, ARM64) get sibling
//! crates later; the HAL keeps them interchangeable.

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
