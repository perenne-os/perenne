#![cfg_attr(not(test), no_std)]
//! Cryptographic primitives for the Kernel project.
//!
//! Per ADR 0004, a post-quantum baseline using audited libraries (not
//! hand-rolled crypto). Phase 3c integrates ML-KEM-768 (RustCrypto
//! `ml-kem`, `no_std`/no-alloc) and exposes one round-trip primitive.

use ml_kem::kem::{Decapsulate, Encapsulate, Kem};
use ml_kem::MlKem768;
use rand_chacha::ChaCha20Rng;
use rand_core::{Rng, SeedableRng};

/// Run an ML-KEM-768 round-trip seeded by `seed`: generate a keypair,
/// encapsulate a shared secret to the public key, then decapsulate the
/// ciphertext with the private key. Returns `Some(secret)` iff the
/// encapsulated and decapsulated 32-byte shared secrets are identical.
///
/// `seed` keys the CSPRNG used for keygen/encapsulation. It is a parameter
/// so a later phase can pass real entropy; the demo uses a fixed
/// (non-secret) seed — this proves the algorithm runs, not that it is
/// securely keyed.
pub fn ml_kem768_agree(seed: [u8; 32]) -> Option<[u8; 32]> {
    let mut rng = ChaCha20Rng::from_seed(seed);
    let (dk, ek) = MlKem768::generate_keypair_from_rng(&mut rng);
    let (ct, k_send) = ek.encapsulate_with_rng(&mut rng);
    let k_recv = dk.decapsulate(&ct);
    if k_send == k_recv {
        let mut out = [0u8; 32];
        out.copy_from_slice(k_send.as_slice());
        Some(out)
    } else {
        None
    }
}

/// A reseedable kernel entropy pool: a ChaCha20 CSPRNG seeded by a hardware
/// entropy source (the virtio-rng component) and serving entropy on demand.
///
/// `const`-constructible (so it can back a `static` cell) because it holds an
/// `Option<ChaCha20Rng>` that is `None` until first seeded. This is a minimal
/// pool built on an audited stream cipher — not a NIST-certified DRBG.
pub struct EntropyPool {
    rng: Option<ChaCha20Rng>,
}

impl EntropyPool {
    /// An unseeded pool. `reseed` seeds it; drawing before that yields zeros.
    pub const fn new() -> Self {
        Self { rng: None }
    }

    /// Fold 32 bytes of entropy into the pool. The first call seeds it; later
    /// calls MIX — draw 32 bytes of current output, XOR with `bytes`, and
    /// re-key with the result — so new entropy is added to existing state (a
    /// stuck source cannot erase prior entropy).
    pub fn reseed(&mut self, bytes: [u8; 32]) {
        let new_seed = match self.rng.as_mut() {
            None => bytes,
            Some(rng) => {
                let mut cur = [0u8; 32];
                rng.fill_bytes(&mut cur);
                let mut mixed = [0u8; 32];
                for i in 0..32 {
                    mixed[i] = cur[i] ^ bytes[i];
                }
                mixed
            }
        };
        self.rng = Some(ChaCha20Rng::from_seed(new_seed));
    }

    /// Fill `out` with CSPRNG output (ratchets the stream). If the pool has
    /// never been seeded, fills zeros (a kernel-config bug — seed first).
    pub fn fill(&mut self, out: &mut [u8]) {
        match self.rng.as_mut() {
            Some(rng) => rng.fill_bytes(out),
            None => out.iter_mut().for_each(|b| *b = 0),
        }
    }

    /// A fresh 32-byte draw (e.g. an ML-KEM seed).
    pub fn next_seed(&mut self) -> [u8; 32] {
        let mut s = [0u8; 32];
        self.fill(&mut s);
        s
    }
}

impl Default for EntropyPool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_agrees() {
        let secret = ml_kem768_agree([7u8; 32]).expect("round-trip should agree");
        assert_ne!(secret, [0u8; 32], "shared secret should not be all-zero");
    }

    #[test]
    fn different_seeds_give_different_secrets() {
        let a = ml_kem768_agree([1u8; 32]).unwrap();
        let b = ml_kem768_agree([2u8; 32]).unwrap();
        assert_ne!(a, b, "distinct seeds must produce distinct shared secrets");
    }

    #[test]
    fn pool_yields_distinct_successive_draws() {
        let mut p = EntropyPool::new();
        p.reseed([1u8; 32]);
        assert_ne!(p.next_seed(), p.next_seed(), "a CSPRNG stream must not repeat");
    }

    #[test]
    fn pool_same_seed_same_stream() {
        let mut a = EntropyPool::new();
        a.reseed([7u8; 32]);
        let mut b = EntropyPool::new();
        b.reseed([7u8; 32]);
        assert_eq!(a.next_seed(), b.next_seed(), "same seed reproduces the stream");
    }

    #[test]
    fn pool_reseed_changes_the_stream() {
        // Without reseed: seed, consume one draw, observe the next.
        let mut base = EntropyPool::new();
        base.reseed([1u8; 32]);
        let _ = base.next_seed();
        let no_reseed = base.next_seed();
        // With reseed at the same point: the next draw is from a re-keyed rng.
        let mut p = EntropyPool::new();
        p.reseed([1u8; 32]);
        let _ = p.next_seed();
        p.reseed([2u8; 32]);
        assert_ne!(p.next_seed(), no_reseed, "reseed must alter the stream");
    }

    #[test]
    fn pool_reseed_mixes_not_replaces() {
        // Seeded [1] then reseeded [2] must differ from a fresh pool seeded [2]
        // alone — proving reseed folds in prior state rather than replacing it.
        let mut mixed = EntropyPool::new();
        mixed.reseed([1u8; 32]);
        mixed.reseed([2u8; 32]);
        let mut plain = EntropyPool::new();
        plain.reseed([2u8; 32]);
        assert_ne!(mixed.next_seed(), plain.next_seed(), "reseed must mix, not replace");
    }

    #[test]
    fn unseeded_pool_yields_zeros() {
        let mut p = EntropyPool::new();
        assert_eq!(p.next_seed(), [0u8; 32], "an unseeded pool yields zeros");
    }

    #[test]
    fn tampered_ciphertext_does_not_agree() {
        // Drive the KEM directly so we can corrupt the ciphertext between
        // encapsulation and decapsulation. ML-KEM uses implicit rejection:
        // a bad ciphertext yields a DIFFERENT (pseudo-random) secret, never
        // the original — so the agreement check in ml_kem768_agree is real.
        let mut rng = ChaCha20Rng::from_seed([9u8; 32]);
        let (dk, ek) = MlKem768::generate_keypair_from_rng(&mut rng);
        let (mut ct, k_send) = ek.encapsulate_with_rng(&mut rng);
        ct[0] ^= 0xff; // corrupt one byte
        let k_recv = dk.decapsulate(&ct);
        assert_ne!(k_send, k_recv, "tampered ciphertext must not agree");
    }
}
