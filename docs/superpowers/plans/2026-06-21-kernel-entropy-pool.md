# Kernel entropy pool Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline, per the user's preference for this project) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A reseedable kernel-side ChaCha20 CSPRNG, seeded by the virtio-rng component, serving entropy on demand to kernel code (ML-KEM) — replacing the one-shot boot seed.

**Architecture:** A pure `EntropyPool` (ChaCha20 DRBG) in `kernel_crypto`; a `SingleHartCell<EntropyPool>` static + accessors in the kernel binary (keeping crypto out of the `arch` crate); the existing `pqc` consumer rewired to seed/reseed the pool from the entropy component's device draws and key ML-KEM from a pool draw.

**Tech Stack:** Rust `no_std`, RISC-V (riscv64gc), QEMU. Host tests: `cargo test -p kernel-crypto` / `-p kernel-arch-riscv64`. Bare: `cargo build -p kernel --target riscv64gc-unknown-none-elf`.

**Spec:** `docs/superpowers/specs/2026-06-21-kernel-entropy-pool-design.md`

**Existing-code facts (verified):**
- `kernel_crypto` (`libs/crypto/src/lib.rs`) is `#![cfg_attr(not(test), no_std)]` and already imports `rand_chacha::ChaCha20Rng`, `rand_core::SeedableRng`. `rand_core::RngCore` (for `fill_bytes`) is available from the same dep.
- `kernel_arch_riscv64::mem::SingleHartCell<T>` is `pub` with `const fn new(value: T)` and `with(|&mut T| ...)` (the same primitive backing `SCHED`); it is `Sync` for any `T`.
- `kernel/src/main.rs` `bare` module imports `mem` and uses `kernel_crypto::ml_kem768_agree(...)` by full path. The `pqc_consumer` (a kernel task) calls `sched::recv_message(ENTROPY_CAP)` twice, `seed_from_message(&m)` to rebuild `[u8;32]`, runs `ml_kem768_agree`, then idles. `seed_from_message` already exists. `ENTROPY_CAP`=0.
- Smoke currently asserts `entropy: virtio-rng live \(two draws differ\)` and `pqc: ML-KEM-768 round-trip ok \(entropy-seeded\)`.

---

## Task 1: the `EntropyPool` CSPRNG (pure, host-tested)

**Files:**
- Modify: `libs/crypto/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

In `libs/crypto/src/lib.rs`, add to the `tests` module:

```rust
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
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p kernel-crypto`
Expected: FAIL — `EntropyPool` does not exist (compile error).

- [ ] **Step 3: Implement `EntropyPool`**

In `libs/crypto/src/lib.rs`, change the `rand_core` import to also bring in `RngCore`:

```rust
use rand_core::{RngCore, SeedableRng};
```

Then add (after `ml_kem768_agree`, before the `tests` module):

```rust
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
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p kernel-crypto`
Expected: PASS — the five new tests and the existing ML-KEM tests.

- [ ] **Step 5: Verify the bare kernel still builds (no_std/no-alloc)**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: SUCCESS (the pool uses only existing no_std deps).

- [ ] **Step 6: Commit**

```bash
git add libs/crypto/src/lib.rs
git commit -m "feat(crypto): EntropyPool - a reseedable ChaCha20 CSPRNG (host-tested)"
```

---

## Task 2: update the smoke test for the pool (red)

**Files:**
- Modify: `tools/test-qemu.ps1`

- [ ] **Step 1: Swap the entropy/PQC assertions**

In `tools/test-qemu.ps1`, replace these two lines in `$mustMatch`:

```powershell
    "entropy: virtio-rng live \(two draws differ\)",
    "pqc: ML-KEM-768 round-trip ok \(entropy-seeded\)",
```

with:

```powershell
    "entropy: pool seeded from virtio-rng",
    "entropy: pool serves on demand \(draws differ\)",
    "entropy: pool reseeded from virtio-rng",
    "pqc: ML-KEM-768 round-trip ok \(pool-seeded\)",
