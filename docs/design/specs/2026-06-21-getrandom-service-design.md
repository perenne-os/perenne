# Kernel — Design: a capability-gated U-mode getrandom service

- **Date:** 2026-06-21
- **Status:** Draft — awaiting review
- **Scope of this document:** let U-mode components obtain randomness from the
  kernel entropy pool, via a capability-gated `getrandom` syscall (the pool's
  CSPRNG must stay kernel-side). Fully QEMU-testable.

---

## 0. Where this sits

The kernel has a reseedable entropy pool (a ChaCha20 CSPRNG) seeded by the
virtio-rng component, but only **kernel** code (ML-KEM) can draw from it — no
U-mode component can get randomness. This adds that, the roadmap's named next
candidate.

**Key constraint:** the CSPRNG cannot run in U-mode (it uses `core` routines
and `.rodata` tables — the same wall that keeps ML-KEM and complex driver code
out of U-mode). So the pool must stay kernel-side, and the U-mode interface is
a **syscall** (as in Linux's `getrandom(2)`), not a user-space "rng server"
(which could only proxy to that syscall). Access is **capability-gated** per
the project ethos: a component must hold a `Randomness` capability.

It builds on: the entropy pool (`kernel_crypto::EntropyPool`), the capability
model (`cap.rs`), and the syscall surface (`syscall.rs`).

## 1. Goal

A U-mode component that holds a `Randomness` capability can call `getrandom`
and receive 32 fresh bytes from the kernel entropy pool; a component without
the capability is refused. The pool moves into the `arch` crate so the syscall
layer can reach it; kernel-side seeding/use (the `pqc` consumer, ML-KEM) is
unchanged in behavior.

**You learn (kept brief):** why a kernel-owned CSPRNG is exposed to user space
as a *syscall* rather than a server (the generator can't run unprivileged), and
how a capability gates a syscall (the caller names a `Randomness` capability it
was granted, exactly as IPC names an endpoint) — the same unforgeable-index
check, now on a non-IPC syscall.

**Done when** `./tools/test-qemu.ps1` observes, alongside every existing
milestone:

1. **Capability-gated randomness** — a `rnguser` U-mode component is refused
   when it asks without a capability (`rng: request rejected (no capability)`)
   and served when it asks with its granted capability (`rng: served 32 bytes
   to 'rnguser'`); it confirms two draws differ (live) and exits cleanly
   (`sched: task 'rnguser' exited (code 0)`).

And off the bare target:

2. **Host unit tests** — `has_randomness` (hit / wrong-type / empty /
   out-of-range) and `decode_syscall(9) == Getrandom`.

## 2. Non-goals (deferred)

- **Variable-length output / a user buffer** — `getrandom` returns a fixed
  32 bytes in registers (one seed/nonce); a buffer interface (with the
  confused-deputy guard + a SUM window) is future work nothing needs yet.
- **A user-space "rng server" component** — impossible to make meaningful: the
  CSPRNG can't run in U-mode, so any server would just forward to this syscall.
- **Per-call entropy accounting / blocking until seeded** — `getrandom` always
  serves from the pool; if drawn before the pool is seeded it returns zeros (a
  kernel-config ordering issue, documented, not a runtime error).
- **Changing the pool's CSPRNG, the entropy component, or the virtio driver** —
  only the pool's *home* (kernel binary → `arch`) and the new syscall are added.

## 3. Design

### 3.1 Components

| Component | Location | Responsibility |
|-----------|----------|----------------|
| Pool home | `arch/riscv64/src/entropy.rs` *(new)* | The `SingleHartCell<EntropyPool>` static (moved from the kernel binary) + `reseed`/`next_seed` accessors. `arch` gains a `kernel-crypto` dependency. |
| `getrandom` service | `arch/riscv64/src/sched.rs` | `getrandom(frame)`: cap-check the current task + log + fill registers from `entropy::next_seed()`. Lives with `ipc_*`/`restart` (which already access the current task's caps and name via `SCHED`). |
| `Capability::Randomness` + `has_randomness` | `arch/riscv64/src/cap.rs` | New cap variant; pure `has_randomness(caps, idx) -> bool` (host-tested), mirroring `cap_lookup`/`restart_target`. |
| `Syscall::Getrandom` + decode/dispatch | `arch/riscv64/src/syscall.rs` | New syscall number (`a7 = 9`); pure decode (host-tested) + dispatch to `sched::getrandom`. |
| `rnguser` component + kernel wiring | `kernel/src/main.rs` | A U-mode component holding a `Randomness` cap that exercises refused + served; `kmain` grants the cap; the `pqc` consumer's pool calls retarget to `entropy::*`. |

### 3.2 Moving the pool into `arch`

`arch/riscv64/Cargo.toml` adds `kernel-crypto = { path = "../../libs/crypto" }`.
A new `arch/riscv64/src/entropy.rs` (declared `pub mod entropy;` in `lib.rs`,
gated `#[cfg(target_arch = "riscv64")]` like the other singleton-holding
modules) holds:

```
static POOL: SingleHartCell<EntropyPool> = SingleHartCell::new(EntropyPool::new());
pub fn reseed(bytes: [u8; 32]) { POOL.with(|p| p.reseed(bytes)); }
pub fn next_seed() -> [u8; 32] { POOL.with(|p| p.next_seed()) }
```

The kernel binary's `pqc_consumer` calls `entropy::reseed` / `entropy::next_seed`
in place of the local `pool_reseed` / `pool_next_seed` (which, with their
`static ENTROPY_POOL`, are removed from the kernel binary). Behavior is
identical; the pool simply lives with the scheduler/frame-allocator singletons
now that a syscall consumes it.

### 3.3 The capability

`cap.rs` gains:

```
pub enum Capability {
    Endpoint(EndpointId),
    Restart(usize),
    Randomness,          // authority to call getrandom
}

/// True iff capability `idx` is a Randomness capability.
pub fn has_randomness(caps: &[Option<Capability>], idx: usize) -> bool {
    matches!(caps.get(idx), Some(Some(Capability::Randomness)))
}
```

`Randomness` is a pure authority token (no parameter) — holding it at a cap
slot is the permission to draw from the pool. `cap_lookup`/`restart_target`
already fall through to `None`/`false` for the new variant.

### 3.4 The syscall

New `getrandom` syscall (`a7 = 9`); ABI: `a0` = cap index. On success
(`has_randomness` true): `a0 = 0` and `a1..a4` = 32 random bytes (four 64-bit
words from `entropy::next_seed()`). On failure (no/wrong cap): `a0 = usize::MAX`
and no bytes. The kernel logs the outcome — it has the caller's name — using
the scheduler's current task:

- served: `rng: served 32 bytes to '<name>'`
- refused: `rng: request rejected (no capability)`

`sched::getrandom(frame)` reads `frame.regs[9]` (a0 = cap index); inside
`SCHED.with` it checks `cap::has_randomness(&current.caps, idx)`. On success it
fills `frame.regs[9] = 0` and `frame.regs[10..14]` (a1..a4) with the four words
from `crate::entropy::next_seed()`, and logs `rng: served 32 bytes to '<name>'`;
on failure it sets `frame.regs[9] = usize::MAX` and logs the rejection. (It
lives in `sched` because, like `ipc_*`/`restart`, it needs the current task's
caps and name — which `SCHED` holds.) `syscall::dispatch` routes
`Syscall::Getrandom` to it and returns `Outcome::Resume` (the handler advances
`sepc`).

### 3.5 The demo (`rnguser`)

One new U-mode component proves both gating outcomes itself:

```
let bad  = sys_getrandom(99);        // no cap at slot 99 -> a0 == usize::MAX
let (ok1, w) = sys_getrandom(RNG_CAP); // holds the cap -> ok, 32 bytes
let (ok2, w2) = sys_getrandom(RNG_CAP);
let live = ok1 && ok2 && w != w2;    // two draws differ -> live pool
sys_exit(if bad == usize::MAX && live { 0 } else { 7 });
```

`kmain` spawns `rnguser` (holding `Capability::Randomness` at `RNG_CAP`) after
the entropy/`pqc` tasks (so the pool is seeded first), grants the cap, and the
kernel logs the refused + served lines. `MAX_TASKS` rises 10 → 12 (room for
`rnguser` + headroom). The component reads the returned words from registers
(via the syscall wrapper) — no `.rodata`/buffer, codegen-safe.

### 3.6 Error handling summary

| Situation | Behavior |
|-----------|----------|
| `getrandom` with a `Randomness` cap | `a0 = 0`, `a1..a4` = 32 fresh bytes; logged "served". |
| `getrandom` without/with wrong cap | `a0 = usize::MAX`, no bytes; logged "rejected". |
| `getrandom` before the pool is seeded | Pool yields zeros (ordering: `rnguser` runs after the `pqc` consumer seeds it). |
| Plain syscalls (print/exit/…) | Unchanged. |

## 4. Testing

- **Host unit tests** (`arch/riscv64`, `cargo test`):
  - `cap::has_randomness` — `true` for a `Randomness` cap at the index; `false`
    for an `Endpoint`/`Restart` cap (wrong type), an empty slot, and an
    out-of-range index.
  - `syscall::decode_syscall(9) == Syscall::Getrandom` (and existing numbers
    still decode).
- **QEMU smoke test** (`tools/test-qemu.ps1`, extended; written first,
  failing): add `rng: request rejected (no capability)`, `rng: served 32 bytes
  to 'rnguser'`, and `sched: task 'rnguser' exited \(code 0\)`; keep every
  other milestone.

## 5. Deliverables

1. `arch/riscv64/Cargo.toml` + `lib.rs`: the `kernel-crypto` dep and
   `pub mod entropy;`.
2. `arch/riscv64/src/entropy.rs` (new): the pool static + `reseed`/`next_seed`
   accessors.
3. `arch/riscv64/src/cap.rs`: `Capability::Randomness` + `has_randomness` +
   host tests.
4. `arch/riscv64/src/sched.rs`: the `getrandom(frame)` service.
5. `arch/riscv64/src/syscall.rs`: `Syscall::Getrandom` (`a7=9`) decode +
   dispatch, host test.
6. `kernel/src/main.rs`: remove the local pool static/accessors (retarget the
   `pqc` consumer to `entropy::*`); add `sys_getrandom` wrapper, the `rnguser`
   component + its stacks, the grant, and `MAX_TASKS` 10 → 12.
7. Extended QEMU smoke test + host tests, all green.
8. Short learning note `docs/learning/0019-getrandom-service.md`.
9. Roadmap: getrandom marked done (the "next candidate" note updated).
10. Glossary: only genuinely new terms (e.g. *getrandom*).

## 6. Open questions (for later phases)

- **A variable-length buffer interface** (`getrandom(cap, ptr, len)`) with the
  confused-deputy guard + SUM window, once a component needs more than 32 bytes.
- **Blocking-until-seeded semantics** / an entropy estimate gating first use.
- **Per-component reseed isolation** or rate limiting, if randomness becomes a
  contended resource.
