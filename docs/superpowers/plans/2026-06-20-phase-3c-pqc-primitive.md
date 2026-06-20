# Phase 3c: ML-KEM post-quantum primitive — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline, per the user's preference for this project) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Integrate the audited RustCrypto `ml-kem` crate into `libs/crypto` and prove one primitive — an ML-KEM-768 round-trip (keygen → encapsulate → decapsulate, shared secrets agree) — works both host-tested and on the bare riscv64 `no_std` kernel.

**Architecture:** `libs/crypto` gains `ml-kem` + `rand_chacha` + `rand_core` (all `--no-default-features`, `no_std`/no-alloc), exposes `ml_kem768_agree(seed) -> Option<[u8;32]>`, and the kernel calls it from `kmain` with a fixed (non-secret, flagged) seed, printing a proof line the smoke test asserts. The first real dependency means `Cargo.lock` gets committed.

**Tech Stack:** Rust `no_std`, RISC-V (riscv64gc), QEMU smoke test. Crates: `ml-kem` 0.3.2, `rand_chacha` 0.10, `rand_core` 0.10.1 (one shared `rand_core` — verified). Host tests: `cargo test -p kernel-crypto`. Bare build: `cargo build -p kernel-crypto --target riscv64gc-unknown-none-elf` and `cargo build -p kernel --target riscv64gc-unknown-none-elf`.

**Spec:** `docs/superpowers/specs/2026-06-20-phase-3c-pqc-primitive-design.md`

**Verified API (RustCrypto ml-kem 0.3.2):** `MlKem768::generate_keypair_from_rng(&mut rng) -> (dk, ek)` (the `Kem` trait); `ek.encapsulate_with_rng(&mut rng) -> (Ciphertext, SharedKey)` (the `Encapsulate` trait, infallible); `dk.decapsulate(&ct) -> SharedKey` (the `Decapsulate` trait, infallible). `SharedKey` is a 32-byte `Array` (`==` and `.as_slice()` work). `ChaCha20Rng::from_seed([u8;32])` (the `SeedableRng` trait). With `--no-default-features` the `getrandom`/`alloc` features are off and the whole tree builds `no_std`/no-alloc on riscv64 (confirmed).

---

## Task 1: add the crypto dependencies, `no_std`, and commit `Cargo.lock`

**Files:**
- Modify: `libs/crypto/Cargo.toml` (via `cargo add`)
- Modify: `libs/crypto/src/lib.rs` (add `#![no_std]` attribute)
- Modify: `.gitignore` (un-ignore `Cargo.lock`)
- Add: `Cargo.lock`

- [ ] **Step 1: Add the three dependencies (no default features)**

Run:
```bash
cargo add ml-kem --no-default-features -p kernel-crypto
cargo add rand_chacha --no-default-features -p kernel-crypto
cargo add rand_core --no-default-features -p kernel-crypto
```
Expected: `libs/crypto/Cargo.toml` gains `ml-kem`, `rand_chacha`, `rand_core` under `[dependencies]`, each `default-features = false`. (`ml-kem` resolves to 0.3.2, `rand_chacha` 0.10.0, `rand_core` 0.10.1.)

- [ ] **Step 2: Make `libs/crypto` `no_std`**

In `libs/crypto/src/lib.rs`, add the attribute as the very first line (it is currently absent — the crate was host-only):

```rust
#![cfg_attr(not(test), no_std)]
```

(Leave the rest of the placeholder file untouched for now; Task 2 replaces the body.)

- [ ] **Step 3: Verify host + bare builds with the deps present**

Run: `cargo build -p kernel-crypto`
Expected: SUCCESS (host).

Run: `cargo build -p kernel-crypto --target riscv64gc-unknown-none-elf`
Expected: SUCCESS — this is the `no_std`/no-alloc integration gate. If it fails with "can't find crate for `std`" or an `alloc` error from a dependency, STOP and report: a transitive crate needs `std`/`alloc`, which means the crate/feature choice must be revisited (do not add a heap to work around it).

- [ ] **Step 4: Un-ignore and commit `Cargo.lock`**

In `.gitignore`, remove the line:

```
Cargo.lock
```

(It is the only change to `.gitignore`.) Then stage the lockfile.

- [ ] **Step 5: Commit**

```bash
git add libs/crypto/Cargo.toml libs/crypto/src/lib.rs .gitignore Cargo.lock
git commit -m "build(crypto): add ml-kem/rand_chacha deps (no_std, no-alloc); commit Cargo.lock"
```

