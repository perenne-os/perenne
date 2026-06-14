# Phase 3a — Privilege Drop to User Space Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Drop the CPU to U-mode for the first time, run an unprivileged task that reaches the kernel only through `ecall` syscalls (`print`/`exit`), and prove the privilege boundary enforces by containing a U-mode task that touches kernel memory — all proven live in QEMU alongside the 2a/2b/2c milestones.

**Architecture:** The user episode runs **before** the 2c scheduler demo and is bounded: `kmain` calls `enter_user`, which `sret`s into U-mode and *returns* to `kmain` when the user task exits or is killed. The 2c scheduler (spawn A/B/C, `enter()` forever) stays the last thing `kmain` does and is left untouched. A single embedded user "program" lives in dedicated `.user_text` (R-X-U) and `.user_data` (RW-U) linker sections; everything kernel stays non-`U`, so a U-mode access to it faults. The trap entry gains an `sscratch`-based stack swap (kernel trap stack on a trap from U-mode; unchanged behavior on a trap from S-mode, where `sscratch == 0` is the sentinel). The `print` syscall validates the user pointer against the `.user_data` bounds (confused-deputy guard), then opens a brief `sstatus.SUM` window to copy the bytes.

**Tech Stack:** Rust `no_std`, RISC-V (riscv64gc), QEMU virt + OpenSBI, PowerShell test scripts.

**Spec:** `docs/superpowers/specs/2026-06-14-phase-3a-user-mode-design.md`

**Conventions reminder:** commits use the house style (`feat(arch):`, `test:`, `docs:`), no Claude co-author line, identity Kathir <kathirpsmy@gmail.com>. Host tests: `cargo test` (whole workspace, runs on the Windows host). Cross-build: `cargo build -p kernel --target riscv64gc-unknown-none-elf`. Integration: `./tools/test-qemu.ps1`. Reference check: `./tools/check-references.ps1`.

**Key invariants to preserve:**
- `sscratch == 0` means "the kernel is running on a kernel stack" (the 2c S-mode trap path depends on this). It is set to the trap-stack top only while a user task runs, and reset to 0 the instant the kernel resumes (during trap handling, and on the way back to `kmain`).
- The 2c S-mode trap path (`ebreak`, the W^X probe, timer preemption) must behave exactly as before — the smoke test's 2a/2b/2c patterns are the regression guard.
- The kernel reads user memory **only** inside a validated `SUM` window in the `print` syscall; `SUM` is 0 everywhere else.
- A trap's origin privilege is read from the saved `sstatus.SPP` (bit 8): 0 = trapped from U-mode, 1 = from S-mode. A U-mode page fault is fatal-to-the-task (contained); an S-mode store fault may be the W^X probe (skipped).
- The 3a user task is **standalone** (never a run-queue slot), so termination is reported via `ExitReason`; `TaskState` is unchanged (no `Exited` variant until 3b).

---

### Task 1: Failing QEMU smoke test (test-first)

**Files:**
- Modify: `tools/test-qemu.ps1`

- [ ] **Step 1: Update the header comment**

In `tools/test-qemu.ps1`, replace the header comment (lines 1–7) with:

```powershell
# Boot smoke test: cross-builds the kernel, boots it headless under QEMU
# (riscv64 virt + OpenSBI), captures the serial console, and asserts the
# Phase 2a milestones (greeting, survived breakpoint, >= 2 timer ticks),
# the Phase 2b milestones (Sv39 paging on, W^X blocking a rodata write, a
# frame alloc/free round-trip), the Phase 2c milestones (three tasks
# round-robin cooperatively, then a non-yielding task is preempted), and
# the Phase 3a milestones (a U-mode task makes a print syscall, exits
# cleanly, and a second U-mode task touching kernel memory is contained).
# Usage: ./tools/test-qemu.ps1     (exit code 0 = pass, 1 = fail)
```

- [ ] **Step 2: Add the Phase 3a patterns**

In the `$mustMatch` array, add the three 3a patterns after the
`"frames: alloc/free ok"` line (keep every existing pattern). The array
becomes:

```powershell
$mustMatch = @(
    "hello world",
    "trap: breakpoint",
    "survived breakpoint",
    "paging: sv39 on",
    "wx: rodata write blocked",
    "frames: alloc/free ok",
    "user: hello from user mode",
    "user: task exited with code 0",
    "user: task killed by load page fault",
    "(?s)sched: A step 0.*sched: B step 0.*sched: C step 0.*sched: A step 1.*sched: B step 1.*sched: C step 1",
    "preempted the hog",
    "tick: 2(?!\d)"
)
```

- [ ] **Step 3: Update the PASS message**

Replace the PASS line:

```powershell
    Write-Host "BOOT TEST PASS: 2a + 2b + 2c milestones plus the U-mode print round-trip, clean exit, and contained fault all observed." -ForegroundColor Green
```

- [ ] **Step 4: Run the test to verify it fails on exactly the new patterns**

Run: `./tools/test-qemu.ps1`
Expected: `BOOT TEST FAIL: missing within 30s:` listing the three new
`user: ...` patterns. Every 2a/2b/2c pattern is still observed, proving
the current kernel still boots and schedules.

- [ ] **Step 5: Commit**

```powershell
git add tools/test-qemu.ps1
git commit -m "test: extend QEMU smoke test for Phase 3a user-mode milestones (failing)"
```

---

### Task 2: PTE `U` flag + user buffer validation — pure (TDD)

**Files:**
- Modify: `arch/riscv64/src/mem/paging.rs`
- Create: `arch/riscv64/src/syscall.rs`
- Modify: `arch/riscv64/src/lib.rs`

- [ ] **Step 1: Add the `PTE_U` flag and a test that `pte_for` carries it**

In `arch/riscv64/src/mem/paging.rs`, replace the commented-out U line:

```rust
// 1 << 4 is U (user-accessible) — deliberately absent until Phase 3.
```

with the real constant:

```rust
pub const PTE_U: u64 = 1 << 4; // user-accessible (Phase 3a)
```

Then add a test inside the existing `mod tests` block (after
`pte_for_sets_valid_and_keeps_flags`):

```rust
    #[test]
    fn pte_for_carries_the_user_bit() {
        let pte = pte_for(0x8030_0000, PTE_R | PTE_X | PTE_U);
        assert!(pte_is_valid(pte));
        assert_ne!(pte & PTE_U, 0);
        assert_ne!(pte & PTE_X, 0);
    }
```

- [ ] **Step 2: Run the paging tests to verify they pass**

Run: `cargo test -p kernel-arch-riscv64 paging`
Expected: PASS (the new `pte_for_carries_the_user_bit` plus the existing
paging tests). This is a pure constant + flag-OR, so it passes at once.

- [ ] **Step 3: Declare the `syscall` module**

In `arch/riscv64/src/lib.rs`, after the `pub mod sched;` block, add:

```rust
/// System calls: the U-mode → kernel entry surface. Pure decoding and
/// the confused-deputy pointer guard are host-tested here; the gated
/// dispatcher (which reads user memory and writes the console) lives
/// inside.
pub mod syscall;
```

- [ ] **Step 4: Write the failing pure-core tests**

Create `arch/riscv64/src/syscall.rs` with the pure core; the two function
bodies are `todo!()` for now:

```rust
//! System calls — the only way a U-mode task reaches the kernel.
//!
//! A user task executes `ecall`, which traps as `scause = 8` ("environment
//! call from U-mode"). The ABI: `a7` = syscall number, `a0..` = arguments,
//! and the return value goes back in `a0`. Two calls exist in Phase 3a:
//! `print` (1) and `exit` (2).
//!
//! Pure here (host-testable): decoding the syscall number and the
//! confused-deputy guard that validates a user-supplied buffer lies inside
//! the task's own memory. The gated dispatcher below reads user memory
//! (inside a `SUM` window) and writes the console.

/// A decoded syscall request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syscall {
    /// `print(ptr, len)` — write `len` bytes at `ptr` to the console.
    Print,
    /// `exit(code)` — terminate the calling task.
    Exit,
    /// An unrecognized syscall number (a user bug, not a kernel bug).
    Unknown(usize),
}

/// Map a raw `a7` syscall number to a [`Syscall`].
pub fn decode_syscall(a7: usize) -> Syscall {
    todo!()
}

/// The confused-deputy guard: is `[ptr, ptr + len)` fully inside the
/// half-open user region `[lo, hi)`?
///
/// Rejects (returns `false`) on: a pointer below `lo`, an end past `hi`,
/// and `ptr + len` overflowing `usize` (a wrap that could otherwise slip
/// a kernel address past a naive `end <= hi` check). A zero-length buffer
/// at a valid `ptr` is accepted.
pub fn validate_user_buffer(lo: usize, hi: usize, ptr: usize, len: usize) -> bool {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_known_syscalls() {
        assert_eq!(decode_syscall(1), Syscall::Print);
        assert_eq!(decode_syscall(2), Syscall::Exit);
    }

    #[test]
    fn decodes_unknown_syscall() {
        assert_eq!(decode_syscall(99), Syscall::Unknown(99));
    }

    #[test]
    fn accepts_a_buffer_inside_the_region() {
        // region [0x1000, 0x2000); buffer [0x1100, 0x1110) fits.
        assert!(validate_user_buffer(0x1000, 0x2000, 0x1100, 0x10));
    }

    #[test]
    fn accepts_a_buffer_flush_against_the_end() {
        // ends exactly at hi (half-open: 0x1ff0 + 0x10 == 0x2000).
        assert!(validate_user_buffer(0x1000, 0x2000, 0x1ff0, 0x10));
    }

    #[test]
    fn rejects_a_buffer_starting_below_the_region() {
        assert!(!validate_user_buffer(0x1000, 0x2000, 0x0ff0, 0x10));
    }

    #[test]
    fn rejects_a_buffer_overrunning_the_end() {
        // ends at 0x2001, one past hi.
        assert!(!validate_user_buffer(0x1000, 0x2000, 0x1ff1, 0x10));
    }

    #[test]
    fn rejects_a_length_that_wraps_usize() {
        // ptr + len overflows; a naive end-check could wrap below hi.
        assert!(!validate_user_buffer(0x1000, 0x2000, 0x1100, usize::MAX));
    }

    #[test]
    fn accepts_a_zero_length_buffer_at_a_valid_pointer() {
        assert!(validate_user_buffer(0x1000, 0x2000, 0x1100, 0));
    }
}
```

- [ ] **Step 5: Run the tests to verify they fail**

Run: `cargo test -p kernel-arch-riscv64 syscall`
Expected: FAIL — every test hits a `todo!()`.

- [ ] **Step 6: Implement the two pure functions**

Replace the `decode_syscall` body:

```rust
pub fn decode_syscall(a7: usize) -> Syscall {
    match a7 {
        1 => Syscall::Print,
        2 => Syscall::Exit,
        n => Syscall::Unknown(n),
    }
}
```

Replace the `validate_user_buffer` body:

```rust
pub fn validate_user_buffer(lo: usize, hi: usize, ptr: usize, len: usize) -> bool {
    match ptr.checked_add(len) {
        Some(end) => ptr >= lo && end <= hi,
        None => false, // ptr + len wrapped — reject outright
    }
}
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p kernel-arch-riscv64 syscall`
Expected: PASS (all seven).

- [ ] **Step 8: Cross-build and commit**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: success (the gated dispatcher is added in Task 7; unused-warnings
acceptable).

```powershell
git add arch/riscv64/src/mem/paging.rs arch/riscv64/src/syscall.rs arch/riscv64/src/lib.rs
git commit -m "feat(arch): PTE U-bit and the syscall decode + buffer-validation pure core"
```

---

### Task 3: CSR accessors for the privilege transition (gated)

**Files:**
- Modify: `arch/riscv64/src/csr.rs`

These all compile into the kernel binary; they are unused until later
tasks, so verification is the cross-build.

- [ ] **Step 1: Add `sstatus_read`, `sscratch_write`, and the SUM window helpers**

Append to `arch/riscv64/src/csr.rs`:

```rust
/// Read the whole `sstatus` register. The user-context forge reads it to
/// derive the value to load before `sret`ing into U-mode (clearing SPP,
/// setting SPIE) without disturbing unrelated bits.
#[inline]
pub fn sstatus_read() -> usize {
    let value: usize;
    // SAFETY: a plain CSR read has no memory effects.
    unsafe { asm!("csrr {}, sstatus", out(reg) value, options(nostack, nomem)) };
    value
}

/// Write `sscratch`. The trap entry uses `sscratch` as a privilege-aware
/// stack pointer: the kernel trap-stack top while a user task runs, and
/// `0` (the sentinel meaning "already on a kernel stack") while the kernel
/// runs.
///
/// # Safety
/// A wrong value corrupts the trap-entry stack swap: a non-zero value
/// while the kernel runs would make the next S-mode trap try to swap to a
/// bogus stack. Callers must restore `0` before the kernel resumes.
#[inline]
pub unsafe fn sscratch_write(value: usize) {
    unsafe { asm!("csrw sscratch, {}", in(reg) value, options(nostack, nomem)) };
}

/// Set `sstatus.SUM` (bit 18), permitting S-mode to read/write U-mode
/// pages. Opened only around a validated copy in the `print` syscall.
///
/// # Safety
/// While SUM is set the kernel can dereference user pages, so the caller
/// must have validated the pointer first and must clear SUM immediately
/// after the copy (see [`sstatus_clear_sum`]).
#[inline]
pub unsafe fn sstatus_set_sum() {
    unsafe { asm!("csrs sstatus, {}", in(reg) 1usize << 18, options(nostack, nomem)) };
}

/// Clear `sstatus.SUM` (bit 18): S-mode accesses to U-mode pages fault
/// again. The default state — only the `print` copy window deviates.
///
/// # Safety
/// Always memory-safe; pairs with [`sstatus_set_sum`].
#[inline]
pub unsafe fn sstatus_clear_sum() {
    unsafe { asm!("csrc sstatus, {}", in(reg) 1usize << 18, options(nostack, nomem)) };
}
```

- [ ] **Step 2: Cross-build**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: success (unused-warnings acceptable).

- [ ] **Step 3: Commit**

```powershell
git add arch/riscv64/src/csr.rs
git commit -m "feat(arch): sstatus read, sscratch write, and SUM window CSR accessors"
```

---

