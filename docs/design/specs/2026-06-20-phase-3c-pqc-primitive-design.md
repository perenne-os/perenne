# Kernel — Phase 3c Design: A post-quantum primitive (ML-KEM)

- **Date:** 2026-06-20
- **Status:** Draft — awaiting review
- **Scope of this document:** Phase 3c only — integrating an audited
  post-quantum KEM into `libs/crypto` and proving one primitive (an
  **ML-KEM-768 round-trip**) is usable, both host-tested and run on the
  bare riscv64 kernel. This completes Phase 3 (the security spine).

---

## 0. Where 3c sits

Phase 3 ("security spine") was decomposed into **3a** (user mode — done),
**3b** (capabilities & IPC — done: 3b-i/ii/iii) → **3c** (this doc, the PQC
primitive). Per [ADR 0004](../decisions/0004-post-quantum-crypto.md), the
project adopts a post-quantum cryptography baseline and **prefers audited
libraries over hand-rolled crypto**; concrete library selection was
explicitly deferred to "Phase 3, when crypto is actually integrated," with
`libs/crypto` a placeholder until now. 3c makes that real for one primitive.

This is a *different kind* of phase from 3a/3b: it introduces the project's
**first external dependency**, so it also settles dependency hygiene
(committing `Cargo.lock`) and proves an off-the-shelf `no_std` crate builds
and runs on the bare kernel.

## 1. Goal

`libs/crypto` provides a real post-quantum **key-encapsulation mechanism**
(KEM): generate an ML-KEM-768 keypair, **encapsulate** a shared secret to
the public key (producing a ciphertext), and **decapsulate** that
ciphertext with the private key — and both sides derive the *same* shared
secret. The primitive is host-tested and also executed on the real riscv64
`no_std` kernel.

**You learn (kept brief):** what a KEM is and the encapsulate/decapsulate
shape of post-quantum key exchange; how to bring an external `no_std`,
no-`alloc` crate into the bare kernel; and why the first real dependency
means committing `Cargo.lock`.

**Done when** `./tools/test-qemu.ps1` observes, in one boot, *in addition
to the existing milestones* (greeting, breakpoint recovery, paging line,
W^X block, frame round-trip, ≥ 2 ticks, the 3b IPC milestone):

1. **ML-KEM-768 round-trip on the bare target** — `kmain` runs keygen →
   encapsulate → decapsulate via `libs/crypto` and prints
   `pqc: ML-KEM-768 round-trip ok (shared secret agreed)` because the two
   shared secrets matched.

And, off the bare target:

2. **Host unit tests pass** — the round-trip agrees; two different seeds
   produce different shared secrets; a tampered ciphertext decapsulates to
   a *different* secret (ML-KEM implicit rejection) — so the wrapper is
   provably doing the real work, not returning a constant.

3. **`Cargo.lock` is committed** — the build of the audited dependency is
   reproducible.

## 2. Non-goals (deferred)

- **Real entropy** — the demo seeds a CSPRNG from a *fixed, non-secret*
  seed (§3.3). Seeding from a hardware/entropy source (and a kernel CSPRNG
  service) is deferred to Phase 4+. **This means the demo keys are not
  secret; it proves the algorithm runs, not that it is securely keyed.**
