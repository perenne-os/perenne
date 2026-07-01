# U-mode getrandom service Implementation Plan

> **For agentic workers:** Implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a U-mode component obtain randomness from the kernel entropy pool via a capability-gated `getrandom` syscall.

**Architecture:** The pool moves from the kernel binary into the `arch` crate (so the syscall layer can reach it); a new `getrandom` syscall, serviced in `sched` (alongside `ipc_*`/`restart`), checks the caller holds a `Randomness` capability, then fills 32 bytes from the pool into the return registers. A `rnguser` component proves both refusal (no cap) and service (with cap).

**Tech Stack:** Rust `no_std`, RISC-V (riscv64gc), QEMU. Host tests: `cargo test -p kernel-arch-riscv64`. Bare: `cargo build -p kernel --target riscv64gc-unknown-none-elf`.

**Spec:** `docs/design/specs/2026-06-21-getrandom-service-design.md`

**Existing-code facts (verified):**
- `arch/riscv64/Cargo.toml` has an empty `[dependencies]`. The kernel binary deps `kernel-crypto = { path = "../libs/crypto" }`; from `arch/riscv64/` the path is `../../libs/crypto`. Crate name `kernel-crypto` → imported `kernel_crypto`.
- `kernel_crypto::EntropyPool` has `const fn new()`, `reseed(&mut self,[u8;32])`, `next_seed(&mut self)->[u8;32]`.
- `kernel/src/main.rs` (`bare` module) currently holds `static ENTROPY_POOL: mem::SingleHartCell<kernel_crypto::EntropyPool>` + `pool_reseed`/`pool_next_seed`; `pqc_consumer` calls them. Import line: `use kernel_arch_riscv64::{cap::Capability, console, dt, mem, println, sched, task::Message, timer, trap, virtio};`.
- `cap.rs`: `Capability { Endpoint(EndpointId), Restart(usize) }`; `cap_lookup`/`restart_target` end with `_ => None`.
- `sched.rs`: `restart(frame)` reads `frame.regs[9]` (a0), uses `SCHED.with`, `cap::restart_target(&s.tasks[cur].caps, idx)`, logs via `crate::println!`. `MAX_TASKS = 10`; `Scheduler::new` initializes a 10-element `[None; ..]`.
- `syscall.rs`: `Syscall` ends `Call, Reply, Unknown(usize)`; `decode_syscall` maps 1..8; `dispatch` routes them; `Outcome::Resume`.
- `mem::SingleHartCell` is `pub`, generic, `const fn new`.
- `kmain` spawns the entropy/`pqc` tasks inside `if let Some(rng) = rng_base { ... }`, then `sched::spawn("idle", ...)`.

**Cast after this plan (MAX_TASKS → 12):** rtc(0) client(1) rogue(2) healer(3) transient(4) flaky(5) pqc(6) entropy(7) **rnguser(8)** idle(9). `rnguser` runs after `pqc` (which seeds the pool).

---

## Task 1: move the entropy pool into the `arch` crate

**Files:**
- Modify: `arch/riscv64/Cargo.toml`
- Create: `arch/riscv64/src/entropy.rs`
- Modify: `arch/riscv64/src/lib.rs`
- Modify: `kernel/src/main.rs` (retarget the consumer; remove the local pool)

- [ ] **Step 1: Add the `kernel-crypto` dependency to `arch`**

In `arch/riscv64/Cargo.toml`, under `[dependencies]`:

```toml
[dependencies]
kernel-crypto = { path = "../../libs/crypto" }
```

- [ ] **Step 2: Create `arch/riscv64/src/entropy.rs`**

```rust
//! The kernel entropy pool — a ChaCha20 CSPRNG (`kernel_crypto::EntropyPool`)
//! seeded by the virtio-rng component, serving entropy to kernel code on
//! demand. Held in a `SingleHartCell` alongside the scheduler/frame allocator.
//!
//! It lives in `arch` (not the kernel binary) so the syscall layer
//! (`sched::getrandom`) can reach it — the CSPRNG cannot run in U-mode, so the
//! pool stays kernel-side and U-mode draws from it via a syscall.

use kernel_crypto::EntropyPool;

static POOL: crate::mem::SingleHartCell<EntropyPool> =
    crate::mem::SingleHartCell::new(EntropyPool::new());

/// Fold 32 bytes of device entropy into the pool (seeds on first call, mixes
/// after).
pub fn reseed(bytes: [u8; 32]) {
    POOL.with(|p| p.reseed(bytes));
}

/// Draw a fresh 32-byte seed from the pool (the kernel seeds it first).
pub fn next_seed() -> [u8; 32] {
    POOL.with(|p| p.next_seed())
}
```

