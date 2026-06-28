#![cfg_attr(not(test), no_std)]
//! Shared types and utilities used across the Kernel project.
//!
//! `no_std`: this crate is used by the freestanding kernel, so it cannot
//! assume an operating system underneath (std is re-enabled for host
//! unit tests only). Real shared types (capabilities, error types, IDs)
//! arrive in later phases.

/// Returns the provisional project name.
///
/// Centralizing this constant keeps the working title in one place so a
/// future rename (ADR 0006) is a single edit.
pub const PROJECT_NAME: &str = "Kernel (working title)";

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