```

Update the header comment and PASS message to say the device entropy now seeds
a reseedable kernel entropy pool (a ChaCha20 CSPRNG) that serves ML-KEM on
demand, rather than a one-shot seed.

- [ ] **Step 2: Run the smoke test — expect RED**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: FAIL — the four new `pool` lines are absent (the consumer still prints the old `virtio-rng live` / `entropy-seeded` lines).

- [ ] **Step 3: Commit (red test pinned)**

```bash
git add tools/test-qemu.ps1
git commit -m "test: smoke test asserts the kernel entropy pool seeds/reseeds/serves ML-KEM (red)"
```

---

## Task 3: wire the pool into the kernel and the consumer (green)

**Files:**
- Modify: `kernel/src/main.rs`

- [ ] **Step 1: Add the pool static and accessors**

In `kernel/src/main.rs`, add immediately before `fn seed_from_message` (so it sits with the consumer code):

```rust
    /// The kernel entropy pool: a ChaCha20 CSPRNG seeded by the virtio-rng
    /// component, serving entropy to kernel code on demand (reseedable). The
    /// same single-hart cell primitive the scheduler/frame allocator use.
    static ENTROPY_POOL: mem::SingleHartCell<kernel_crypto::EntropyPool> =
        mem::SingleHartCell::new(kernel_crypto::EntropyPool::new());

    /// Fold 32 bytes of device entropy into the pool (seeds on first call,
    /// mixes after).
    fn pool_reseed(bytes: [u8; 32]) {
        ENTROPY_POOL.with(|p| p.reseed(bytes));
    }

    /// Draw a fresh 32-byte seed from the pool (the kernel seeds it first).
    fn pool_next_seed() -> [u8; 32] {
        ENTROPY_POOL.with(|p| p.next_seed())
    }
```

- [ ] **Step 2: Rewire `pqc_consumer` to use the pool**

In `kernel/src/main.rs`, replace the `pqc_consumer` function (keep `seed_from_message` as is). Update its doc comment to describe the pool flow, and the body:

```rust
    /// The entropy consumer (kernel task): seeds the kernel entropy pool from
    /// the virtio-rng component's device draws, proves the pool serves entropy
    /// on demand (one device seed yields a stream) and reseeds with fresh
    /// entropy, then keys the ML-KEM-768 round-trip from a pool draw — so the
    /// post-quantum demo is seeded by the reseedable pool, not a one-shot read.
    /// Then idles cooperatively (kernel tasks never return).
    extern "C" fn pqc_consumer() -> ! {
        // Seed the pool from the first device draw.
        let d1 = sched::recv_message(ENTROPY_CAP);
        pool_reseed(seed_from_message(&d1));
        println!("entropy: pool seeded from virtio-rng");

        // The pool serves entropy on demand: one device seed yields a stream.
        let a = pool_next_seed();
        let b = pool_next_seed();
        if a != b {
            println!("entropy: pool serves on demand (draws differ)");
        } else {
            println!("entropy: WARNING pool draws identical");
        }

        // Fold a second device draw in — reseeding mixes new entropy with state.
        let d2 = sched::recv_message(ENTROPY_CAP);
        pool_reseed(seed_from_message(&d2));
        println!("entropy: pool reseeded from virtio-rng");

        // Key ML-KEM from a pool draw (not the raw device bytes).
        let seed = pool_next_seed();
        match kernel_crypto::ml_kem768_agree(seed) {
            Some(_) => println!("pqc: ML-KEM-768 round-trip ok (pool-seeded)"),
            None => println!("pqc: ML-KEM-768 FAIL (secrets disagreed)"),
        }
        loop {
            sched::yield_now();
            // SAFETY: wait for the next interrupt between yields.
            unsafe { core::arch::asm!("wfi") };
        }
    }
```

- [ ] **Step 3: Build the kernel**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: SUCCESS.

- [ ] **Step 4: Run the smoke test — expect GREEN**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS`, including `entropy: pool seeded from virtio-rng`, `entropy: pool serves on demand (draws differ)`, `entropy: pool reseeded from virtio-rng`, and `pqc: ML-KEM-768 round-trip ok (pool-seeded)`, with the RTC call/reply, healing (5a/5b), and all prior milestones still present.