---

## Task 2: the `ml_kem768_agree` wrapper (host-tested)

**Files:**
- Modify: `libs/crypto/src/lib.rs` (replace the placeholder with the wrapper + tests)

- [ ] **Step 1: Write the wrapper and the failing tests**

Replace the entire body of `libs/crypto/src/lib.rs` (keeping the `#![cfg_attr(not(test), no_std)]` first line) with:

```rust
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
```

- [ ] **Step 2: Run the tests**

Run: `cargo test -p kernel-crypto`
Expected: PASS — `roundtrip_agrees`, `different_seeds_give_different_secrets`, `tampered_ciphertext_does_not_agree` all green.

(If `ct[0] ^= 0xff` does not compile because the `Ciphertext` array's indexing differs, use `ct.as_mut_slice()[0] ^= 0xff;` instead — the `Array` type derefs to `[u8]`.)

- [ ] **Step 3: Verify the bare build still succeeds**

Run: `cargo build -p kernel-crypto --target riscv64gc-unknown-none-elf`
Expected: SUCCESS (the real wrapper compiles `no_std`/no-alloc).

- [ ] **Step 4: Commit**

```bash
git add libs/crypto/src/lib.rs Cargo.lock
git commit -m "feat(crypto): ml_kem768_agree - ML-KEM-768 round-trip primitive (host-tested)"
```

(`Cargo.lock` is staged in case resolution metadata changed; it likely did not.)

---

## Task 3: update the smoke test for the PQC milestone (red)

**Files:**
- Modify: `tools/test-qemu.ps1`

- [ ] **Step 1: Add the PQC pattern**

In `tools/test-qemu.ps1`, add one line to the `$mustMatch` array (put it just before the `"tick: 2(?!\d)"` entry):

```powershell
    "pqc: ML-KEM-768 round-trip ok",
```

Also update the header comment block and the PASS result message to mention the 3c milestone, e.g. add: "and the Phase 3c milestone — an ML-KEM-768 post-quantum round-trip runs on the bare kernel".

- [ ] **Step 2: Run the smoke test — expect RED**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: FAIL — `kmain` does not call the KEM yet, so `pqc: ML-KEM-768 round-trip ok` is missing.

- [ ] **Step 3: Commit (red test pinned)**

```bash
git add tools/test-qemu.ps1
git commit -m "test: smoke test asserts the Phase 3c ML-KEM round-trip (red)"
```

---

## Task 4: kernel demo — run the KEM on the bare target (green)

**Files:**
- Modify: `kernel/Cargo.toml` (depend on `kernel-crypto`)
- Modify: `kernel/src/main.rs` (call the KEM in `kmain`)

- [ ] **Step 1: Add the kernel → crypto dependency**

In `kernel/Cargo.toml`, under `[dependencies]`, add (matching the existing path-dep style):

```toml
kernel-crypto = { path = "../libs/crypto" }
```

- [ ] **Step 2: Call the KEM from `kmain`**

In `kernel/src/main.rs`, in `kmain` (inside the `bare` module), add a call to a new `pqc_demo()` right after `frame_roundtrip();` and before the `// Phase 3b-iii:` spawn-block comment:

```rust
        frame_roundtrip();
        pqc_demo();
```

Add the `pqc_demo` function alongside the other helper fns in the `bare` module (e.g. just after `frame_roundtrip`):

```rust
    /// Phase 3c: prove the post-quantum KEM runs on the bare kernel — an
    /// ML-KEM-768 round-trip whose two shared secrets must agree. The seed
    /// is FIXED and NOT secret (real entropy seeding is deferred); this
    /// proves the algorithm runs no_std/no-alloc on the kernel, not that it
    /// is securely keyed.
    fn pqc_demo() {
        const PQC_DEMO_SEED: [u8; 32] = [0x3c; 32];
        match kernel_crypto::ml_kem768_agree(PQC_DEMO_SEED) {
            Some(_) => println!("pqc: ML-KEM-768 round-trip ok (shared secret agreed)"),
            None => println!("pqc: ML-KEM-768 FAIL (secrets disagreed)"),
        }
    }
```

(No `use` is needed — `kernel_crypto::ml_kem768_agree` is referenced by full path. The crate `kernel-crypto` is imported as `kernel_crypto`.)

- [ ] **Step 3: Build the kernel binary**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: SUCCESS.

- [ ] **Step 4: Run the smoke test — expect GREEN**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS` — including `pqc: ML-KEM-768 round-trip ok`. If it shows `pqc: ... FAIL`, the shared secrets disagreed (a real bug — investigate the wrapper, do not weaken the test).

- [ ] **Step 5: Commit**

```bash
git add kernel/Cargo.toml kernel/src/main.rs Cargo.lock
git commit -m "feat: Phase 3c live - ML-KEM-768 round-trip runs on the bare kernel"
```

---

## Task 5: docs — short learning note, roadmap (Phase 3 complete), glossary; final verification

**Files:**
- Create: `docs/learning/0010-post-quantum-crypto.md`
- Modify: `docs/roadmap/roadmap.md`
- Modify: `docs/glossary.md`
- Modify: `docs/learning/README.md`

- [ ] **Step 1: Write a short learning note**

Create `docs/learning/0010-post-quantum-crypto.md` (brief, per project preference):

```markdown
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

## Proof
Host tests (round-trip agrees; different seeds differ; a tampered ciphertext
does not agree). Smoke test: `pqc: ML-KEM-768 round-trip ok` on the bare
kernel. This completes Phase 3 (the security spine).
```

- [ ] **Step 2: Update the roadmap (3c done, Phase 3 complete)**

In `docs/roadmap/roadmap.md`, replace the `### Phase 3c — PQC primitive` block with:

```markdown
### Phase 3c — PQC primitive  *(done — 2026-06-20)*

- **Goal:** integrate an audited post-quantum crypto crate and expose one
  usable primitive (per ADR 0004).
- **You learn:** what a KEM is, integrating an external `no_std`/no-alloc
  crate into the bare kernel, and why the first real dependency means
  committing `Cargo.lock` (see [learning note 0010](../learning/0010-post-quantum-crypto.md)).
- **Done when:** `./tools/test-qemu.ps1` observes an ML-KEM-768 round-trip
  succeed on the bare kernel, with the wrapper host-tested (round-trip
  agrees, distinct seeds differ, a tampered ciphertext does not).

**Phase 3 (security spine) is complete:** U-mode (3a), the run queue +
address-space isolation + capability-checked IPC (3b), and a post-quantum
primitive (3c). Next is Phase 4 — real hardware.
```

- [ ] **Step 3: Add glossary entries**

In `docs/glossary.md`, add entries (in the file's format) for the genuinely
new terms — **post-quantum cryptography (PQC)** (encryption designed to
resist a quantum-equipped attacker; ADR 0004's baseline), **KEM
(key-encapsulation mechanism)** (a scheme that establishes a shared secret:
encapsulate to a public key → (ciphertext, secret); decapsulate the
ciphertext with the private key → the same secret), **ML-KEM** (the
NIST/FIPS 203 post-quantum KEM, formerly Kyber; this kernel uses ML-KEM-768),
and **shared secret** (the symmetric key two parties derive via a KEM).
`Capability` already has an entry; don't duplicate.

- [ ] **Step 4: Update the learning-notes index**

In `docs/learning/README.md`, add under the notes list:

```markdown
- [0010 — A post-quantum primitive: ML-KEM (Phase 3c)](0010-post-quantum-crypto.md)
```

- [ ] **Step 5: Final verification (run all, paste results)**

Run: `cargo test -p kernel-crypto` → expect all green.
Run: `cargo test -p kernel-arch-riscv64` → expect all green (unchanged).
Run: `cargo build --workspace` → expect success.
Run (PowerShell): `./tools/check-references.ps1` → expect PASS (new note + roadmap links resolve; fix any of YOUR broken references).
Run (PowerShell): `./tools/test-qemu.ps1` → expect `BOOT TEST PASS`.

- [ ] **Step 6: Commit**

```bash
git add docs/learning/0010-post-quantum-crypto.md docs/roadmap/roadmap.md docs/glossary.md docs/learning/README.md
git commit -m "docs: Phase 3c learning note, roadmap (Phase 3 complete), glossary terms"
```

---

## Done-when checklist (maps to spec §1)

- [ ] ML-KEM-768 round-trip runs on the bare kernel — smoke pattern `pqc: ML-KEM-768 round-trip ok`.
- [ ] Host tests pass — round-trip agrees, distinct seeds differ, tampered ciphertext does not agree.
- [ ] `Cargo.lock` committed; `.gitignore` no longer ignores it.
- [ ] `check-references` clean; `cargo build --workspace` green; arch host tests unchanged.