- [ ] **Step 3: Declare the module in `lib.rs`**

In `arch/riscv64/src/lib.rs`, add after the `pub mod virtio;` declaration:

```rust
/// The kernel entropy pool: the ChaCha20 CSPRNG (seeded by the virtio-rng
/// component) that backs the `getrandom` syscall. Gated — it holds a static
/// cell and is only used by the bare kernel.
#[cfg(target_arch = "riscv64")]
pub mod entropy;
```

- [ ] **Step 4: Retarget the kernel consumer and remove the local pool**

In `kernel/src/main.rs`, add `entropy` to the arch import:

```rust
    use kernel_arch_riscv64::{cap::Capability, console, dt, entropy, mem, println, sched, task::Message, timer, trap, virtio};
```

Delete the local pool block (the `static ENTROPY_POOL` and the `pool_reseed`/`pool_next_seed` fns):

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

In `pqc_consumer`, change the three calls: `pool_reseed(` → `entropy::reseed(` (both occurrences) and `pool_next_seed()` → `entropy::next_seed()` (both occurrences).

- [ ] **Step 5: Verify host tests + bare build**

Run: `cargo test -p kernel-arch-riscv64` → expect PASS (existing suite).
Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf` → expect SUCCESS.

- [ ] **Step 6: Run the smoke test (behavior unchanged)**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS` — the pool moved but the four `pool` lines (`entropy: pool seeded …` etc.) and everything else are identical to before.

- [ ] **Step 7: Commit**

```bash
git add arch/riscv64/Cargo.toml arch/riscv64/src/entropy.rs arch/riscv64/src/lib.rs kernel/src/main.rs
git commit -m "refactor(entropy): move the entropy pool into arch (so the syscall layer can reach it)"
```

---

## Task 2: the `Randomness` capability + its lookup

**Files:**
- Modify: `arch/riscv64/src/cap.rs`

- [ ] **Step 1: Write the failing test**

In `arch/riscv64/src/cap.rs`, add to the `tests` module:

```rust
    #[test]
    fn has_randomness_checks_the_slot() {
        let caps = [None, Some(Capability::Randomness), Some(Capability::Endpoint(0))];
        assert!(has_randomness(&caps, 1));
        assert!(!has_randomness(&caps, 2), "an Endpoint cap is not Randomness");
        assert!(!has_randomness(&caps, 0), "empty slot");
        assert!(!has_randomness(&caps, 9), "out of range");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p kernel-arch-riscv64 cap::`
Expected: FAIL — `Capability::Randomness` and `has_randomness` do not exist.

- [ ] **Step 3: Add the variant and the lookup**

In `arch/riscv64/src/cap.rs`, extend the `Capability` enum:

```rust
    /// Authority to `restart` the task in this scheduler slot (Phase 5b).
    Restart(usize),
    /// Authority to call `getrandom` — draw from the kernel entropy pool.
    Randomness,
}
```

Add, after `restart_target`:

```rust
/// True iff capability `idx` is a `Randomness` capability (the authority to
/// draw from the kernel entropy pool). `false` for an empty slot, an
/// out-of-range index, or the wrong capability type.
pub fn has_randomness(caps: &[Option<Capability>], idx: usize) -> bool {
    matches!(caps.get(idx), Some(Some(Capability::Randomness)))
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p kernel-arch-riscv64 cap::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/cap.rs
git commit -m "feat(cap): Randomness capability + has_randomness lookup (host-tested)"
```

---

## Task 3: the `getrandom` service in `sched`

**Files:**
- Modify: `arch/riscv64/src/sched.rs`

- [ ] **Step 1: Add `getrandom` after `restart`**

In `arch/riscv64/src/sched.rs`, add after the `restart` function:

