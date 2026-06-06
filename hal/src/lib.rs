//! Hardware Abstraction Layer — the device-agnostic boundary.
//!
//! Phase 0 placeholder. This is where every device (today's hardware and
//! future accelerators like GPUs/NPUs/QPUs) registers behind a uniform
//! interface, keeping the kernel hardware-agnostic. See
//! docs/architecture/hardware-abstraction.md.

/// True once at least one backend is wired up. None in Phase 0.
pub const HAS_BACKEND: bool = false;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_backend_in_phase_0() {
        assert!(!HAS_BACKEND);
    }
}