- **Exposing crypto to U-mode** — no `crypto` syscall; 3c calls the library
  from kernel code only. A capability-gated crypto service for user tasks
  is a later phase (it would build on 3b's capabilities/IPC).
- **Signatures (ML-DSA)** — only a KEM in 3c; signatures are a future
  addition to `libs/crypto`.
- **Using PQC for a real channel** — 3c proves the primitive works; wiring
  a shared secret into an encrypted IPC channel or secure boot is later.
- **Key storage / lifecycle / zeroization policy** beyond what the crate
  does; **side-channel / constant-time auditing** beyond what the crate
  provides; **classical-hybrid** KEM (X25519+ML-KEM).
- **A heap** — the integration must work with no global allocator (§3.4);
  adding `alloc`/a heap is out of scope.

## 3. Design

### 3.1 Components

| Component | Location | Responsibility |
|-----------|----------|----------------|
| `ml-kem` + `rand_chacha` deps | `libs/crypto/Cargo.toml` | The audited KEM and a CSPRNG, both with `default-features = false` to stay `no_std` / no-`alloc`. |
| `ml_kem768_agree` | `libs/crypto/src/lib.rs` | The wrapper: seed a CSPRNG, generate a keypair, encapsulate, decapsulate, and return `Some(shared_secret)` iff the encapsulated and decapsulated secrets match. Replaces the `PQC_PLANNED` placeholder. |
| kernel dependency + demo | `kernel/Cargo.toml`, `kernel/src/main.rs` | Depend on `kernel-crypto`; in `kmain`, call the wrapper with a fixed seed and print the proof (or a loud FAIL). |
| committed lockfile | `.gitignore`, `Cargo.lock` | Un-ignore and commit `Cargo.lock` for reproducible builds. |

### 3.2 The KEM wrapper

`libs/crypto` exposes one function (the public surface stays tiny; the
typed key/ciphertext types are an internal detail until a consumer needs
them):

```
/// Run an ML-KEM-768 round-trip seeded by `seed`: generate a keypair,
/// encapsulate a shared secret to the public key, then decapsulate the
/// ciphertext with the private key. Returns Some(secret) iff the
/// encapsulated and decapsulated 32-byte shared secrets are identical.
pub fn ml_kem768_agree(seed: [u8; 32]) -> Option<[u8; 32]>
```

Internally (RustCrypto `ml-kem` shape): seed a `ChaCha20Rng`; `generate`
the (decapsulation key, encapsulation key) pair; `encapsulate` on the
encapsulation key → `(ciphertext, shared_secret_a)`; `decapsulate` the
ciphertext with the decapsulation key → `shared_secret_b`; return
`Some(secret)` when `shared_secret_a == shared_secret_b`, else `None`. The
32-byte secrets are copied out of the crate's array type into a plain
`[u8; 32]` for a dependency-free return.

ML-KEM is *implicit-rejection*: decapsulation never errors — a bad
ciphertext yields a pseudo-random (different) secret rather than a failure.
So the wrapper's equality check is the meaningful correctness signal, and a
tamper test (§4) confirms a corrupted ciphertext does **not** agree.

### 3.3 Randomness — a fixed-seed CSPRNG

ML-KEM keygen and encapsulation consume randomness. The bare kernel has no
entropy source yet (no hardware RNG wired up, no OS). The design seeds a
**real CSPRNG** — `rand_chacha::ChaCha20Rng::from_seed(seed)` — from a
**fixed, non-secret 32-byte seed** chosen in `kmain`.

This is deliberately *not* a hand-rolled RNG (which would violate ADR
0004's "don't hand-roll crypto") and *not* a non-cryptographic RNG wearing
a `CryptoRng` marker. It is a sound CSPRNG construction made deterministic
only by the fixed seed. The honest limitation: **a fixed seed is public, so
the demo's keys are reproducible and not secret.** Real security needs the
seed to come from genuine entropy — deferred (§2). The wrapper takes the
seed as a parameter precisely so a later phase can pass real entropy
without changing the KEM code.

### 3.4 `no_std` / no-`alloc` on the bare target

`libs/crypto` is already `no_std`-capable and used by the kernel binary
(built for `riscv64gc-unknown-none-elf`, which has no `std` and — in this
project — no global allocator). Both deps are added with
`default-features = false` so they don't pull `std`; `ml-kem` and
`rand_chacha` are no-`alloc` in that configuration (fixed-size arrays via
`hybrid-array`, no heap). **The first kernel build after adding the
dependency is the gate**: if either crate (or a transitive dep) needs
`std` or `alloc`, the bare build fails immediately, and the design is
revisited (crate/feature choice) before going further — raised with the
user, not worked around by adding a heap.

### 3.5 The kernel demo

`kmain`, after the existing 2a/2b probes and before scheduling, calls the
wrapper with a fixed seed and reports:

```
match kernel_crypto::ml_kem768_agree(PQC_DEMO_SEED) {
    Some(_) => println!("pqc: ML-KEM-768 round-trip ok (shared secret agreed)"),
    None    => println!("pqc: ML-KEM-768 FAIL (secrets disagreed)"),
}
```

`PQC_DEMO_SEED` is a fixed `[u8; 32]` constant (e.g. a recognizable byte
pattern) with a comment that it is **not** secret. The shared secret bytes
themselves are not printed (no value in it, and avoids implying they're
meaningful). This runs in S-mode kernel context — a plain library call, no
syscall path. It proves the crate compiles *and executes* on the bare
no_std/no-alloc kernel, which host tests cannot show.

### 3.6 Dependency hygiene — committing `Cargo.lock`

`Cargo.lock` is currently gitignored (a Phase 0 choice that was fine with
zero external dependencies). With an audited crypto dependency entering the
tree, the exact resolved versions become security-relevant and must be
reproducible. 3c **removes `Cargo.lock` from `.gitignore` and commits it.**
A short note records why (so the Phase 0 rationale isn't silently
contradicted).

## 4. Testing

Test-first where it fits (the wrapper is host-testable):

- **Host unit tests** (`libs/crypto`, `cargo test -p kernel-crypto`):
  - **round-trip agrees:** `ml_kem768_agree(seed)` is `Some` for a sample
    seed, and the secret is 32 bytes (not all-zero).
  - **different seeds differ:** two distinct seeds produce different shared
    secrets (the keypair/secret actually depend on the seed).
  - **tamper differs:** using the `ml-kem` API directly, flipping a byte of
    the ciphertext before decapsulation yields a secret *different* from the
    encapsulated one (implicit rejection) — proving the agreement check is
    real.
- **QEMU smoke test** (`tools/test-qemu.ps1`, extended; written first,
  failing): keeps every existing pattern and adds
  `pqc: ML-KEM-768 round-trip ok`.
- **Bare build** (`cargo build -p kernel --target riscv64gc-unknown-none-elf`):
  the integration gate for `no_std`/no-`alloc` (§3.4).

## 5. Deliverables

1. `libs/crypto`: `ml-kem` + `rand_chacha` dependencies; `ml_kem768_agree`
   replacing `PQC_PLANNED`; host unit tests.
2. `kernel`: dependency on `kernel-crypto`; the `kmain` ML-KEM demo + proof
   line.
3. `Cargo.lock` un-ignored and committed.
4. Extended QEMU smoke test + host unit tests, all green.
5. Short learning note `docs/learning/0010-post-quantum-crypto.md`.
6. Roadmap: 3c marked done with date; **Phase 3 (security spine) complete.**
7. Glossary: post-quantum cryptography, KEM, encapsulation/decapsulation,
   ML-KEM, shared secret — only genuinely new terms.

## 6. Open questions (for later phases)

- **Real entropy (Phase 4+):** a kernel CSPRNG seeded from a hardware
  entropy source (or device-tree-provided seed), replacing the fixed demo
  seed everywhere.
- **A crypto capability/service:** exposing KEM (and later signatures) to
  U-mode components behind a capability-checked syscall or IPC service,
  building on 3b.
- **Signatures (ML-DSA)** and **hybrid** (classical + PQC) schemes.
- **Using the shared secret:** deriving keys and wiring an authenticated,
  encrypted channel (e.g. over IPC) or secure boot.
- **Zeroization / constant-time review** of secret material across the
  kernel boundary.