```rust
/// Service a `getrandom` syscall: if the caller holds a `Randomness`
/// capability at index `a0` (= `frame.regs[9]`), fill `a1..a4` with 32 fresh
/// bytes from the kernel entropy pool and set `a0 = 0`; otherwise set `a0 =
/// usize::MAX` and return no bytes. Every outcome is logged with the caller's
/// name.
#[cfg(target_arch = "riscv64")]
pub fn getrandom(frame: &mut crate::trap::TrapFrame) {
    let cap_idx = frame.regs[9];
    let ok = SCHED.with(|s| {
        let t = s.tasks[s.current].as_ref().unwrap();
        if crate::cap::has_randomness(&t.caps, cap_idx) {
            crate::println!("rng: served 32 bytes to '{}'", t.name);
            true
        } else {
            crate::println!("rng: request rejected (no capability)");
            false
        }
    });
    if !ok {
        frame.regs[9] = usize::MAX;
        return;
    }
    let bytes = crate::entropy::next_seed();
    let word = |i: usize| {
        let mut c = [0u8; 8];
        c.copy_from_slice(&bytes[i..i + 8]);
        u64::from_le_bytes(c) as usize
    };
    frame.regs[9] = 0; // a0 = ok
    frame.regs[10] = word(0); // a1
    frame.regs[11] = word(8); // a2
    frame.regs[12] = word(16); // a3
    frame.regs[13] = word(24); // a4
}
```

- [ ] **Step 2: Verify host tests + bare build**

Run: `cargo test -p kernel-arch-riscv64` → expect PASS (gated code excluded on host).
Run: `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf` → expect SUCCESS.

- [ ] **Step 3: Commit**

```bash
git add arch/riscv64/src/sched.rs
git commit -m "feat(sched): getrandom service - capability-checked draw from the entropy pool"
```

---

## Task 4: the `getrandom` syscall number (decode + dispatch)

**Files:**
- Modify: `arch/riscv64/src/syscall.rs`

- [ ] **Step 1: Write the failing test**

In `arch/riscv64/src/syscall.rs`, add to the `tests` module:

```rust
    #[test]
    fn decodes_getrandom_syscall() {
        assert_eq!(decode_syscall(9), Syscall::Getrandom);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p kernel-arch-riscv64 syscall::`
Expected: FAIL — `Syscall::Getrandom` does not exist.

- [ ] **Step 3: Add the variant, decode, and dispatch**

In `arch/riscv64/src/syscall.rs`, add to `Syscall` (after `Reply`):

```rust
    /// `getrandom(cap)` — draw 32 bytes from the kernel entropy pool.
    Getrandom,
```

Add the decode arm (after `8 => Syscall::Reply,`):

```rust
        9 => Syscall::Getrandom,
```

Add the dispatch arm (after the `Syscall::Reply` arm):

```rust
        Syscall::Getrandom => {
            crate::sched::getrandom(frame);
            Outcome::Resume
        }
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p kernel-arch-riscv64 syscall::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/syscall.rs
git commit -m "feat(syscall): getrandom syscall (a7=9) decode + dispatch"
```

---

## Task 5: update the smoke test for getrandom (red)

**Files:**
- Modify: `tools/test-qemu.ps1`

- [ ] **Step 1: Add the getrandom assertions**

In `tools/test-qemu.ps1`, add to `$mustMatch` (after the `pqc: ML-KEM-768 round-trip ok \(pool-seeded\)` line):

```powershell
    "rng: request rejected \(no capability\)",
    "rng: served 32 bytes to 'rnguser'",
    "sched: task 'rnguser' exited \(code 0\)",
```

Update the header comment + PASS message to note a U-mode component now draws from the entropy pool through a capability-gated `getrandom` syscall (refused without the capability, served with it).

- [ ] **Step 2: Run the smoke test — expect RED**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: FAIL — the three `rng:`/`rnguser` lines are absent (no `rnguser` yet).

- [ ] **Step 3: Commit (red test pinned)**

```bash
git add tools/test-qemu.ps1
git commit -m "test: smoke test asserts capability-gated U-mode getrandom (red)"
```

---

## Task 6: the `rnguser` component + wiring (green)

**Files:**
- Modify: `arch/riscv64/src/sched.rs` (`MAX_TASKS`)
- Modify: `kernel/src/main.rs`

- [ ] **Step 1: Bump `MAX_TASKS` to 12**

In `arch/riscv64/src/sched.rs`, update the constant and doc:

```rust
/// Maximum concurrent tasks: the demo runs ten (rtc, client, rogue, healer,
/// transient, flaky, pqc, entropy, rnguser, idle) plus headroom.
pub const MAX_TASKS: usize = 12;
```

And extend `Scheduler::new`'s initializer to twelve `None`s:

```rust
        Self { tasks: [None, None, None, None, None, None, None, None, None, None, None, None], current: 0 }
```

- [ ] **Step 2: Add the `getrandom` syscall wrapper**

In `kernel/src/main.rs`, add after `sys_reply`:

```rust
    /// getrandom syscall (a7 = 9): a0 = cap index of a Randomness capability.
    /// Returns (status, 4 words): status a0 = 0 on success (a1..a4 = 32 random
    /// bytes) or `usize::MAX` if the caller lacks the capability.
    ///
    /// # Safety
    /// Always sound; the kernel validates the capability and fills the words.
    #[inline(always)]
    unsafe fn sys_getrandom(cap: usize) -> (usize, [usize; 4]) {
        let status;
        let w0;
        let w1;
        let w2;
        let w3;
        core::arch::asm!(
            "ecall",
            in("a7") 9usize,
            inout("a0") cap => status,
            out("a1") w0,
            out("a2") w1,
            out("a3") w2,
            out("a4") w3,
            options(nostack),
        );
        (status, [w0, w1, w2, w3])
    }
```

- [ ] **Step 3: Add the `RNG_CAP` constant and the stacks**

In `kernel/src/main.rs`, after the `ENTROPY_CAP` constant, add:

```rust
    /// The `rnguser` component's cap slot holding its `Randomness` capability.
    const RNG_CAP: usize = 0;
```

Add a kernel + user stack (after the entropy stacks `KS_ENTROPY` / `US_ENTROPY`):

```rust
    static mut KS_RNGUSER: KStack = [0; TASK_STACK];
```

and:

```rust
    #[link_section = ".user_data"]
    static mut US_RNGUSER: UStack = UStack([0; USER_STACK_SIZE]);
```

- [ ] **Step 4: Add the `rnguser` component**

In `kernel/src/main.rs`, add after `entropy_component` (or near the other U-mode tasks):

```rust
    /// A U-mode component that draws from the kernel entropy pool via the
    /// capability-gated `getrandom` syscall. It proves both gating outcomes:
    /// a request with no capability (cap slot 99) is refused, and a request
    /// with its granted `Randomness` capability is served. It draws twice and
    /// checks the two 32-byte results differ (a live pool), then exits 0.
    /// Register-only (no `.rodata`/buffer) — codegen-safe in U-mode.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn rnguser_task() -> ! {
        // SAFETY: getrandom is always sound; the kernel checks the capability.
        unsafe {
            let (bad, _) = sys_getrandom(99); // no capability at slot 99
            let (ok1, a) = sys_getrandom(RNG_CAP);
            let (ok2, b) = sys_getrandom(RNG_CAP);
            let differ = a[0] != b[0] || a[1] != b[1] || a[2] != b[2] || a[3] != b[3];
            let good = bad == usize::MAX && ok1 == 0 && ok2 == 0 && differ;
            sys_exit(if good { 0 } else { 7 })
        }
    }
