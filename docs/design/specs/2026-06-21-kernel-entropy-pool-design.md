# Kernel — Design: a kernel entropy pool seeded by virtio-rng

- **Date:** 2026-06-21
- **Status:** Draft — awaiting review
- **Scope of this document:** a reseedable kernel-side entropy pool (a
  ChaCha20-based CSPRNG) seeded by the virtio-rng component, serving entropy on
  demand to kernel code (ML-KEM), replacing the current one-shot boot seed.
  Fully QEMU-testable.

---

## 0. Where this sits

The virtio-rng component delivers real device entropy over IPC, and the kernel
`pqc` consumer currently uses it **once** to seed an ML-KEM round-trip — a
one-shot. There is no persistent kernel randomness: nothing can ask for fresh
entropy after boot. A kernel entropy pool fixes that — the standard kernel-RNG
design (a CSPRNG seeded by the hardware source, reseedable, serving on demand),
and the roadmap's named next step after call/reply.

It builds on: the virtio-rng entropy component (the device source), call/reply
IPC and the kernel `pqc` consumer task (the seeding path), and `kernel_crypto`
(which already depends on `rand_chacha`).

## 1. Goal

Collect device entropy from the virtio-rng component into a kernel-side
**pool** — a ChaCha20 CSPRNG — and serve entropy from it **on demand** to
kernel consumers, **reseeding** it with fresh device entropy over time. ML-KEM
is now seeded from the pool, not directly from a single device read.

**You learn (kept brief):** what a kernel RNG actually is — a CSPRNG seeded by
a hardware entropy source that yields unlimited output on demand from a finite
seed, and reseeds by *mixing* new entropy into existing state (so a stuck
source cannot erase what you had); and why this is built on an audited stream
cipher (ChaCha20) rather than hand-rolled.

**Done when** `./tools/test-qemu.ps1` observes, alongside every existing
milestone:

1. **Seeded, on-demand, reseeded, pool-keyed crypto** — the kernel logs, in
   order: `entropy: pool seeded from virtio-rng`; `entropy: pool serves on
   demand (draws differ)` (two successive pool draws differ — one device seed
   yields a stream); `entropy: pool reseeded from virtio-rng`; and `pqc:
   ML-KEM-768 round-trip ok (pool-seeded)` (ML-KEM keyed by a pool draw).

And off the bare target:

2. **Host unit tests** for the pure pool: distinct successive draws, a seed
   reproduces its stream, reseeding changes the stream, and reseed *mixes*
   (the post-reseed stream differs from both the pre-reseed stream and a fresh
   stream keyed by the reseed bytes alone).

## 2. Non-goals (deferred)

- **A U-mode `getrandom` service** — no component needs randomness yet. The
  pool serves kernel code only; a capability-gated U-mode entropy endpoint is
  a clean follow-on.
- **Continuous / timer-driven reseeding** — the demo seeds then reseeds once
  (two device draws at boot), enough to exercise seed + reseed. Periodic
  background reseeding (the component drawing on a schedule) is future work.
- **A NIST-certified DRBG** (SP 800-90A) or entropy estimation/health tests —
  this is a minimal pool on an audited primitive, not a certified construction.
- **Replacing ML-KEM's internal RNG** — ML-KEM still drives its own
  `ChaCha20Rng` from the 32-byte seed we hand it; the pool supplies that seed.
- **Changing the entropy component or the virtio driver** — both are untouched;
  only the kernel consumer's use of the delivered bytes changes.

## 3. Design

### 3.1 Components

| Component | Location | Responsibility |
|-----------|----------|----------------|
| `EntropyPool` | `libs/crypto/src/lib.rs` (`kernel_crypto`) | Pure, host-tested CSPRNG: `const fn new()` (unseeded), `reseed`, `fill`, `next_seed`. |
| Pool state + accessors | `kernel/src/main.rs` (the `bare` module) | A `SingleHartCell<EntropyPool>` static + `reseed`/`next_seed` wrappers. (Crypto stays out of the `arch` crate; the kernel binary already deps `kernel_crypto` and `kernel_arch_riscv64::mem::SingleHartCell`.) |
| Seeding/consuming | `kernel/src/main.rs` `pqc_consumer` | Seed + reseed the pool from the entropy component's IPC draws; draw ML-KEM's seed from the pool. |

### 3.2 The pool (`kernel_crypto::EntropyPool`)

A ChaCha20 CSPRNG, `const`-constructible (so it can back a `static` cell)
because it holds an `Option<ChaCha20Rng>` that is `None` until first seeded:

```
pub struct EntropyPool { rng: Option<ChaCha20Rng> }

impl EntropyPool {
    pub const fn new() -> Self { Self { rng: None } }

    /// Fold 32 bytes of entropy into the pool. The first call seeds it; later
    /// calls MIX: draw 32 bytes of current output, XOR with `bytes`, and
    /// re-key with the result — so new entropy is added to existing state
    /// (a stuck source cannot erase prior entropy).
    pub fn reseed(&mut self, bytes: [u8; 32]) { ... }

    /// Fill `out` with CSPRNG output (ratchets the stream). If the pool has
    /// never been seeded, fills zeros (a kernel-config bug; the kernel asserts
    /// it seeds before drawing).
    pub fn fill(&mut self, out: &mut [u8]) { ... }

    /// Convenience: a fresh 32-byte draw (e.g. an ML-KEM seed).
    pub fn next_seed(&mut self) -> [u8; 32] { ... }
}
```

