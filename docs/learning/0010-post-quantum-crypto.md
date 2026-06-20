# 0010 — A post-quantum primitive: ML-KEM (Phase 3c)

**One-line:** the kernel can run a post-quantum key-encapsulation round-trip
(ML-KEM-768) — the project's first real crypto and first external dependency.

## What changed
- `libs/crypto` is no longer a placeholder: it pulls RustCrypto `ml-kem`
  (`no_std`, no-alloc) and exposes `ml_kem768_agree(seed) -> Option<[u8;32]>`
  = keygen → encapsulate → decapsulate, returning the shared secret iff both
  sides agree.
- The kernel calls it from `kmain`; the smoke test sees the round-trip
  succeed on the real riscv64 target — proving an off-the-shelf crate works
  in the bare `no_std`/no-alloc kernel.
- First external dependency, so `Cargo.lock` is now committed (reproducible,
  auditable builds).

## The key ideas
- A **KEM** establishes a shared secret: `encapsulate` to a public key
  produces (ciphertext, secret); the holder of the private key
  `decapsulate`s the ciphertext to the same secret. ML-KEM (FIPS 203) is the
  post-quantum standard (ADR 0004).
- **Honest limitation:** keygen/encapsulation need randomness, but there is
  no entropy source on the bare target yet. We seed a real CSPRNG
  (`ChaCha20Rng`) from a *fixed, non-secret* seed — so the demo proves the
  algorithm runs, NOT that it is securely keyed. Real entropy is deferred.

## A gotcha that bit us
The debug build (no inlining) of the ML-KEM round-trip used **>64 KiB of
stack** and overflowed the boot stack into its guard page (a clean store
fault, thanks to that guard page). Fix: bump the boot stack to 512 KiB. A
reminder that audited algorithms can still be heavy on an un-optimized
build.

## Proof
Host tests (round-trip agrees; different seeds differ; a tampered ciphertext
does not agree). Smoke test: `pqc: ML-KEM-768 round-trip ok` on the bare
kernel. This completes Phase 3 (the security spine).
