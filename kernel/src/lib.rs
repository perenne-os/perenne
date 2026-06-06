//! The microkernel (working title).
//!
//! Phase 0 placeholder that compiles on the host. In Phase 1 this becomes
//! a `no_std` freestanding binary that boots under QEMU on riscv64 and
//! prints "hello world". For now it only exposes the project name.

use kernel_common::PROJECT_NAME;

/// Returns a startup banner string. Real boot code arrives in Phase 1.
pub fn banner() -> String {
    format!("{PROJECT_NAME} — Phase 0 foundation")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_mentions_project() {
        assert!(banner().contains("working title"));
    }
}