### Task 4: `UserEcall` cause + `from_user` helper (TDD where pure)

**Files:**
- Modify: `arch/riscv64/src/trap.rs`

- [ ] **Step 1: Add the `UserEcall` variant and a failing decode test**

In `arch/riscv64/src/trap.rs`, add a variant to the `Cause` enum, after
`StorePageFault`:

```rust
    /// `ecall` executed from U-mode (exception code 8) — a syscall.
    UserEcall,
```

Add a test inside `mod tests` (after `decodes_page_faults`):

```rust
    #[test]
    fn decodes_user_ecall() {
        // Exception code 8 = environment call from U-mode.
        assert_eq!(decode(8), Cause::UserEcall);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p kernel-arch-riscv64 trap`
Expected: FAIL — `decode(8)` currently returns `Cause::Unknown { interrupt: false, code: 8 }`.

- [ ] **Step 3: Add the decode arm**

In `decode`, add the arm before the catch-all `_ =>`:

```rust
        (false, 8) => Cause::UserEcall,
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p kernel-arch-riscv64 trap`
Expected: PASS (`decodes_user_ecall` plus all existing trap tests).

- [ ] **Step 5: Add the `from_user` helper (gated)**

In `arch/riscv64/src/trap.rs`, after the `instruction_len_at` function
(before `fatal`), add:

```rust
/// `sstatus.SPP` (bit 8) records the privilege the hart was in when the
/// trap fired: 0 = U-mode, 1 = S-mode. A trap "from user" is contained
/// (the task is killed); a fault from the kernel itself is fatal-to-the-
/// kernel, except the deliberate S-mode W^X probe.
#[cfg(target_arch = "riscv64")]
fn from_user(frame: &TrapFrame) -> bool {
    frame.sstatus & (1 << 8) == 0
}
```

