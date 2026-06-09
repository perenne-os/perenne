#![cfg_attr(not(test), no_std)]
//! The microkernel (working title).
//!
//! Phase 1: the package's binary (`src/main.rs`) is a freestanding
//! `no_std` kernel that boots under QEMU on riscv64 and prints a
//! greeting. This library holds the host-testable parts.

/// The greeting the kernel prints at boot — the Phase 1 milestone
/// ("Done when: ./tools/run-qemu.ps1 loads our kernel and it prints
/// 'hello world'", docs/roadmap/roadmap.md). The boot smoke test
/// (tools/test-qemu.ps1) greps the serial log for exactly this string.
pub const GREETING: &str = "hello world";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greeting_is_hello_world() {
        assert_eq!(GREETING, "hello world");
    }
}