Troubleshooting (diagnose, don't weaken the test):
- `WARNING pool draws identical` → `pool_next_seed` is not ratcheting: confirm `EntropyPool::fill` calls `rng.fill_bytes` on a seeded `Some(rng)` (not re-keying each draw).
- Missing `pool-seeded` line / hang → the consumer still needs the two device `recv`s in order; confirm both `recv_message(ENTROPY_CAP)` calls remain and the entropy component still sends twice.

- [ ] **Step 5: Commit**

```bash
git add kernel/src/main.rs
git commit -m "feat: kernel entropy pool - device entropy seeds a CSPRNG that keys ML-KEM on demand"
```

---

## Task 4: docs — learning note, roadmap, glossary; final verification

**Files:**
- Create: `docs/learning/0018-kernel-entropy-pool.md`
- Modify: `docs/roadmap/roadmap.md`
- Modify: `docs/glossary.md`
- Modify: `docs/learning/README.md`

- [ ] **Step 1: Write a short learning note**

Create `docs/learning/0018-kernel-entropy-pool.md`:

```markdown
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

## Proof
`entropy: pool seeded from virtio-rng` → `pool serves on demand (draws differ)`
→ `pool reseeded from virtio-rng` → `pqc: ML-KEM-768 round-trip ok
(pool-seeded)`: real device entropy, pooled and reseedable, now keys the
post-quantum round-trip on demand.
```

- [ ] **Step 2: Update the roadmap**

In `docs/roadmap/roadmap.md`, find the "(Next candidates: …)" note that follows the call/reply block and lists "a kernel entropy pool/CSPRNG seeded by virtio-rng" first. Replace that parenthetical with:

```markdown
### Kernel entropy pool  *(done — 2026-06-21)*

- **Goal:** a reseedable kernel CSPRNG seeded by the virtio-rng component,
  serving entropy on demand to kernel crypto — replacing the one-shot boot
  seed for ML-KEM.
- **You learn:** a kernel RNG is a CSPRNG seeded by a hardware source (a finite
  read becomes unlimited output); reseeding *mixes* new entropy into existing
  state rather than replacing it (see
  [learning note 0018](../learning/0018-kernel-entropy-pool.md)).
- **Done when:** `./tools/test-qemu.ps1` shows the pool seeded from virtio-rng,
  serving distinct on-demand draws, reseeded, and keying the ML-KEM round-trip.
  QEMU-only.

(Next candidates: a capability-gated U-mode `getrandom` service over the pool;
an interrupt-driven (PLIC) device path; one-shot reply capabilities for
deferred/forwarded replies.)
```

- [ ] **Step 3: Add glossary entries**

In `docs/glossary.md`, add after the existing PQC/KEM cluster (only genuinely new terms):

```markdown
- **CSPRNG / DRBG** — a cryptographically secure pseudo-random generator: from a small random *seed* it produces an unbounded stream that is computationally indistinguishable from random. The kernel's entropy pool is a CSPRNG built on the ChaCha20 stream cipher.
- **Entropy pool** — the kernel's randomness source: a CSPRNG seeded by a hardware entropy device (here the virtio-rng component) that serves randomness on demand and is periodically *reseeded* with fresh device entropy (mixed into, not replacing, its state).
```

- [ ] **Step 4: Update the learning-notes index**

In `docs/learning/README.md`, add after the 0017 line:

```markdown
- [0018 — A kernel entropy pool](0018-kernel-entropy-pool.md)
```

- [ ] **Step 5: Final verification (run all, paste results)**

Run: `cargo test -p kernel-crypto` → expect all green.
Run: `cargo test -p kernel-arch-riscv64` → expect all green.
Run: `cargo build --workspace` → expect success.
Run (PowerShell): `./tools/check-references.ps1` → expect PASS.
Run (PowerShell): `./tools/test-qemu.ps1` → expect `BOOT TEST PASS`.

- [ ] **Step 6: Commit**

```bash
git add docs/learning/0018-kernel-entropy-pool.md docs/roadmap/roadmap.md docs/glossary.md docs/learning/README.md
git commit -m "docs: kernel entropy pool - learning note, roadmap, glossary"
```

---

## Done-when checklist (maps to spec §1)

- [ ] **Seeded → on-demand → reseeded → pool-keyed** — smoke shows all four `pool` lines in order, with the system still running.
- [ ] **Host tests** — distinct successive draws, same-seed reproducibility, reseed changes the stream, reseed mixes (not replaces), unseeded yields zeros.
- [ ] `check-references` clean; `cargo build --workspace` green; `BOOT TEST PASS`.
```