- [ ] **Step 6: Cross-build, host test, commit**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf` — expected: success (unused `from_user` warning acceptable).
Run: `cargo test -p kernel-arch-riscv64` — expected: PASS.

```powershell
git add arch/riscv64/src/trap.rs
git commit -m "feat(arch): decode UserEcall (scause 8) and a trap-origin (SPP) helper"
```

---

### Task 5: `sscratch` trap-stack swap in `__trap_entry` (gated)

**Files:**
- Modify: `arch/riscv64/src/trap.rs`

This is the most delicate change in the phase. The rewritten entry must be
**behaviorally identical** for S-mode traps (where `sscratch == 0`) and
add the U-mode path. The 2a/2b/2c smoke patterns are the regression guard.

- [ ] **Step 1: Set the `sscratch` sentinel in `trap::init`**

`sscratch` must be `0` before the first trap (OpenSBI may leave it
non-zero). In `arch/riscv64/src/trap.rs`, in `init`, add the sentinel
write after installing the vector:

```rust
#[cfg(target_arch = "riscv64")]
pub fn init() {
    extern "C" {
        fn __trap_entry();
    }
    // SAFETY: __trap_entry is the real entry defined above; .align 2
    // gives the required 4-byte alignment.
    unsafe { crate::csr::stvec_write(__trap_entry as *const () as usize) };
    // sscratch == 0 is the "kernel is on a kernel stack" sentinel the
    // entry's stack-swap relies on; make it true before any trap fires.
    // SAFETY: writing 0 is the documented sentinel; no user task runs yet.
    unsafe { crate::csr::sscratch_write(0) };
}
```

- [ ] **Step 2: Replace the `__trap_entry` assembly**

In `arch/riscv64/src/trap.rs`, replace the entire `global_asm!` block for
`__trap_entry` (the comment above it and the `core::arch::global_asm!( r#" ... "# );`)
with the version below. The only changes from 2c are the entry prologue
(the `sscratch` swap and per-origin pre-trap-sp capture) and the exit
epilogue (restoring `sscratch` to the trap-stack top when returning to
U-mode). The middle register save/restore is unchanged.

```rust
// The assembly trap entry. Layout contract: see [`TrapFrame`].
// 288 = size_of::<TrapFrame>() (280) rounded up to keep `sp` 16-aligned.
//
// Stack swap (Phase 3a): `sscratch` holds the kernel trap-stack top while a
// user task runs and `0` while the kernel runs. On entry we swap sp and
// sscratch:
//   - from U-mode: sp <- trap-stack top, sscratch <- user sp (saved into
//     the frame's x2 slot); sscratch is then reset to 0 for the handler.
//   - from S-mode: sscratch was 0, so the swap puts 0 in sp; we detect
//     that, swap back, and run on the current kernel stack exactly as in 2c.
// On exit, if returning to U-mode (SPP == 0) we restore sscratch to the
// trap-stack top so the next trap from the user swaps correctly.
#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(
    r#"
    .section .text
    .align 2                # stvec requires 4-byte alignment (mode bits = 00, direct)
    .global __trap_entry
__trap_entry:
    csrrw sp, sscratch, sp  # swap sp <-> sscratch
    bnez sp, 1f             # sp != 0 => trapped from U-mode (sp = trap-stack top)
    csrrw sp, sscratch, sp  # from S-mode: undo the swap (sp = kernel sp, sscratch = 0)
    addi sp, sp, -288
    sd t0, 32(sp)           # free t0 (x5 slot) as scratch
    addi t0, sp, 288        # pre-trap sp (kernel): just above this frame
    j 2f
1:                          # from U-mode: sp = trap-stack top, sscratch = user sp
    addi sp, sp, -288
    sd t0, 32(sp)           # free t0 (x5 slot) as scratch
    csrr t0, sscratch       # t0 = user sp (the pre-trap sp)
    csrw sscratch, zero     # kernel now running: restore the 0 sentinel
2:                          # common path: t0 = pre-trap sp, frame allocated, x5 saved
    sd t0, 8(sp)            # x2 (pre-trap sp) into the frame
    sd x1,  0(sp)
    sd x3,  16(sp)
    sd x4,  24(sp)
    sd x6,  40(sp)
    sd x7,  48(sp)
    sd x8,  56(sp)
    sd x9,  64(sp)
    sd x10, 72(sp)
    sd x11, 80(sp)
    sd x12, 88(sp)
    sd x13, 96(sp)
    sd x14, 104(sp)
    sd x15, 112(sp)
    sd x16, 120(sp)
    sd x17, 128(sp)
    sd x18, 136(sp)
    sd x19, 144(sp)
    sd x20, 152(sp)
    sd x21, 160(sp)
    sd x22, 168(sp)
    sd x23, 176(sp)
    sd x24, 184(sp)
    sd x25, 192(sp)
    sd x26, 200(sp)
    sd x27, 208(sp)
    sd x28, 216(sp)
    sd x29, 224(sp)
    sd x30, 232(sp)
    sd x31, 240(sp)
    csrr t0, sepc
    sd t0, 248(sp)
    csrr t0, sstatus
    sd t0, 256(sp)
    csrr t0, scause
    sd t0, 264(sp)
    csrr t0, stval
    sd t0, 272(sp)
    mv a0, sp               # &mut TrapFrame
    call trap_handler
    ld t0, 248(sp)          # handler may have advanced sepc
    csrw sepc, t0
    ld t0, 256(sp)          # restored sstatus; t0 = its value
    csrw sstatus, t0
    andi t1, t0, 0x100      # SPP (bit 8): 0 => returning to U-mode
    bnez t1, 3f             # SPP = 1 (to S-mode): leave sscratch = 0
    addi t1, sp, 288        # trap-stack top (this frame sits at top - 288)
    csrw sscratch, t1       # arm sscratch for the next trap from the user
3:
    ld x1,  0(sp)
    ld x3,  16(sp)
    ld x4,  24(sp)
    ld x5,  32(sp)
    ld x6,  40(sp)
    ld x7,  48(sp)
    ld x8,  56(sp)
    ld x9,  64(sp)
    ld x10, 72(sp)
    ld x11, 80(sp)
    ld x12, 88(sp)
    ld x13, 96(sp)
    ld x14, 104(sp)
    ld x15, 112(sp)
    ld x16, 120(sp)
    ld x17, 128(sp)
    ld x18, 136(sp)
    ld x19, 144(sp)
    ld x20, 152(sp)
    ld x21, 160(sp)
    ld x22, 168(sp)
    ld x23, 176(sp)
    ld x24, 184(sp)
    ld x25, 192(sp)
    ld x26, 200(sp)
    ld x27, 208(sp)
    ld x28, 216(sp)
    ld x29, 224(sp)
    ld x30, 232(sp)
    ld x31, 240(sp)
    ld x2,  8(sp)           # restore original sp LAST (user sp or kernel sp)
    sret
"#
);
```

- [ ] **Step 3: Cross-build**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: success.

- [ ] **Step 4: Regression — the 2a/2b/2c milestones must still pass**

Run: `./tools/test-qemu.ps1`
Expected: still `BOOT TEST FAIL`, but **only** the three `user: ...`
patterns missing. The rewritten entry must not break any 2a/2b/2c pattern
(greeting, breakpoint, paging, W^X, frames, the round-robin rotation,
preempted-the-hog, ticks). If a 2a/2b/2c pattern regressed, the swap logic
is wrong for the S-mode path — debug with systematic-debugging before
proceeding (likely the S-mode `bnez`/undo-swap or the pre-trap-sp capture).

- [ ] **Step 5: Host tests and commit**

Run: `cargo test -p kernel-arch-riscv64` — expected: PASS.

```powershell
git add arch/riscv64/src/trap.rs
git commit -m "feat(arch): sscratch trap-stack swap in __trap_entry (U-mode trap path)"
```

---

### Task 6: User-context forge + `enter_user`/termination path (gated, with pure tests)

**Files:**
- Modify: `arch/riscv64/src/task.rs`
- Modify: `arch/riscv64/src/sched.rs`

- [ ] **Step 1: Add `ExitReason` and the pure `user_sstatus` forge (TDD)**

In `arch/riscv64/src/task.rs`, after the `TaskState` enum, add the
`ExitReason` type and the pure `sstatus` computation. `Cause` is the
ungated enum in `trap`, referenced by full path.

```rust
/// Why a U-mode task stopped running, reported back by `sched::enter_user`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    /// The task called `exit(code)`.
    Exited(usize),
    /// The task was killed by a fatal U-mode trap (e.g. touching kernel
    /// memory). Carries the decoded cause for the diagnostic line.
    Killed(crate::trap::Cause),
}

/// Compute the `sstatus` value to load before `sret`ing into U-mode,
/// starting from the current `sstatus`. Clears SPP (bit 8) so `sret`
/// drops to U-mode, and sets SPIE (bit 5) so interrupts are enabled after
/// the return. All other bits (notably SUM = 0) are preserved.
pub fn user_sstatus(current: usize) -> usize {
    (current & !(1 << 8)) | (1 << 5)
}
```

Add to `mod tests` in `task.rs`:

```rust
    #[test]
    fn user_sstatus_drops_to_user_with_interrupts_on() {
        // Start from SPP = 1 (in S-mode) with SPIE clear.
        let s = user_sstatus(1 << 8);
        assert_eq!(s & (1 << 8), 0, "SPP must be 0 so sret enters U-mode");
        assert_ne!(s & (1 << 5), 0, "SPIE must be 1 so interrupts resume");
    }

    #[test]
    fn user_sstatus_preserves_other_bits() {
        // SUM (bit 18) must NOT be turned on by the forge.
        let s = user_sstatus(1 << 18);
        assert_ne!(s & (1 << 18), 0, "unrelated bits preserved");
        assert_eq!(s & (1 << 8), 0);
        assert_ne!(s & (1 << 5), 0);
    }
```

- [ ] **Step 2: Run the new pure tests**

Run: `cargo test -p kernel-arch-riscv64 task`
Expected: PASS (the two `user_sstatus_*` tests plus the existing `task`
tests). Pure bit math, passes immediately.

- [ ] **Step 3: Add the launch assembly to `sched.rs`**

In `arch/riscv64/src/sched.rs`, append a second `global_asm!` block (after
the existing `switch_context`/`task_trampoline` block):

```rust
// First entry into U-mode. Reached via `switch_context` "returning" into
// it with the launchpad context built by `enter_user`:
//   s0 = user entry (-> sepc), s1 = user sp, s2 = user sstatus,
//   sp = kernel trap-stack top (we are running on it now).
// We arm sscratch with the trap-stack top (so the user's first trap swaps
// onto it), load the user CSRs, switch to the user stack, and `sret`.
#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(
    r#"
    .section .text
    .global enter_user_asm
enter_user_asm:
    csrw sscratch, sp       # sscratch = trap-stack top (for the U->S trap)
    csrw sepc, s0           # user entry point
    csrw sstatus, s2        # SPP = 0, SPIE = 1
    mv sp, s1               # switch to the user stack
    sret                    # -> U-mode at the entry point
"#
);

#[cfg(target_arch = "riscv64")]
extern "C" {
    fn enter_user_asm();
}
```

- [ ] **Step 4: Add the user-return statics, `enter_user`, and `terminate_user`**

Append to `arch/riscv64/src/sched.rs`:

```rust
use crate::task::ExitReason;

/// The kernel context to resume when the user task ends. `enter_user`
/// saves `kmain`'s registers here; `terminate_user` restores them, so
/// `enter_user` returns to its caller. Single hart, mutated only with
/// interrupts disabled.
#[cfg(target_arch = "riscv64")]
static mut USER_RETURN: Context = Context::zeroed();

/// Set by `terminate_user`, read by `enter_user` after it resumes.
#[cfg(target_arch = "riscv64")]
static mut USER_EXIT: ExitReason = ExitReason::Exited(0);

/// Run a single U-mode task to completion. Builds a launchpad context that
/// `switch_context` "returns" into (landing in `enter_user_asm`, which
/// `sret`s to U-mode), and saves `kmain`'s context so the termination path
/// can switch back. Returns the reason the task stopped.
///
/// `user_sp` must be the (16-aligned) top of a `U`-mapped stack; `entry`
/// must live in a `U`-mapped, executable page; `trap_stack_top` is the top
/// of a trusted kernel stack the user's traps land on.
#[cfg(target_arch = "riscv64")]
pub fn enter_user(entry: extern "C" fn() -> !, user_sp: usize, trap_stack_top: usize) -> ExitReason {
    // SAFETY: the launch handshake mutates the gated statics and the live
    // CSRs; interrupts are off so no trap re-enters mid-handshake.
    let _ = unsafe { crate::csr::sstatus_disable_interrupts() };

    let mut launch = Context::zeroed();
    launch.ra = enter_user_asm as *const () as usize;
    launch.sp = trap_stack_top & !0xF;
    launch.s[0] = entry as *const () as usize; // -> sepc
    launch.s[1] = user_sp & !0xF; // -> user sp
    launch.s[2] = crate::task::user_sstatus(crate::csr::sstatus_read()); // -> sstatus

    // SAFETY: USER_RETURN is a 'static save target; `launch` is a valid
    // launchpad. switch_context saves kmain into USER_RETURN and resumes
    // in enter_user_asm. We return here only when terminate_user restores
    // USER_RETURN.
    unsafe {
        switch_context(core::ptr::addr_of_mut!(USER_RETURN), &launch);
        core::ptr::read(core::ptr::addr_of!(USER_EXIT))
    }
}

/// End the running U-mode task: record `reason`, drop `sscratch` back to
/// the kernel sentinel, and switch to the context `enter_user` parked.
/// Never returns to the caller (the trap handler) — control resumes inside
/// `enter_user`. Called from the trap handler for `exit` and fatal U-mode
/// faults.
#[cfg(target_arch = "riscv64")]
pub fn terminate_user(reason: ExitReason) -> ! {
    // SAFETY: single hart, interrupts off inside the trap handler. Record
    // the reason, reset the sscratch sentinel (the kernel is resuming),
    // and switch into kmain's parked context via a throwaway save slot.
    unsafe {
        core::ptr::write(core::ptr::addr_of_mut!(USER_EXIT), reason);
        crate::csr::sscratch_write(0);
        let mut throwaway = Context::zeroed();
        switch_context(
            core::ptr::addr_of_mut!(throwaway),
            core::ptr::addr_of!(USER_RETURN),
        );
    }
    unreachable!("terminate_user resumes inside enter_user, never here")
}
```

- [ ] **Step 5: Cross-build and host test**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: success (unused `enter_user`/`terminate_user` warnings acceptable
until Task 7/8 wire them).
Run: `cargo test -p kernel-arch-riscv64`
Expected: PASS (gated code is invisible to the host).

- [ ] **Step 6: Commit**

```powershell
git add arch/riscv64/src/task.rs arch/riscv64/src/sched.rs
git commit -m "feat(arch): user-context forge, enter_user, and the termination path"
```

---

### Task 7: Linker `.user` sections, user-image mapping, and syscall dispatch (gated)

**Files:**
- Modify: `kernel/kernel.ld`
- Modify: `arch/riscv64/src/mem/mod.rs`
- Modify: `arch/riscv64/src/syscall.rs`
- Modify: `arch/riscv64/src/trap.rs`

The linker sections and the `mem` accessor come first so the gated
dispatcher (which calls `mem::user_data_bounds()`) links. With no user code
in `kmain` yet, the `.user_*` sections are empty (start == end) — that is
fine; the kernel still links and boots, and Task 8 fills them in.

- [ ] **Step 1: Add the `.user_text` and `.user_data` sections to the linker script**

In `kernel/kernel.ld`, insert the two user sections after the `.stack`
block and before `__kernel_end` (so the user image is part of the kernel
image, below the free RAM the allocator manages, and is NOT covered by the
existing kernel mappings). Replace:

```
    /* Boot stack for the single hart we run. */
    .stack (NOLOAD) : {
        __stack_start = .;
        . += 64K;
        __stack_top = .;
    }
    __kernel_end = .;            /* frame allocator manages from here */
```

with:

```
    /* Boot stack for the single hart we run. */
    .stack (NOLOAD) : {
        __stack_start = .;
        . += 64K;
        __stack_top = .;
    }

    /* Phase 3a: the embedded U-mode "program". mem::init() maps these with
     * the PTE U bit (text R-X-U, data RW-U); everything above stays non-U,
     * so a U-mode access to the kernel faults. 4 KiB-aligned: permissions
     * are per-page. */
    . = ALIGN(4K);
    __user_text_start = .;
    .user_text : { *(.user_text) }
    . = ALIGN(4K);
    __user_text_end = .;

    __user_data_start = .;
    .user_data : { *(.user_data) }
    . = ALIGN(4K);
    __user_data_end = .;

    __kernel_end = .;            /* frame allocator manages from here */
```

- [ ] **Step 2: Declare the user symbols and map the user image in `mem`**

In `arch/riscv64/src/mem/mod.rs`, add the four linker symbols to the
`extern "C"` block (after `__kernel_end`):

```rust
    static __user_text_start: u8;
    static __user_text_end: u8;
    static __user_data_start: u8;
    static __user_data_end: u8;
```

In `init`, replace the PTE-flag `use` line:

```rust
    use paging::{PTE_G, PTE_R, PTE_W, PTE_X};
```

with the `PTE_U`-including version:

```rust
    use paging::{PTE_G, PTE_R, PTE_U, PTE_W, PTE_X};
```

Then, inside the `unsafe` block in `init`, add the user mappings after the
free-RAM `map_range` and immediately before the `satp_write` call:

```rust
        // Phase 3a: the embedded user image gets the U bit so a U-mode
        // task can fetch/read/write it. NOT global (G): user mappings are
        // not shared across address spaces. text R-X-U, data RW-U. Empty
        // until kmain places code/data here (start == end maps nothing).
        paging::map_range(root, sym!(__user_text_start), sym!(__user_text_end), PTE_R | PTE_X | PTE_U);
        paging::map_range(root, sym!(__user_data_start), sym!(__user_data_end), PTE_R | PTE_W | PTE_U);
```

Add a public accessor after `total_frames`:

```rust
/// Half-open bounds `[start, end)` of the `.user_data` region — the only
/// memory a `U`-mode task may legitimately ask the kernel to read in a
/// `print` syscall. Used by the confused-deputy guard.
#[cfg(target_arch = "riscv64")]
pub fn user_data_bounds() -> (usize, usize) {
    // SAFETY: both are linker-script symbol addresses (read as addresses,
    // never dereferenced here); the region is defined by kernel.ld.
    unsafe { (sym!(__user_data_start), sym!(__user_data_end)) }
}
```

- [ ] **Step 3: Cross-build to confirm the symbols resolve**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: success — the kernel links with the new (empty) `.user_*`
sections. `user_data_bounds` is unused for now (warning acceptable).

- [ ] **Step 4: Add the gated dispatcher to `syscall.rs`**

Append to `arch/riscv64/src/syscall.rs`:

```rust
/// What the trap handler should do after a syscall returns.
#[cfg(target_arch = "riscv64")]
pub enum Outcome {
    /// Resume the user task (the handler advances `sepc` past the `ecall`).
    Resume,
    /// The task asked to exit with this code.
    Exit(usize),
}

/// Largest `print` we copy in one syscall. The demo strings are short; a
/// longer buffer is silently truncated to this (a real kernel would loop).
#[cfg(target_arch = "riscv64")]
const PRINT_MAX: usize = 256;

/// Service a U-mode `ecall`. Reads the ABI registers from `frame`,
/// dispatches, and writes the return value into the `a0` slot. Reading
/// user memory for `print` happens only inside a validated `SUM` window.
///
/// Register/`TrapFrame` mapping: `regs[n-1]` holds `x_n`, so `a0` = `x10`
/// is `regs[9]`, `a1` = `x11` is `regs[10]`, `a7` = `x17` is `regs[16]`.
#[cfg(target_arch = "riscv64")]
pub fn dispatch(frame: &mut crate::trap::TrapFrame) -> Outcome {
    let a7 = frame.regs[16];
    let a0 = frame.regs[9];
    let a1 = frame.regs[10];
    match decode_syscall(a7) {
        Syscall::Print => {
            let written = sys_print(a0, a1);
            frame.regs[9] = written; // return value in a0
            Outcome::Resume
        }
        Syscall::Exit => Outcome::Exit(a0),
        Syscall::Unknown(_) => {
            frame.regs[9] = usize::MAX; // -1: unknown syscall
            Outcome::Resume
        }
    }
}

/// Validate, then copy `[ptr, ptr+len)` out of user memory and print it.
/// Returns the number of bytes written, or `usize::MAX` if validation
/// failed (the confused-deputy guard refused the pointer).
#[cfg(target_arch = "riscv64")]
fn sys_print(ptr: usize, len: usize) -> usize {
    let (lo, hi) = crate::mem::user_data_bounds();
    if !validate_user_buffer(lo, hi, ptr, len) {
        return usize::MAX;
    }
    let n = core::cmp::min(len, PRINT_MAX);
    let mut buf = [0u8; PRINT_MAX];
    // SAFETY: the range is validated to lie within the user data region,
    // which is mapped R+U. SUM is opened only for this copy and cleared
    // immediately after, so the kernel cannot read kernel memory here.
    unsafe {
        crate::csr::sstatus_set_sum();
        for i in 0..n {
            buf[i] = core::ptr::read_volatile((ptr + i) as *const u8);
        }
        crate::csr::sstatus_clear_sum();
    }
    // Print as a lossy string; the demo message is valid UTF-8.
    crate::print!("{}", core::str::from_utf8(&buf[..n]).unwrap_or("<non-utf8>"));
    n
}
```

- [ ] **Step 5: Wire `UserEcall` and the U-mode fault arms in `trap_handler`**

In `arch/riscv64/src/trap.rs`, in `trap_handler`, the page-fault arms must
distinguish a U-mode fault (contain the task) from the S-mode W^X probe
(skip) and other S-mode faults (fatal). Replace the three page-fault arms
and the `Unknown` arm, and add a `UserEcall` arm. The `Breakpoint` and
`SupervisorTimer` arms are unchanged.

Replace this block:

```rust
        Cause::InstructionPageFault => fatal("instruction page fault", frame),
        Cause::LoadPageFault => fatal("load page fault", frame),
        Cause::StorePageFault => {
            if EXPECTING_WX_FAULT.swap(false, Ordering::AcqRel) {
                crate::println!("trap: W^X store fault at {:#x} (probe)", frame.stval);
                // Like the breakpoint: skip the faulting store so
                // execution resumes after the probe.
                frame.sepc += instruction_len_at(frame.sepc);
            } else {
                fatal("store page fault", frame);
            }
        }
        Cause::Unknown { interrupt, code } => {
            crate::println!("trap: unknown cause interrupt={interrupt} code={code}");
            fatal("unknown", frame);
        }
```

with:

```rust
        Cause::UserEcall => {
            // A syscall from U-mode. ecall does not advance the PC, so on
            // Resume we step sepc past the 4-byte ecall; Exit never returns
            // here (terminate_user switches back to kmain).
            match crate::syscall::dispatch(frame) {
                crate::syscall::Outcome::Resume => frame.sepc += 4,
                crate::syscall::Outcome::Exit(code) => {
                    crate::sched::terminate_user(crate::task::ExitReason::Exited(code))
                }
            }
        }
        Cause::InstructionPageFault | Cause::LoadPageFault if from_user(frame) => {
            // A U-mode task reached for memory it does not own: contain it.
            crate::sched::terminate_user(crate::task::ExitReason::Killed(decode(frame.scause)));
        }
        Cause::InstructionPageFault => fatal("instruction page fault", frame),
        Cause::LoadPageFault => fatal("load page fault", frame),
        Cause::StorePageFault => {
            if from_user(frame) {
                crate::sched::terminate_user(crate::task::ExitReason::Killed(Cause::StorePageFault));
            } else if EXPECTING_WX_FAULT.swap(false, Ordering::AcqRel) {
                crate::println!("trap: W^X store fault at {:#x} (probe)", frame.stval);
                // Like the breakpoint: skip the faulting store so
                // execution resumes after the probe.
                frame.sepc += instruction_len_at(frame.sepc);
            } else {
                fatal("store page fault", frame);
            }
        }
        Cause::Unknown { interrupt, code } => {
            crate::println!("trap: unknown cause interrupt={interrupt} code={code}");
            fatal("unknown", frame);
        }
```

- [ ] **Step 6: Cross-build and host test**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: success (`enter_user` is still unused until Task 8 — warning
acceptable).
Run: `cargo test -p kernel-arch-riscv64`
Expected: PASS.

- [ ] **Step 7: Commit**

```powershell
git add kernel/kernel.ld arch/riscv64/src/mem/mod.rs arch/riscv64/src/syscall.rs arch/riscv64/src/trap.rs
git commit -m "feat(arch): .user sections, U-mapped user image, syscall dispatch + fault containment"
```

---

### Task 8: Wire kmain — the U-mode program and demo (smoke test goes green)

**Files:**
- Modify: `kernel/src/main.rs`

- [ ] **Step 1: Update the greeting label and imports**

In `kernel/src/main.rs`, update the greeting phase label:

```rust
        println!("{GREETING} from {PROJECT_NAME} - Phase 3a (hart {hartid})");
```

Update the `use` line inside `mod bare` to add `syscall` is *not* needed
(the user program uses raw `ecall`); add the `task::ExitReason` type:

```rust
    use kernel_arch_riscv64::{mem, println, sched, task::ExitReason, timer, trap};
```

- [ ] **Step 2: Insert the user episode in `kmain`**

In `kmain`, insert the user episode between `frame_roundtrip();` and the
`// Phase 2c:` comment (so it runs after 2b setup and before the scheduler,
which stays last). Add:

```rust
        frame_roundtrip();

        // Phase 3a: run the embedded U-mode program to completion, twice.
        // The first task prints via a syscall and exits cleanly; the second
        // reaches into kernel memory and is contained. enter_user returns
        // when each task ends. Interrupts are still off here (timer::start
        // is below), so the user runs with no preemption — the focus is the
        // privilege boundary, not scheduling.
        let trap_top = core::ptr::addr_of!(TRAP_STACK) as usize + TASK_STACK;
        let user_sp = core::ptr::addr_of!(USER_STACK) as usize + USER_STACK_SIZE;
        match sched::enter_user(user_good, user_sp, trap_top) {
            ExitReason::Exited(code) => println!("user: task exited with code {code}"),
            ExitReason::Killed(c) => println!("user: task killed by {c:?} (unexpected)"),
        }
        match sched::enter_user(user_bad, user_sp, trap_top) {
            ExitReason::Killed(_) => println!("user: task killed by load page fault"),
            ExitReason::Exited(code) => {
                println!("user: task exited with code {code} (boundary NOT enforced!)")
            }
        }

        // Phase 2c: spawn three tasks and hand the CPU to the scheduler.
```

- [ ] **Step 3: Add the trap stack and the user program**

In `kernel/src/main.rs`, inside `mod bare`, after the existing
`static mut STACK_C` declaration (and the `TASK_STACK` const), add:

```rust
    /// Trusted kernel stack that U-mode traps land on (via the sscratch
    /// swap). Kernel memory — never mapped U.
    static mut TRAP_STACK: [u8; TASK_STACK] = [0; TASK_STACK];

    /// The U-mode task's stack. Lives in `.user_data` so mem::init maps it
    /// RW-U; sized separately from kernel stacks.
    const USER_STACK_SIZE: usize = 8 * 1024;
    #[link_section = ".user_data"]
    static mut USER_STACK: [u8; USER_STACK_SIZE] = [0; USER_STACK_SIZE];

    /// The message the good user task asks the kernel to print. In
    /// `.user_data` (R-U) so the confused-deputy guard accepts its pointer
    /// and the SUM-window copy can read it. A `.rodata` string would be
    /// rejected (it is outside the user region) — that is the point. The
    /// "user: " prefix is part of the message so the smoke test can grep
    /// for the kernel's verbatim echo. Length is exact: 27 bytes.
    #[link_section = ".user_data"]
    static USER_MSG: [u8; 27] = *b"user: hello from user mode\n";

    /// Print syscall (a7 = 1): a0 = ptr, a1 = len. `inline(always)` so it
    /// folds into the user entry and the `.user_text` page stays
    /// self-contained (no call into kernel `.text`). a0 is in/out: the
    /// kernel returns the byte count there, which we discard.
    ///
    /// # Safety
    /// `ptr`/`len` must describe the buffer the kernel should print; the
    /// kernel validates the range before reading it.
    #[inline(always)]
    unsafe fn sys_print(ptr: *const u8, len: usize) {
        core::arch::asm!(
            "ecall",
            in("a7") 1usize,
            inout("a0") ptr => _,
            in("a1") len,
            options(nostack),
        );
    }

    /// Exit syscall (a7 = 2): a0 = code. Never returns.
    ///
    /// # Safety
    /// Always sound; the kernel terminates the task and never resumes it.
    #[inline(always)]
    unsafe fn sys_exit(code: usize) -> ! {
        core::arch::asm!(
            "ecall",
            in("a7") 2usize,
            in("a0") code,
            options(nostack, noreturn),
        );
    }

    /// The well-behaved U-mode task: print a message, then exit cleanly.
    /// In `.user_text` (R-X-U); calls only the inlined syscall stubs, so it
    /// never touches a non-U page.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn user_good() -> ! {
        // SAFETY: USER_MSG is in .user_data (R-U); we only take its address
        // (no U-mode read of it) — the kernel validates and reads it.
        unsafe {
            sys_print(USER_MSG.as_ptr(), USER_MSG.len());
            sys_exit(0)
        }
    }

    /// The misbehaving U-mode task: read a kernel address. The kernel page
    /// is mapped non-U, so the U-mode load faults and the kernel contains
    /// the task (it never reaches the exit below).
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn user_bad() -> ! {
        // 0x80200000 is the kernel .text base (mapped R-X-G, no U bit).
        let kernel_addr = 0x8020_0000 as *const u8;
        // SAFETY: this is the deliberate boundary violation. The volatile
        // read faults in U-mode before it can complete; control never
        // returns to this task.
        let _ = unsafe { core::ptr::read_volatile(kernel_addr) };
        unsafe { sys_exit(0) } // unreachable: the read above faults first
    }
```

- [ ] **Step 4: Run the full smoke test**

Run: `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS: 2a + 2b + 2c milestones plus the U-mode print
round-trip, clean exit, and contained fault all observed.` — exit code 0.

If it fails, the script prints the serial log. Debug with the
systematic-debugging skill before editing — likely culprits and checks:
- `user: hello from user mode` missing, no fault → the `sret` did not reach
  U-mode, or the `.user_text`/`.user_data` pages were not mapped U
  (re-check the linker symbols and the `mem::init` `map_range` calls).
- A FATAL TRAP / panic instead of the print → the kernel itself faulted
  reading the user buffer: confirm the `SUM` window wraps the copy and the
  `print` pointer passes validation (`USER_MSG` must be in `.user_data`,
  not `.rodata`).
- `user: task exited with code 0` missing but the print appeared → the
  `exit` syscall is not routing through `terminate_user`, or `enter_user`
  did not park/restore `USER_RETURN` correctly.
- `user: task killed by load page fault` missing / a kernel panic on the
  second task → the U-mode load is being treated as an S-mode fault:
  re-check `from_user` (SPP bit) and that `sscratch` was reset to 0 by the
  prior `terminate_user`.
- A 2c regression (rotation / preempted-the-hog missing or a hang) → the
  scheduler episode after the user episode is mis-sequenced; confirm the
  user `match` arms come before the `sched::spawn` calls and that
  `sscratch == 0` on return from the second `enter_user`.

- [ ] **Step 5: Run the host tests**

Run: `cargo test`
Expected: PASS (whole workspace).

- [ ] **Step 6: Commit**

```powershell
git add kernel/src/main.rs
git commit -m "feat: Phase 3a live - U-mode task, print/exit syscalls, contained fault"
```

---

### Task 9: Documentation

**Files:**
- Create: `docs/learning/0006-user-mode-and-syscalls.md`
- Modify: `docs/learning/README.md`
- Modify: `docs/glossary.md`
- Modify: `docs/roadmap/roadmap.md`

- [ ] **Step 1: Write the learning note**

Create `docs/learning/0006-user-mode-and-syscalls.md`. The draft below is
real content to adapt — extend any section with what actually surprised
you during implementation:

```markdown
# 0006 — User mode and system calls (Phase 3a)

How the kernel handed the CPU to code it does not trust, gave that code a
single narrow door back in (the syscall), and proved the door is the
*only* way in.

## Two privilege levels, one instruction to cross

RISC-V S-mode (the kernel) and U-mode (applications) differ in what memory
and instructions are allowed. `sret` is the move *down*: it reads
`sstatus.SPP` (0 = return to U-mode), restores the prior interrupt-enable
from `SPIE`, and jumps to `sepc`. To launch the first user task we forge
that state — `SPP = 0`, `SPIE = 1`, `sepc` = the user entry, `sp` = the
user stack — and `sret`. There was no "previous" U-mode state to restore;
we manufactured a plausible one, exactly as Phase 2c forged a task's first
context.

## The untrusted stack problem

A trap from U-mode arrives with `sp` still pointing at the *user* stack.
The kernel must not push its trap frame there — the user controls it. The
fix is `sscratch`, a scratch CSR used as a privilege-aware stack pointer:
while the user runs it holds the kernel trap-stack top; while the kernel
runs it holds 0. The first instruction of the trap entry swaps `sp` and
`sscratch`. From U-mode that lands us on the trap stack (and stashes the
user `sp`); from S-mode the swap yields 0, which we detect and undo,
running on the current kernel stack exactly as before. One instruction,
two behaviors, selected by a sentinel.

## The syscall is the whole interface

A user task reaches the kernel only by executing `ecall` (trap cause 8).
The ABI is a convention: `a7` selects the call, `a0..` carry arguments,
`a0` carries the result. `ecall` does not advance the PC, so the handler
adds 4 to `sepc` or the task would trap on the same instruction forever.
Two calls exist so far: `print` and `exit`.

## The confused deputy, and why SUM is off by default

`print` takes a pointer and a length *from the user*. If the kernel — which
can read anything — blindly dereferenced them, a user could pass a kernel
address and have the privileged kernel read it out. That is the "confused
deputy": a powerful agent tricked into misusing its power. Two defenses
compose. First, the kernel validates the range lies inside the user's own
memory before touching it. Second, the hardware bit `sstatus.SUM` is 0 by
default, so even an *accidental* kernel read of a user page faults — and
because of that, reading the (validated) user buffer requires *explicitly*
opening a SUM window for the copy and closing it immediately. The check
stops a deliberate bad pointer; the default-off SUM stops a careless one.

## Containment: a fault that kills the task, not the kernel

When the second demo task reads a kernel address, the MMU faults because
the kernel's pages have no U bit. The handler reads `sstatus.SPP`: the
fault came from U-mode, so instead of panicking it *contains* the task —
records the reason and switches back to the kernel context that launched
it. Contrast the Phase 2b W^X probe: that store fault comes from S-mode and
is deliberately skipped. Same machinery (page fault), opposite response,
chosen by where the fault came from.

## Why the user episode runs before the scheduler

Phase 2c's scheduler demo loops forever, so the user episode runs first and
is *bounded*: `enter_user` `sret`s into the task and returns to `kmain`
only when the task exits or is killed. That kept the 2c scheduler code
untouched and the new privilege machinery isolated — the boundary, not
scheduling, is the lesson here. Putting U-mode tasks into the run queue is
Phase 3b's job.
```

- [ ] **Step 2: Index the note**

In `docs/learning/README.md`, append to the Notes list:

```markdown
- [0006 — User mode and system calls (Phase 3a)](0006-user-mode-and-syscalls.md)
```

- [ ] **Step 3: Add glossary entries**

Append to `docs/glossary.md` (keeping the one-bullet-per-term format; the
generic terms `ecall`, `M/S/U-mode`, and `Capability` already exist — these
add the 3a specifics):

```markdown
- **System call (syscall)** — the controlled entry by which an unprivileged user task requests a service from the kernel; in this kernel, an `ecall` from U-mode decoded by number (`print`, `exit`).
- **Syscall ABI** — the register convention for a syscall: here `a7` selects the call, `a0..` pass arguments, and `a0` returns the result.
- **`sret`** — the RISC-V instruction that returns from a trap to a lower or equal privilege; reading `sstatus.SPP` decides whether it lands in U-mode or S-mode.
- **`sstatus.SPP`** — the "supervisor previous privilege" bit: records whether a trap came from U-mode (0) or S-mode (1), and selects where `sret` returns.
- **`sstatus.SPIE` / `SIE`** — the saved and live supervisor interrupt-enable bits; `sret` restores `SIE` from `SPIE`, so a task entered with `SPIE = 1` resumes with interrupts on.
- **`sscratch`** — a scratch CSR used as a privilege-aware stack pointer: it holds the kernel trap-stack top while a user task runs and 0 while the kernel runs, letting the trap entry swap to a trusted stack only when needed.
- **Trap stack / kernel stack** — the trusted stack a trap from U-mode must switch to before the kernel touches it, since the interrupted user stack is untrusted.
- **`sstatus.SUM`** — "permit Supervisor User Memory access": when 0 (the default), S-mode accesses to user pages fault; the `print` syscall opens it briefly for a validated copy.
- **Confused deputy** — a privileged component tricked into misusing its authority on behalf of a less-privileged caller; here, the kernel reading a kernel address supplied as a user `print` pointer — prevented by validating the pointer against the user's own memory.
- **Task termination / containment** — ending a task cleanly (`exit`) or because it faulted (killed); a U-mode fault contains just the task, leaving the kernel running.
- **U bit (PTE)** — the page-table entry flag that makes a page accessible to U-mode; kernel pages omit it, so a user touch faults.
```

- [ ] **Step 4: Decompose Phase 3 and mark 3a done in the roadmap**

In `docs/roadmap/roadmap.md`, replace the entire `## Phase 3 — Security
spine` section body (the three bullet lines):

```markdown
- **Goal:** capability-based isolation and post-quantum crypto primitives, designed in from here.
- **You learn:** capabilities, IPC, integrating audited crypto.
- **Done when:** components run with least authority and a PQC primitive is usable.
```

with the decomposed version:

```markdown
Decomposed into three sub-phases (2026-06-14), mirroring the 2a/2b/2c split.
Capabilities need an unprivileged component to *hold* them, so the privilege
transition comes first; PQC is the self-contained finale. Each sub-phase
gets its own design → plan → build cycle.

### Phase 3a — Privilege drop to user space  *(done — 2026-06-14)*

- **Goal:** the first transition to U-mode, a minimal `print`/`exit` syscall path, and proof the privilege boundary enforces.
- **You learn:** the U/S boundary (`sstatus.SPP`/`SPIE`, `sret`), the `sscratch` trap-stack swap, `ecall` syscalls and the ABI, the confused-deputy guard and `SUM`, the PTE `U` bit.
- **Done when:** `./tools/test-qemu.ps1` observes a U-mode task make a `print` syscall, exit cleanly, and a second U-mode task touching kernel memory contained — all in one boot, alongside the 2a/2b/2c milestones.

### Phase 3b — Capabilities & IPC

- **Goal:** unforgeable capability tokens, capability-checked syscalls, and synchronous message passing between two isolated user components (with blocking/wait queues and U-mode tasks in the run queue).
- **You learn:** capabilities, IPC, per-address-space isolation, blocking.
- **Done when:** two components run at least authority and communicate only through capability-checked IPC.

### Phase 3c — PQC primitive

- **Goal:** integrate an audited post-quantum crypto crate and expose one usable primitive (per ADR 0004).
- **You learn:** integrating audited crypto into `no_std`, the `libs/crypto` crate.
- **Done when:** a PQC primitive (e.g. ML-KEM keygen/encapsulation or ML-DSA verify) is usable and host-tested.
```

- [ ] **Step 5: Verify references and commit**

Run: `./tools/check-references.ps1` — expected: `Reference check OK`.
Run: `cargo test` — expected: PASS.

```powershell
git add docs/learning docs/glossary.md docs/roadmap/roadmap.md
git commit -m "docs: user-mode learning note, glossary terms, Phase 3 decomposed and 3a done"
```

---

### Task 10: Final verification

- [ ] **Step 1: Full host test suite**

Run: `cargo test`
Expected: PASS, zero failures.

- [ ] **Step 2: Cross-build clean**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: success, no warnings about unused user-mode items (everything is
wired by now).

- [ ] **Step 3: Smoke test**

Run: `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS` — all twelve patterns in one boot.

- [ ] **Step 4: Reference check**

Run: `./tools/check-references.ps1`
Expected: `Reference check OK`.

No commit — this task only verifies. If anything fails, fix it with the
systematic-debugging skill before declaring the phase done.
