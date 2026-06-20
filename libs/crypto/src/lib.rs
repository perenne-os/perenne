#![cfg_attr(not(test), no_std)]
//! Cryptographic primitives for the Kernel project.
//!
//! Per ADR 0004, a post-quantum baseline using audited libraries (not
//! hand-rolled crypto). Phase 3c integrates ML-KEM-768 (RustCrypto
//! `ml-kem`, `no_std`/no-alloc) and exposes one round-trip primitive.

use ml_kem::kem::{Decapsulate, Encapsulate, Kem};
use ml_kem::MlKem768;
use rand_chacha::ChaCha20Rng;
use rand_core::SeedableRng;

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