```

- [ ] **Step 5: Spawn `rnguser` and grant its capability**

In `kernel/src/main.rs` `kmain`, insert just before the `idle` spawn (after the `if let Some(rng) = rng_base { ... }` block):

```rust
        // A U-mode component that draws from the entropy pool via the
        // capability-gated getrandom syscall (it runs after pqc has seeded the
        // pool). It holds a Randomness capability; a request without one is
        // refused.
        let nu = ustack(core::ptr::addr_of!(US_RNGUSER) as usize);
        let rnguser = sched::spawn_user("rnguser", rnguser_task, nu.1,
            core::ptr::addr_of!(KS_RNGUSER) as usize + TASK_STACK,
            mem::build_user_space(nu, NO_DEVICE));
        sched::grant_cap(rnguser, RNG_CAP, Capability::Randomness);
```

- [ ] **Step 6: Build the kernel**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: SUCCESS.

- [ ] **Step 7: Run the smoke test — expect GREEN**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS`, including `rng: request rejected (no capability)`, `rng: served 32 bytes to 'rnguser'`, and `sched: task 'rnguser' exited (code 0)`, with all prior milestones present.

Troubleshooting (diagnose, don't weaken the test):
- `rnguser` exits 7 → either the cap check is wrong (served path not taken — confirm `RNG_CAP` matches the grant slot) or the two draws didn't differ (the pool wasn't seeded before `rnguser` ran — confirm `rnguser` is spawned after the `pqc`/entropy block).
- No `rng:` lines → `getrandom` not dispatched: confirm `a7=9` decode + dispatch to `sched::getrandom`.

- [ ] **Step 8: Commit**

```bash
git add arch/riscv64/src/sched.rs kernel/src/main.rs
git commit -m "feat: rnguser component - capability-gated U-mode entropy from the kernel pool"
```

---

## Task 7: docs — learning note, roadmap, glossary; final verification

**Files:**
- Create: `docs/learning/0019-getrandom-service.md`
- Modify: `docs/roadmap/roadmap.md`
- Modify: `docs/glossary.md`
- Modify: `docs/learning/README.md`

- [ ] **Step 1: Write a short learning note**

Create `docs/learning/0019-getrandom-service.md`:

