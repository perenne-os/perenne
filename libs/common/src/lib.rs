#![cfg_attr(not(test), no_std)]
//! Shared types and utilities used across the Perenne project.
//!
//! `no_std`: this crate is used by the freestanding kernel, so it cannot
//! assume an operating system underneath (std is re-enabled for host
//! unit tests only). Real shared types (capabilities, error types, IDs)
//! arrive in later phases.

/// The project name. Centralized here so the name lives in one place
/// (the rename from the "Kernel (working title)" placeholder to **Perenne**
/// was a single edit — see ADR 0006 → ADR 0008).
pub const PROJECT_NAME: &str = "Perenne";

pub mod fs;
pub mod kb;
pub mod net;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_name_is_set() {
        assert!(!PROJECT_NAME.is_empty());
    }
}
