# 0032 — Encrypted IPC channel: putting the post-quantum secret to work

**One-line:** the ML-KEM shared secret — built in Phase 3c but only ever computed
in a round-trip demo — finally *does* something: it keys a ChaCha20-Poly1305
session, and two components exchange an authenticated-encrypted message over IPC.

## What changed
- An audited **`chacha20poly1305`** crate (RustCrypto, `no_std`/no-alloc via the
  in-place detached AEAD) — pure `seal`/`open` in `kernel-crypto`, host-tested.
- A kernel **channel** module: one session keyed by `ml_kem768_agree(...)`, a
  nonce counter, and `seal_word`/`open_word`.
- `seal`/`open` **syscalls** gated by a new **`Session` capability** (mirroring
  how `Randomness` gates `getrandom`).
- A demo: a `sealer` seals a known word and sends `{nonce, ciphertext, tag}` to
  an `opener`, which decrypts, verifies, and confirms a *tampered* ciphertext is
  rejected; a `nocap` component without the `Session` cap is refused.

## The idea worth keeping: AEAD gives confidentiality *and* integrity
The whole crypto stack is now *applied* end to end: hardware entropy → ML-KEM
session key → an AEAD-encrypted message. ChaCha20-Poly1305 is **authenticated**
encryption: the 16-byte Poly1305 tag means `open` doesn't just decrypt — it
*rejects* a ciphertext that was altered (or encrypted under a different key). So
the opener's proof is two-sided: the real message verifies, and a single flipped
bit fails the tag.

## Honest threat model
This is a **kernel session service** — the crypto runs in the kernel (U-mode
can't run it, the codegen rule below), so the kernel, already the TCB, sees
plaintext. The value is making the post-quantum secret *real* and establishing
the `seal`/`open` + session pattern (the foundation for at-rest and on-the-wire
encryption), **not** end-to-end secrecy *from* the kernel — that needs U-mode
crypto, deferred.

## Two things that bit (and the rules behind them)
- **ML-KEM keygen overflows a 16 KiB stack.** First attempt established the
  session key lazily inside the `seal` syscall, which runs on the caller's 16 KiB
  trap stack. ML-KEM keygen is enormously stack-hungry (the boot stack was bumped
  to 512 KiB for it back in 3c) — it overflowed, corrupted the saved return
  address, and the kernel jumped to a garbage (ciphertext-looking) address. Fix:
  **establish the key once at boot in `kmain`, on the big boot stack**; the
  syscalls then only run the light AEAD. A consequence: the key is fixed-seeded at
  boot (pool-seeding it is deferred — it would need a large-stack establishment
  after the pool is ready).
- **A 64-bit constant in U-mode faults.** The demo plaintext as a full 64-bit
  constant was materialized via a `ld` from the kernel's `.rodata` constant pool
  — unmapped in U-mode → load page fault. Keeping it ≤ 32 bits makes the compiler
  emit `lui`+`addi` (no load). The same recurring rule as note 0013: **U-mode code
  must not touch kernel `.text`/`.rodata`.**

## Proof
`crypto: channel session established (ML-KEM)` → the sealer seals and sends →
`sched: task 'opener' exited (code 14)` (decrypted, verified, *and* a tampered
ciphertext rejected) → `crypto: 'nocap' seal refused (no Session capability)` /
`sched: task 'nocap' exited (code 15)`.

## What's next
U-mode crypto for true end-to-end secrecy (remove the kernel from the
confidentiality TCB); pool-seeding the channel key (large-stack establishment);
a real two-party ML-KEM handshake; multi-message nonce/rekey; and applying the
channel to disk (at-rest) or a future network stack (in-transit).