`reseed` uses `rand_core::RngCore::fill_bytes` on the current `ChaCha20Rng` to
get the mixing bytes, then `ChaCha20Rng::from_seed(mixed)`. `fill`/`next_seed`
use `fill_bytes`. (All from existing deps — no new crate.)

### 3.3 Kernel state and accessors

In the kernel `bare` module:

```
static ENTROPY_POOL: SingleHartCell<EntropyPool> = SingleHartCell::new(EntropyPool::new());
fn pool_reseed(bytes: [u8; 32]) { ENTROPY_POOL.with(|p| p.reseed(bytes)); }
fn pool_next_seed() -> [u8; 32] { ENTROPY_POOL.with(|p| p.next_seed()) }
```

`SingleHartCell` (from `kernel_arch_riscv64::mem`) is the same re-entry-guarded
primitive the scheduler and frame allocator use.

### 3.4 Flow (the `pqc` consumer, rewired)

The `entropy` component is unchanged: it draws 32 bytes from virtio-rng twice
and `send`s each over IPC. The consumer now routes them through the pool:

```
let d1 = recv_message(ENTROPY_CAP);  pool_reseed(seed_from_message(&d1));
println!("entropy: pool seeded from virtio-rng");

let a = pool_next_seed();  let b = pool_next_seed();
assert a != b;  println!("entropy: pool serves on demand (draws differ)");

let d2 = recv_message(ENTROPY_CAP);  pool_reseed(seed_from_message(&d2));
println!("entropy: pool reseeded from virtio-rng");

let seed = pool_next_seed();
match ml_kem768_agree(seed) { Some => "pqc: ML-KEM-768 round-trip ok (pool-seeded)", None => FAIL }
```

The two `recv`s rendezvous with the component's two `send`s exactly as today;
the only change is what the consumer does with the bytes. (`seed_from_message`
already exists.) The "two draws differ" claim now demonstrates the *pool's*
stream from a single device seed — a stronger statement than the old
device-passthrough check.

### 3.5 Error handling summary

| Situation | Behavior |
|-----------|----------|
| `next_seed`/`fill` before any `reseed` | Pool unseeded → fills zeros. The consumer always seeds before drawing; an accessor `debug_assert`/comment documents "seed first". |
| Two successive pool draws collide | Astronomically unlikely (ChaCha stream); the smoke's `draws differ` check would catch a broken stream. |
| No virtio-rng device (smoke without `-device`) | The component isn't spawned (existing behavior); the consumer never seeds the pool and never runs the demo — unchanged from today. |
| ML-KEM disagree | `pqc: ML-KEM-768 FAIL` (unchanged failure path). |

## 4. Testing

- **Host unit tests** (`kernel_crypto`, `cargo test`):
  - `seeded pool yields distinct successive draws` — `reseed([1;32])`, then
    `next_seed() != next_seed()`.
  - `same seed reproduces the stream` — two pools each `reseed([7;32])` give
    identical first draws.
  - `reseed changes the stream` — after a draw, `reseed([2;32])` then the next
    draw differs from what the un-reseeded stream would have produced.
  - `reseed mixes` — a pool seeded `[1;32]` then reseeded `[2;32]` produces a
    draw different from a fresh pool seeded `[2;32]` alone (proves mixing, not
    replacement).
- **QEMU smoke test** (`tools/test-qemu.ps1`, extended; written first,
  failing): replace `entropy: virtio-rng live (two draws differ)` and `pqc:
  ML-KEM-768 round-trip ok (entropy-seeded)` with `entropy: pool seeded from
  virtio-rng`, `entropy: pool serves on demand (draws differ)`, `entropy: pool
  reseeded from virtio-rng`, and `pqc: ML-KEM-768 round-trip ok (pool-seeded)`;
  keep every other milestone.

## 5. Deliverables

1. `libs/crypto/src/lib.rs`: `EntropyPool` (`new`/`reseed`/`fill`/`next_seed`)
   + host tests.
2. `kernel/src/main.rs`: the `SingleHartCell<EntropyPool>` static, the
   `pool_reseed`/`pool_next_seed` accessors, and the rewired `pqc_consumer`.
3. Extended QEMU smoke test + host tests, all green.
4. Short learning note `docs/learning/0018-kernel-entropy-pool.md`.
5. Roadmap: the entropy pool marked done (the "next candidate" note updated).
6. Glossary: only genuinely new terms (e.g. *CSPRNG / DRBG*, *entropy pool*).

## 6. Open questions (for later phases)

- **A U-mode `getrandom` service** (capability-gated entropy endpoint) once a
  component needs randomness.
- **Continuous reseeding** (the component drawing on a timer; an entropy
  estimate gating first use).
- **A NIST-certified DRBG** + health tests, if the project ever needs
  certification.
- **Using the pool to retire other fixed seeds / nonces** across the kernel.
