//! Shared types and utilities used across the Kernel project.
//!
//! Phase 0 placeholder. Real shared types (capabilities, error types,
//! IDs) arrive in later phases.

/// Returns the provisional project name.
///
/// Centralizing this constant keeps the working title in one place so a
/// future rename (ADR 0006) is a single edit.
pub const PROJECT_NAME: &str = "Kernel (working title)";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_name_is_set() {
        assert!(!PROJECT_NAME.is_empty());
    }
}
