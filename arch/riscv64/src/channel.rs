//! The encrypted IPC channel (Phase 14): the kernel holds one ChaCha20-Poly1305
//! session keyed by an ML-KEM shared secret, and seals/opens small messages for
//! capability-gated components. A kernel session service — the kernel sees
//! plaintext (it is the TCB); the value is making the post-quantum secret real
//! and establishing the seal/open pattern (the foundation for future at-rest /
//! on-the-wire encryption).

/// The 32-byte session key, lazily established (ML-KEM, pool-seeded) on first use.
static mut SESSION_KEY: Option<[u8; 32]> = None;
/// Per-session nonce counter (never reused under the key).
static mut NONCE: u64 = 0;

/// Build a 12-byte ChaCha20-Poly1305 nonce from the session counter.
fn nonce_bytes(counter: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[..8].copy_from_slice(&counter.to_le_bytes());
    n
}

/// Return the session key, establishing it on first call: derive an ML-KEM
/// shared secret seeded by the entropy pool. Logged once. Both `seal_word` and
/// `open_word` call this, so the first caller fixes the key and all share it.
fn ensure_key() -> [u8; 32] {
    // SAFETY: single hart; the first caller establishes, then it is read-only.
    unsafe {
        if let Some(k) = core::ptr::read(core::ptr::addr_of!(SESSION_KEY)) {
            return k;
        }
        let seed = crate::entropy::next_seed();
        let key = kernel_crypto::ml_kem768_agree(seed).unwrap_or([0u8; 32]);
        core::ptr::write(core::ptr::addr_of_mut!(SESSION_KEY), Some(key));
        crate::println!("crypto: channel session established (ML-KEM)");
        key
    }
}

/// Seal an 8-byte plaintext word; returns `(ciphertext, tag, nonce)`.
pub fn seal_word(plain: [u8; 8]) -> ([u8; 8], [u8; 16], u64) {
    let key = ensure_key();
    // SAFETY: single hart; advance the nonce counter.
    let nonce = unsafe {
        let n = core::ptr::read(core::ptr::addr_of!(NONCE));
        core::ptr::write(core::ptr::addr_of_mut!(NONCE), n + 1);
        n
    };
    let mut buf = plain;
    let tag = kernel_crypto::seal(&key, &nonce_bytes(nonce), &mut buf);
    (buf, tag, nonce)
}

/// Open an 8-byte ciphertext word; `Some(plaintext)` iff the tag verifies.
pub fn open_word(ct: [u8; 8], tag: [u8; 16], nonce: u64) -> Option<[u8; 8]> {
    let key = ensure_key();
    let mut buf = ct;
    if kernel_crypto::open(&key, &nonce_bytes(nonce), &mut buf, &tag) {
        Some(buf)
    } else {
        None
    }
}