```markdown
# 0019 — getrandom: capability-gated entropy for U-mode

**One-line:** U-mode components can now draw from the kernel entropy pool
through a capability-gated `getrandom` syscall — because the CSPRNG can't run
unprivileged, a syscall (not a server) is the right interface.

## What changed
- The entropy pool moved from the kernel binary into the `arch` crate (with the
  scheduler/frame-allocator singletons) so the syscall layer can reach it.
- New `Capability::Randomness` + `has_randomness`; a `getrandom` syscall
  (`a7=9`) serviced in `sched`: it checks the caller holds the capability, then
  fills `a1..a4` with 32 fresh pool bytes (`a0=0`), else refuses (`a0=MAX`).
- A `rnguser` component proves both: refused without a capability, served with
  one, and the two draws differ (a live pool).

## The ideas worth keeping
1. **A kernel-owned generator is exposed as a syscall, not a server.** The
   CSPRNG uses `core`/`.rodata` and can't run in U-mode, so no user-space "rng
   server" could hold it — it would only forward to this syscall. (Linux's
   `getrandom(2)` is a syscall for the same reason.)
2. **A capability can gate a non-IPC syscall.** Until now capabilities gated
   IPC (an endpoint index) and restart (a target). `getrandom` shows the same
   unforgeable-index check on an ordinary syscall: the caller names a
   `Randomness` capability it was granted; holders are served, others refused.
3. **Where state lives follows who consumes it.** Once a *syscall* consumes the
   pool, the pool belongs in `arch` (the syscall layer), not the kernel binary
   — even though that means `arch` now depends on `kernel_crypto`.

## Proof
`rng: request rejected (no capability)` then `rng: served 32 bytes to
'rnguser'`, and `rnguser` exits 0 (its two draws differed) — capability-gated,
live entropy in user space.
```

- [ ] **Step 2: Update the roadmap**

In `docs/roadmap/roadmap.md`, find the "(Next candidates: …)" note after the "Kernel entropy pool" block (it lists "a capability-gated U-mode `getrandom` service" first). Replace that parenthetical with:

```markdown
### U-mode getrandom service  *(done — 2026-06-21)*

- **Goal:** let U-mode components draw from the kernel entropy pool, gated by a
  capability.
- **You learn:** a kernel-owned CSPRNG is exposed to user space as a syscall
  (it can't run unprivileged), and a capability can gate an ordinary syscall —
  the same unforgeable-index check IPC uses (see
  [learning note 0019](../learning/0019-getrandom-service.md)).
- **Done when:** `./tools/test-qemu.ps1` shows a component refused without the
  capability and served with it (two draws differ). QEMU-only.

(Next candidates: an interrupt-driven (PLIC) device path; one-shot reply
capabilities for deferred/forwarded replies; richer self-healing once a
filesystem exists.)
```

- [ ] **Step 3: Add a glossary entry**

In `docs/glossary.md`, add after the "Entropy pool" entry:

```markdown
- **getrandom** — the syscall a U-mode component uses to draw bytes from the kernel entropy pool. Capability-gated (the caller must hold a `Randomness` capability) — a kernel-owned generator is exposed as a syscall because the CSPRNG can't run unprivileged.
```

- [ ] **Step 4: Update the learning-notes index**

In `docs/learning/README.md`, add after the 0018 line:

```markdown
- [0019 — getrandom: capability-gated entropy for U-mode](0019-getrandom-service.md)
```

- [ ] **Step 5: Final verification (run all, paste results)**

Run: `cargo test -p kernel-arch-riscv64` → expect all green.
Run: `cargo test -p kernel-crypto` → expect all green.
Run: `cargo build --workspace` → expect success.
Run (PowerShell): `./tools/check-references.ps1` → expect PASS.
Run (PowerShell): `./tools/test-qemu.ps1` → expect `BOOT TEST PASS`.

- [ ] **Step 6: Commit**

```bash
git add docs/learning/0019-getrandom-service.md docs/roadmap/roadmap.md docs/glossary.md docs/learning/README.md
git commit -m "docs: U-mode getrandom service - learning note, roadmap, glossary"
```

---

## Done-when checklist (maps to spec §1)

- [ ] **Capability-gated randomness** — smoke shows `rng: request rejected (no capability)`, `rng: served 32 bytes to 'rnguser'`, and `sched: task 'rnguser' exited (code 0)`.
- [ ] **Host tests** — `cap::has_randomness` (hit/wrong-type/empty/oob) and `decode_syscall(9) == Getrandom`.
- [ ] `check-references` clean; `cargo build --workspace` green; `BOOT TEST PASS`.
```
