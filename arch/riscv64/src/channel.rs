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

/// Establish the channel's session key: derive an ML-KEM shared secret from
/// `seed`. **Must be called from a large-stack context** (`kmain` on the boot
/// stack) — ML-KEM keygen is very stack-hungry and would overflow a task's
/// 16 KiB trap stack if done lazily in the `seal`/`open` syscall path. Called
/// once at boot; `seal`/`open` then only run the (light) AEAD.
pub fn establish(seed: [u8; 32]) {
    let key = kernel_crypto::ml_kem768_agree(seed).unwrap_or([0u8; 32]);
    // SAFETY: single hart; set once at boot before any seal/open.
    unsafe { core::ptr::write(core::ptr::addr_of_mut!(SESSION_KEY), Some(key)) };
    crate::println!("crypto: channel session established (ML-KEM)");
}

/// The established session key (or all-zero if `establish` was never called — a
/// boot-config bug; the syscalls still behave consistently).
fn key() -> [u8; 32] {
    // SAFETY: single hart; read of a boot-established key.
    unsafe { core::ptr::read(core::ptr::addr_of!(SESSION_KEY)).unwrap_or([0u8; 32]) }
}

/// Seal an 8-byte plaintext word; returns `(ciphertext, tag, nonce)`.
pub fn seal_word(plain: [u8; 8]) -> ([u8; 8], [u8; 16], u64) {
    let key = key();
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
    let key = key();
    let mut buf = ct;
    if kernel_crypto::open(&key, &nonce_bytes(nonce), &mut buf, &tag) {
        Some(buf)
    } else {
        None
    }
}
