//! The kernel entropy pool — a ChaCha20 CSPRNG (`kernel_crypto::EntropyPool`)
//! seeded by the virtio-rng component, serving entropy to kernel code on
//! demand. Held in a `SingleHartCell` alongside the scheduler/frame allocator.
//!
//! It lives in `arch` (not the kernel binary) so the syscall layer
//! (`sched::getrandom`) can reach it — the CSPRNG cannot run in U-mode, so the
//! pool stays kernel-side and U-mode draws from it via a syscall.

use kernel_crypto::EntropyPool;

static POOL: crate::mem::SingleHartCell<EntropyPool> =
    crate::mem::SingleHartCell::new(EntropyPool::new());

/// Fold 32 bytes of device entropy into the pool (seeds on first call, mixes
/// after).
pub fn reseed(bytes: [u8; 32]) {
    POOL.with(|p| p.reseed(bytes));
}

/// Draw a fresh 32-byte seed from the pool (the kernel seeds it first).
pub fn next_seed() -> [u8; 32] {
    POOL.with(|p| p.next_seed())
}
