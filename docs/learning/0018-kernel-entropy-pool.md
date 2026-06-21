# 0018 — A kernel entropy pool

**One-line:** device entropy now seeds a reseedable kernel CSPRNG that hands
out randomness on demand — the standard kernel-RNG design — instead of a
one-shot boot seed.

## What changed
- New `kernel_crypto::EntropyPool`: a ChaCha20 CSPRNG, `const`-constructible
  (an `Option<ChaCha20Rng>`, `None` until seeded) so it can back a `static`.
  `reseed` seeds (first call) then mixes (later calls); `next_seed`/`fill`
  ratchet the stream.
- The kernel holds the pool in a `SingleHartCell` (the scheduler's primitive),
  with `pool_reseed`/`pool_next_seed` accessors — crypto stays in the kernel
  binary and `kernel_crypto`, not the `arch` crate.
- The `pqc` consumer now seeds the pool from a virtio-rng draw, shows it serves
  a stream on demand, reseeds with a second draw, and keys ML-KEM from a pool
  draw.

## The ideas worth keeping
1. **A kernel RNG is a CSPRNG seeded by hardware.** A finite hardware read
   becomes unlimited output: the device gives a seed; the CSPRNG ratchets out
   as much as you ask for. You only go back to the device to *reseed*, not for
   every byte.
2. **Reseeding mixes, it doesn't replace.** Folding new entropy as
   `rekey(old_output XOR new_bytes)` means a later (possibly stuck or
   attacker-influenced) source cannot *erase* the entropy you already had — it
   can only add.
3. **`const` statics need a `None` until ready.** `ChaCha20Rng::from_seed`
   isn't `const`, so the pool starts unseeded (`None`) and a `static`
   `SingleHartCell::new(EntropyPool::new())` is still `const`-constructible.

## A dependency-version gotcha
`fill_bytes` lives on the `Rng` trait in `rand_core` 0.10 (where `RngCore` is
just a marker subtrait `RngCore: Rng`) — so the import is `use rand_core::Rng`,
not `RngCore`. APIs drift between rand versions; read the trait you actually
depend on.

## Proof
`entropy: pool seeded from virtio-rng` → `pool serves on demand (draws differ)`
→ `pool reseeded from virtio-rng` → `pqc: ML-KEM-768 round-trip ok
(pool-seeded)`: real device entropy, pooled and reseedable, now keys the
post-quantum round-trip on demand.
