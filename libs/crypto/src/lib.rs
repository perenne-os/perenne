#![cfg_attr(not(test), no_std)]
//! Cryptographic primitives for the Kernel project.
//!
//! Phase 0 placeholder. Per ADR 0004, this will provide a post-quantum
//! cryptography baseline (e.g. ML-KEM / ML-DSA) in Phase 3, preferring
//! audited libraries over hand-rolled crypto.

/// Marker for the planned post-quantum baseline. Not yet implemented.
pub const PQC_PLANNED: bool = true;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pqc_is_planned() {
        assert!(PQC_PLANNED);
    }
}
