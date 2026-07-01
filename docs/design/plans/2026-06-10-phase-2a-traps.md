# Phase 2a — Trap Handling & Timer Heartbeat Implementation Plan

> **For agentic workers:** Implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Install a supervisor-mode trap handler that catches and recovers from a deliberate breakpoint exception, then enables SBI timer interrupts producing a ~1 Hz heartbeat — proven by the QEMU smoke test.

**Architecture:** All mechanism lives in the arch crate (`kernel-arch-riscv64`) beside `sbi.rs`/`console.rs`: hand-rolled CSR accessors, a single direct-mode assembly trap entry saving a full `TrapFrame`, a Rust dispatcher matching on decoded `scause`, and a one-shot SBI TIME-extension timer that re-arms itself. The kernel binary only orchestrates (`trap::init()` → `ebreak` → `timer::start()` → park). Pure logic (`scause` decoding, instruction-length detection) is ungated so it host-tests like the existing arch-crate tests.

**Tech Stack:** Rust nightly (pinned), `riscv64gc-unknown-none-elf` cross-target, inline/global asm, QEMU `virt` + OpenSBI, PowerShell test scripts.

**Spec:** `docs/design/specs/2026-06-10-phase-2a-traps-design.md`

**Conventions:** Conventional Commit messages. **No co-author trailers.** Host checks: `cargo build` + `cargo test`. Cross build: `cargo build -p kernel --target riscv64gc-unknown-none-elf`. Smoke test: `./tools/test-qemu.ps1`.

**Deviation from spec, locked in here:** the spec lists CSR accessors for `sepc`, `scause`, `stval`; the trap entry assembly captures those directly into the `TrapFrame`, so Rust accessors for them would be dead code. YAGNI — `csr.rs` implements only what is called: `stvec` write, `sie`/`sstatus` interrupt-enable bits, `time` read.

---

## File map

| File | Change | Responsibility |
|------|--------|----------------|
| `tools/test-qemu.ps1` | modify | Smoke test asserts breakpoint recovery + ≥2 ticks (Task 1) |
| `arch/riscv64/src/trap.rs` | create | `TrapFrame`, `Cause`, `decode`, `instruction_len` (pure, host-tested); entry asm, dispatcher, `init()` (riscv64-gated) |
| `arch/riscv64/src/csr.rs` | create | Inline-asm CSR accessors (riscv64-gated) |
| `arch/riscv64/src/sbi.rs` | modify | Add TIME-extension `set_timer` |
| `arch/riscv64/src/timer.rs` | create | Tick interval, tick counter, heartbeat print, re-arm (riscv64-gated) |
| `arch/riscv64/src/lib.rs` | modify | Wire new modules |
| `kernel/src/main.rs` | modify | kmain: init traps → `ebreak` → start timer → park |
| `docs/learning/0003-traps-and-interrupts.md` | create | Learning note (Task 6) |
| `docs/glossary.md` | modify | New terms (Task 6) |
| `docs/roadmap/roadmap.md` | modify | Record 2a/2b/2c decomposition (Task 6) |

---

### Task 1: Extend the QEMU smoke test (failing first)

The smoke test currently greps only for "hello world". Phase 2a's exit criterion adds three patterns. Write the test first; it must fail against the Phase 1 kernel.

**Files:**
- Modify: `tools/test-qemu.ps1`

- [ ] **Step 1: Update the smoke test to require all Phase 2a patterns**

Replace the header comment (lines 1–4) with:

```powershell
# Boot smoke test: cross-builds the kernel, boots it headless under QEMU
# (riscv64 virt + OpenSBI), captures the serial console, and asserts the
# Phase 2a milestones: the greeting, a caught-and-survived breakpoint
# exception, and at least two timer-heartbeat ticks.
# Usage: ./tools/test-qemu.ps1     (exit code 0 = pass, 1 = fail)
```

Replace the match loop and verdict (the `$found = $false` block through the final `if/else`) with:

```powershell
# Every pattern must appear in one boot. "tick: 2" implies >= 2 ticks
# because the tick counter is monotonic.
$mustMatch = @(
    "hello world",
    "trap: breakpoint",
    "survived breakpoint",
    "tick: 2"
)
$missing = $mustMatch
try {
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 500
        $text = Read-LogText $serialLog
        $missing = @($mustMatch | Where-Object { $text -notmatch [regex]::Escape($_) })
        if ($missing.Count -eq 0) { break }
    }
}
finally {
    if (-not $qemu.HasExited) { Stop-Process -Id $qemu.Id -Force }
}

if ($missing.Count -eq 0) {
    Write-Host "BOOT TEST PASS: greeting, breakpoint recovery, and heartbeat all observed." -ForegroundColor Green
    exit 0
} else {
    Write-Host "BOOT TEST FAIL: missing within 30s: $($missing -join ', '). Serial output:" -ForegroundColor Red
    Read-LogText $serialLog | Write-Host
    exit 1
}
```

- [ ] **Step 2: Run the test to verify it fails for the right reason**

Run: `./tools/test-qemu.ps1`
Expected: FAIL listing `trap: breakpoint, survived breakpoint, tick: 2` as missing (NOT `hello world` — that still passes from Phase 1).

- [ ] **Step 3: Commit**

```bash
git add tools/test-qemu.ps1
git commit -m "test: extend QEMU smoke test for Phase 2a trap recovery and heartbeat (failing)"
```

---

### Task 2: `scause` decoding and instruction length (pure, host-tested)

The pure parts of `trap.rs`: the `TrapFrame` layout, the `Cause` enum, `decode`, and `instruction_len`. No CSR access — these compile and test on the host, like the existing `arch_is_riscv64` test.

**Files:**
- Create: `arch/riscv64/src/trap.rs`
- Modify: `arch/riscv64/src/lib.rs`
- Test: host `#[cfg(test)]` module inside `trap.rs`

- [ ] **Step 1: Create `trap.rs` with failing tests and stub types**

Create `arch/riscv64/src/trap.rs`:

```rust
//! Trap handling: the kernel's reflexes.
//!
//! A *trap* is the hart's reaction to an exceptional event — either an
//! **exception** (synchronous, caused by the current instruction, e.g.
//! `ebreak`) or an **interrupt** (asynchronous, e.g. the timer). The CPU
//! jumps to the address in `stvec`, and `scause` says why.
//!
//! Pure decoding logic lives ungated in this module so it tests on the
//! host. The assembly entry, the dispatcher, and `init()` are gated to
//! `target_arch = "riscv64"` (added in a later task).

/// Snapshot of the interrupted hart, pushed by the trap entry assembly
/// and restored on the way out. Full (all 31 GPRs) rather than
/// caller-saved-only: this is exactly the structure context switching
/// (Phase 2c) needs, and saving it now avoids a rewrite.
///
/// Layout contract with the entry assembly: `regs[n-1]` holds `x_n`
/// at byte offset `(n-1) * 8`; then sepc, sstatus, scause, stval.
/// `x0` is hardwired to zero and not stored.
#[derive(Debug)]
#[repr(C)]
pub struct TrapFrame {
    /// General-purpose registers x1..=x31; `regs[n-1]` = `x_n`.
    pub regs: [usize; 31],
    /// PC of the trapping/interrupted instruction; `sret` resumes here.
    pub sepc: usize,
    /// Privilege/interrupt state at trap time; restored by `sret`.
    pub sstatus: usize,
    /// Why the trap happened (interrupt bit + cause code).
    pub scause: usize,
    /// Trap-specific extra value (e.g. the faulting address).
    pub stval: usize,
}

/// Decoded `scause`. Only the causes Phase 2a handles get variants;
/// everything else is `Unknown` and treated as fatal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    /// `ebreak`/`c.ebreak` executed (exception code 3).
    Breakpoint,
    /// Supervisor timer interrupt (interrupt code 5).
    SupervisorTimer,
    /// Anything we don't handle yet.
    Unknown { interrupt: bool, code: usize },
}

/// In `scause`, the top bit distinguishes interrupts from exceptions.
const INTERRUPT_BIT: usize = 1 << (usize::BITS - 1);

/// Decode a raw `scause` value.
pub fn decode(scause: usize) -> Cause {
    todo!()
}

/// Length in bytes of the instruction starting with this 16-bit parcel.
/// RISC-V encoding rule: standard 4-byte instructions have the two low
/// bits `11`; compressed (C-extension) 2-byte instructions do not.
pub fn instruction_len(parcel: u16) -> usize {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_breakpoint_exception() {
        assert_eq!(decode(3), Cause::Breakpoint);
    }

    #[test]
    fn decodes_supervisor_timer_interrupt() {
        assert_eq!(decode(INTERRUPT_BIT | 5), Cause::SupervisorTimer);
    }

    #[test]
    fn unknown_exception_is_not_fatal_to_decode() {
        // Exception code 2 = illegal instruction; unhandled in 2a.
        assert_eq!(decode(2), Cause::Unknown { interrupt: false, code: 2 });
    }

    #[test]
    fn unknown_interrupt_keeps_interrupt_flag() {
        // Interrupt code 9 = supervisor external; unhandled in 2a.
        assert_eq!(
            decode(INTERRUPT_BIT | 9),
            Cause::Unknown { interrupt: true, code: 9 }
        );
    }

    #[test]
    fn ebreak_is_four_bytes() {
        // ebreak = 0x00100073; its low parcel 0x0073 ends in 0b11.
        assert_eq!(instruction_len(0x0073), 4);
    }

    #[test]
    fn compressed_ebreak_is_two_bytes() {
        // c.ebreak = 0x9002; ends in 0b10.
        assert_eq!(instruction_len(0x9002), 2);
    }

    #[test]
    fn trap_frame_layout_matches_entry_asm() {
        // The entry assembly allocates 288 bytes (280 rounded up to 16)
        // and stores stval at offset 272. If this changes, trap.rs's
        // assembly (added later) must change with it.
        assert_eq!(core::mem::size_of::<TrapFrame>(), 280);
    }
}
```

In `arch/riscv64/src/lib.rs`, add after the gated `sbi` module declaration:

```rust
pub mod trap;
```

(Ungated on purpose — the pure parts must compile on the host.)

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p kernel-arch-riscv64`
Expected: FAIL — the decode/length tests panic with "not yet implemented" (`todo!`). `trap_frame_layout_matches_entry_asm` passes already; that's fine.

- [ ] **Step 3: Implement `decode` and `instruction_len`**

Replace the two `todo!()` bodies:

```rust
pub fn decode(scause: usize) -> Cause {
    let interrupt = scause & INTERRUPT_BIT != 0;
    let code = scause & !INTERRUPT_BIT;
    match (interrupt, code) {
        (false, 3) => Cause::Breakpoint,
        (true, 5) => Cause::SupervisorTimer,
        _ => Cause::Unknown { interrupt, code },
    }
}
```

```rust
pub fn instruction_len(parcel: u16) -> usize {
    if parcel & 0b11 == 0b11 { 4 } else { 2 }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p kernel-arch-riscv64`
Expected: PASS — all 7 tests green (plus the pre-existing `arch_is_riscv64`).

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/trap.rs arch/riscv64/src/lib.rs
git commit -m "feat(arch): trap frame, scause decoding, instruction-length detection"
```

---

### Task 3: CSR accessors and SBI `set_timer`

Hand-rolled inline-asm wrappers. No host tests possible (riscv64-only instructions); verified by cross-build now and live behavior in Tasks 4–5.

**Files:**
- Create: `arch/riscv64/src/csr.rs`
- Modify: `arch/riscv64/src/sbi.rs`
- Modify: `arch/riscv64/src/lib.rs`

- [ ] **Step 1: Create `csr.rs`**

```rust
//! Control and Status Register (CSR) accessors, hand-rolled.
//!
//! CSRs are per-hart special registers read/written with dedicated
//! instructions (`csrr`, `csrw`, `csrs`). Only the ones Phase 2a
//! actually calls get accessors; the trap entry assembly reads
//! `sepc`/`scause`/`stval` directly into the [`crate::trap::TrapFrame`].

use core::arch::asm;

/// Read the `time` CSR: a wall-clock counter ticking at the platform's
/// timebase frequency (10 MHz on QEMU virt), independent of CPU speed.
#[inline]
pub fn time() -> u64 {
    let value: u64;
    unsafe { asm!("csrr {}, time", out(reg) value) };
    value
}

/// Install the trap vector: all traps jump to `addr`.
///
/// The low two bits select the mode; `00` = direct (one entry for every
/// trap), which `.align 2` on the entry symbol guarantees.
///
/// # Safety
/// `addr` must be the 4-byte-aligned address of a real trap entry that
/// saves/restores state and ends in `sret`. A bogus value turns every
/// trap into a wild jump.
#[inline]
pub unsafe fn stvec_write(addr: usize) {
    unsafe { asm!("csrw stvec, {}", in(reg) addr) };
}

/// Enable supervisor timer interrupts (`sie.STIE`, bit 5).
///
/// # Safety
/// A trap handler must be installed via [`stvec_write`] first.
#[inline]
pub unsafe fn sie_enable_timer() {
    unsafe { asm!("csrs sie, {}", in(reg) 1usize << 5) };
}

/// Globally enable supervisor interrupts (`sstatus.SIE`, bit 1).
///
/// # Safety
/// A trap handler must be installed via [`stvec_write`] first.
#[inline]
pub unsafe fn sstatus_enable_interrupts() {
    unsafe { asm!("csrsi sstatus, 0x2") };
}
```

- [ ] **Step 2: Add the TIME-extension `set_timer` to `sbi.rs`**

Append to `arch/riscv64/src/sbi.rs`:

```rust
/// Program the next timer interrupt (SBI TIME extension, EID
/// 0x54494D45 = "TIME", FID 0).
///
/// Unlike Phase 1's legacy console call, modern SBI extensions take the
/// extension id in `a7` *and* a function id in `a6` — this is the SBI
/// v2 calling convention. The timer is **one-shot**: firing at absolute
/// time `stime_value` (in `time`-CSR ticks) also clears the pending
/// bit, and the handler must call this again for the next tick.
pub fn set_timer(stime_value: u64) {
    unsafe {
        asm!(
            "ecall",
            in("a7") 0x5449_4D45usize,
            in("a6") 0usize,
            inout("a0") stime_value as usize => _, // in: deadline; out: SBI error code (ignored)
            out("a1") _,
        );
    }
}
```

Also update the module doc comment's third paragraph (lines 8–11) to reflect that we now use a modern extension too:

```rust
//! Phase 1 needed exactly one call: legacy `console_putchar` (EID 0x01),
//! deprecated but universally supported (the DBCN replacement can land
//! in a later phase). Phase 2a adds the modern TIME extension —
//! see [`set_timer`] for the v2 calling convention.
```

- [ ] **Step 3: Wire `csr` into `lib.rs`**

In `arch/riscv64/src/lib.rs`, next to the gated `console`/`sbi` declarations, add:

```rust
#[cfg(target_arch = "riscv64")]
pub mod csr;
```

- [ ] **Step 4: Verify both builds stay green**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf && cargo test -p kernel-arch-riscv64`
Expected: cross-build compiles the new asm; host tests still all pass.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/csr.rs arch/riscv64/src/sbi.rs arch/riscv64/src/lib.rs
git commit -m "feat(arch): CSR accessors and SBI TIME-extension set_timer"
```

---

### Task 4: Trap entry assembly, dispatcher, and breakpoint recovery

The heart of the phase: the assembly entry/exit, the Rust dispatcher, and kmain proving recovery from a deliberate `ebreak`. The timer arm of the dispatcher arrives in Task 5 (its target doesn't exist yet).

**Files:**
- Modify: `arch/riscv64/src/trap.rs` (append the gated section)
- Modify: `kernel/src/main.rs`

- [ ] **Step 1: Append the gated entry/dispatcher section to `trap.rs`**

Append to `arch/riscv64/src/trap.rs`:

```rust
/// The assembly trap entry. Layout contract: see [`TrapFrame`].
/// 288 = size_of::<TrapFrame>() (280) rounded up to keep `sp` 16-aligned
/// per the RISC-V ABI. `t0` (= x5) is used as scratch only *after* its
/// slot is saved, and on the way out only *before* its slot is restored.
/// `x2` (sp) is restored last — that load releases the frame.
#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(
    r#"
    .section .text
    .align 2                # stvec requires 4-byte alignment (mode bits = 00, direct)
    .global __trap_entry
__trap_entry:
    addi sp, sp, -288
    sd x1,  0(sp)
    sd x3,  16(sp)
    sd x4,  24(sp)
    sd x5,  32(sp)
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
    addi t0, sp, 288        # reconstruct the pre-trap sp (x2)
    sd t0, 8(sp)
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
    ld t0, 256(sp)
    csrw sstatus, t0
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
    ld x2,  8(sp)           # restore original sp LAST; frame is gone
    sret
"#
);

/// Install [`__trap_entry`] as the trap vector (direct mode). Call once,
/// early in kmain, before anything can fault and before interrupts are
/// enabled.
#[cfg(target_arch = "riscv64")]
pub fn init() {
    extern "C" {
        fn __trap_entry();
    }
    // SAFETY: __trap_entry is the real entry defined above; .align 2
    // gives the required 4-byte alignment.
    unsafe { crate::csr::stvec_write(__trap_entry as usize) };
}

/// Length of the instruction at `addr`, for advancing `sepc` past it.
#[cfg(target_arch = "riscv64")]
fn instruction_len_at(addr: usize) -> usize {
    // SAFETY: addr is the sepc of a just-executed instruction, so it
    // points at readable, identity-mapped kernel code (no paging yet).
    let parcel = unsafe { core::ptr::read_volatile(addr as *const u16) };
    instruction_len(parcel)
}

/// Rust side of every trap; called by the entry assembly with the saved
/// frame. Returning resumes at `frame.sepc` via `sret`.
#[cfg(target_arch = "riscv64")]
#[no_mangle]
extern "C" fn trap_handler(frame: &mut TrapFrame) {
    match decode(frame.scause) {
        Cause::Breakpoint => {
            crate::println!("trap: breakpoint at {:#x}", frame.sepc);
            // ebreak doesn't advance the PC itself; without this, sret
            // would re-execute it forever.
            frame.sepc += instruction_len_at(frame.sepc);
        }
        Cause::SupervisorTimer => {
            // Wired to the timer in the next task; interrupts are not
            // enabled yet, so this is unreachable today.
            panic!("timer interrupt before timer support exists");
        }
        Cause::Unknown { interrupt, code } => {
            crate::println!(
                "FATAL TRAP: interrupt={interrupt} code={code} sepc={:#x} stval={:#x}",
                frame.sepc, frame.stval
            );
            crate::println!("{frame:#x?}");
            panic!("unhandled trap");
        }
    }
}
```

- [ ] **Step 2: Exercise it from kmain**

In `kernel/src/main.rs`, inside `mod bare`:

Change the `use` line:

```rust
    use kernel_arch_riscv64::{println, trap};
```

Replace the `kmain` body:

```rust
    #[no_mangle]
    extern "C" fn kmain(hartid: usize, _dtb: usize) -> ! {
        println!();
        println!("{GREETING} from {PROJECT_NAME} - Phase 2a (hart {hartid})");

        trap::init();
        // Deliberate breakpoint: proves the handler catches an exception
        // and execution RESUMES past it (the smoke test's
        // "survived breakpoint" line can only print if recovery worked).
        unsafe { core::arch::asm!("ebreak") };
        println!("survived breakpoint");

        println!("(kernel is idle; exit QEMU with Ctrl-A then X)");
        park()
    }
```

- [ ] **Step 3: Build and observe the breakpoint recovery live**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf && ./tools/test-qemu.ps1`
Expected: still FAIL overall (no ticks yet), but the dumped serial output must now contain `trap: breakpoint at 0x80200...` followed by `survived breakpoint`. If instead the breakpoint line repeats forever, the sepc advance is broken; if QEMU shows nothing after the greeting, the entry/exit assembly is broken — fix before proceeding.

- [ ] **Step 4: Verify host tests still pass**

Run: `cargo test -p kernel-arch-riscv64 && cargo test -p kernel`
Expected: PASS (gated code is invisible to the host).

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/trap.rs kernel/src/main.rs
git commit -m "feat(arch): direct-mode trap entry, dispatcher, breakpoint recovery"
```

---

### Task 5: Timer heartbeat

One-shot SBI timer re-armed from the handler; ~1 Hz "tick: N" heartbeat. This completes the smoke test.

**Files:**
- Create: `arch/riscv64/src/timer.rs`
- Modify: `arch/riscv64/src/lib.rs`
- Modify: `arch/riscv64/src/trap.rs` (dispatcher arm)
- Modify: `kernel/src/main.rs`

- [ ] **Step 1: Create `timer.rs`**

```rust
//! Timer heartbeat via the SBI TIME extension.
//!
//! SBI timers are one-shot: each interrupt must arm the next one, so
//! the "periodic" tick is really a chain of deadlines computed from the
//! `time` CSR.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::{csr, sbi};

/// The `time` CSR's tick rate on QEMU virt: 10 MHz. **QEMU-specific
/// constant** — real hardware (Phase 4) must read the timebase from the
/// device tree instead.
const TIMEBASE_HZ: u64 = 10_000_000;

/// One heartbeat per second (in `time` ticks).
const TICK_INTERVAL: u64 = TIMEBASE_HZ;

/// Monotonic count of timer interrupts since boot.
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Arm the first tick and enable timer interrupts.
/// [`crate::trap::init`] must have been called first — enabling
/// interrupts with no handler installed turns the first tick into a
/// wild jump.
pub fn start() {
    arm_next();
    // SAFETY: the caller contract above is exactly the safety condition
    // of these two CSR writes.
    unsafe {
        csr::sie_enable_timer();
        csr::sstatus_enable_interrupts();
    }
}

/// Called by the trap dispatcher on each supervisor timer interrupt.
pub(crate) fn on_tick() {
    let n = TICKS.fetch_add(1, Ordering::Relaxed) + 1;
    crate::println!("tick: {n}");
    arm_next();
}

/// Schedule the next interrupt one interval from now.
fn arm_next() {
    sbi::set_timer(csr::time() + TICK_INTERVAL);
}
```

- [ ] **Step 2: Wire into `lib.rs`**

Next to the gated `csr` declaration in `arch/riscv64/src/lib.rs`, add:

```rust
#[cfg(target_arch = "riscv64")]
pub mod timer;
```

- [ ] **Step 3: Point the dispatcher at it**

In `trap.rs`'s `trap_handler`, replace the `Cause::SupervisorTimer` arm:

```rust
        Cause::SupervisorTimer => crate::timer::on_tick(),
```

- [ ] **Step 4: Start the heartbeat from kmain**

In `kernel/src/main.rs`:

```rust
    use kernel_arch_riscv64::{println, timer, trap};
```

and in `kmain`, replace the final two lines (`println!("(kernel is idle..."` and `park()`) with:

```rust
        timer::start();
        println!("(kernel idles; heartbeat ~1/s; exit QEMU with Ctrl-A then X)");
        park()
```

(`park`'s `wfi` now actually wakes once per second, handles the tick, and goes back to sleep — the comment on `park` saying "none are enabled yet" is now stale; update it to `// the trap handler runs on each timer tick, then we sleep again.`)

- [ ] **Step 5: Run the full smoke test**

Run: `./tools/test-qemu.ps1`
Expected: **PASS** — "BOOT TEST PASS: greeting, breakpoint recovery, and heartbeat all observed." (needs ~3s of QEMU runtime for two ticks; the 30s deadline is ample).

- [ ] **Step 6: Run all host checks**

Run: `cargo build && cargo test`
Expected: PASS across the workspace.

- [ ] **Step 7: Commit**

```bash
git add arch/riscv64/src/timer.rs arch/riscv64/src/lib.rs arch/riscv64/src/trap.rs kernel/src/main.rs
git commit -m "feat(arch): SBI timer heartbeat - Phase 2a smoke test passes"
```

---

### Task 6: Documentation (learning note, glossary, roadmap)

**Files:**
- Create: `docs/learning/0003-traps-and-interrupts.md`
- Modify: `docs/glossary.md`
- Modify: `docs/roadmap/roadmap.md`

- [ ] **Step 1: Write the learning note**

Create `docs/learning/0003-traps-and-interrupts.md`. Use this draft, **correcting anything that turned out differently during implementation** (per the project rule: open and verify everything you cite):

```markdown
# 0003 — Traps and the timer heartbeat (Phase 2a)

How the kernel got its reflexes: catching exceptions, surviving them,
and waking up once a second.

## Traps: one mechanism, two flavors

A **trap** is the hart's reaction to an exceptional event: stop what
you're doing, jump to the address in `stvec`, and let the kernel sort
it out. Two flavors share that one mechanism:

- **Exceptions** are *synchronous* — caused by the instruction itself
  (our deliberate `ebreak`, an illegal instruction, later a page fault).
- **Interrupts** are *asynchronous* — they arrive from outside, like the
  timer saying "your deadline passed".

`scause` tells them apart: the top bit is 1 for interrupts, and the rest
is a cause code (3 = breakpoint, interrupt 5 = supervisor timer).

What clicked: this is the same `ecall`-shaped machinery from Phase 1,
pointed the other way. Phase 1 we *made* traps into OpenSBI below us;
now we *receive* traps from our own code (and later, from user programs
above us — that's what a syscall is).

## Why the entry is assembly again

The handler interrupts code mid-thought. Rust would freely overwrite
registers the interrupted code still needs, so the first thing that runs
is assembly that saves **all 31** general-purpose registers plus
`sepc`/`sstatus`/`scause`/`stval` into a `TrapFrame` on the stack, and
the last thing is assembly restoring them and executing `sret`. We save
the *full* set (not just caller-saved) because a saved-everything frame
is precisely a suspended task — Phase 2c's context switch will reuse it.

Subtleties that bit during review: the original `sp` has to be
reconstructed (`sp + frame size`) because the entry already moved it,
and it must be restored *last* — the moment `sp` changes, the frame is
conceptually freed.

## Recovering from a breakpoint

`ebreak` does not advance the PC: `sepc` points *at* the breakpoint, and
a bare `sret` would re-execute it forever. The handler advances `sepc`
past it first — by 4 bytes normally, but riscv64**gc** includes the
compressed extension, so `c.ebreak` is only 2. The encoding rule: a
16-bit parcel ending in `0b11` starts a 4-byte instruction; anything
else is compressed. The "survived breakpoint" line in the boot output
exists purely to prove the resume worked.

## The timer: one-shot by design

SBI timers aren't periodic. `sbi::set_timer(deadline)` fires *once*; the
handler re-arms the next deadline (`time` CSR + interval). The `time`
CSR ticks at the platform **timebase** — 10 MHz on QEMU virt, hardcoded
as a documented constant until Phase 4 reads it from the device tree.

Enabling order matters and is one-way: install `stvec` first, *then*
set `sie.STIE` and `sstatus.SIE`. Enable interrupts with no handler and
the first tick is a wild jump.

The pleasing part: `kmain` still ends in the same `wfi` park loop from
Phase 1 — but now the hart genuinely sleeps, wakes once a second,
handles the tick, and sleeps again. The kernel has a pulse.

## Crash diagnostics for free

Any trap we don't recognize prints the decoded cause, `sepc`, `stval`,
and the full register dump, then panics. This is the project's first
real diagnostic surface — every future fault lands here until it gets
its own handler.

## Day-to-day commands

Unchanged: `./tools/run-qemu.ps1` to watch it live (you'll see the tick
lines), `./tools/test-qemu.ps1` for the automated check.
```

- [ ] **Step 2: Add glossary entries**

Append to the list in `docs/glossary.md` (keeping its one-line, plain-language style):

```markdown
- **Trap** — the CPU's "stop and handle this" mechanism: on an exceptional event the hart jumps to the kernel's registered handler. Exceptions and interrupts are the two flavors.
- **Exception** — a synchronous trap caused by the current instruction itself (e.g. a breakpoint or an illegal instruction).
- **Interrupt** — an asynchronous trap arriving from outside the instruction stream (e.g. the timer). The kernel handles it and resumes the interrupted code.
- **CSR (Control and Status Register)** — special per-hart registers that configure and report CPU state, accessed with dedicated instructions (`csrr`/`csrw`). Trap handling lives in CSRs like `stvec` (handler address), `scause` (why), `sepc` (where), `stval` (extra detail).
- **Trap frame** — the snapshot of every register saved on trap entry and restored on exit, so the interrupted code never notices. A saved frame is effectively a paused task — the seed of context switching.
- **`sret`** — the return-from-trap instruction: restores the pre-trap privilege mode and jumps back to `sepc`.
- **`ebreak`** — the RISC-V breakpoint instruction; Phase 2a triggers one on purpose to prove trap recovery works.
- **Timebase** — the fixed rate the `time` counter ticks at (10 MHz on QEMU virt), independent of CPU clock speed; deadlines for timer interrupts are expressed in these ticks.
```

- [ ] **Step 3: Record the decomposition in the roadmap**

In `docs/roadmap/roadmap.md`, replace the entire `## Phase 2 — The kernel grows up` section (keeping Phases 3+ untouched) with:

```markdown
## Phase 2 — The kernel grows up

Decomposed into three sub-phases (2026-06-10), each with its own design → plan → build cycle. Traps come first because timer interrupts (2c) and page faults (2b) both need the handler infrastructure.

### Phase 2a — Trap handling & timer heartbeat  *(done — 2026-06-10)*

- **Goal:** a supervisor trap handler that catches and recovers from exceptions, plus SBI timer interrupts producing a ~1 Hz heartbeat.
- **You learn:** the trap CSRs (`stvec`, `scause`, `sepc`, …), trap entry/exit and context saving, the SBI TIME extension ([learning note 0003](../learning/0003-traps-and-interrupts.md)).
- **Done when:** `./tools/test-qemu.ps1` sees a survived breakpoint and ≥ 2 timer ticks in one boot.

### Phase 2b — Memory management

- **Goal:** physical frame allocation and virtual memory (paging) — the kernel manages its own address space.
- **You learn:** physical vs. virtual addresses, page tables, the MMU.
- **Done when:** the kernel runs with paging enabled and can allocate/free frames.

### Phase 2c — Basic scheduling

- **Goal:** context switching between simple in-kernel tasks, driven by the timer from 2a.
- **You learn:** context switching, run queues, the tick-policy hook.
- **Done when:** the kernel switches between simple tasks (the original Phase 2 exit criterion).
```

(Adjust the 2a status/date to reality when committing.)

- [ ] **Step 4: Verify cited paths exist**

Run: `./tools/check-references.ps1`
Expected: PASS — every path/anchor cited in the new docs resolves.

- [ ] **Step 5: Commit**

```bash
git add docs/learning/0003-traps-and-interrupts.md docs/glossary.md docs/roadmap/roadmap.md
git commit -m "docs: traps learning note, glossary terms, roadmap 2a/2b/2c split"
```

---

### Task 7: Final verification

- [ ] **Step 1: Full check suite from a clean slate**

Run, in order:

```powershell
cargo build
cargo test
cargo build -p kernel --target riscv64gc-unknown-none-elf
./tools/test-qemu.ps1
./tools/check-references.ps1
```

Expected: every command exits 0; the smoke test prints PASS.

- [ ] **Step 2: Review the diff against the spec**

Skim `git log --oneline main` and the spec's §5 deliverables list — all six items must be ticked off by Tasks 1–6. If anything is missing, fix it now, not in a follow-up.

- [ ] **Step 3: Commit any stragglers**

Only if Step 2 surfaced fixes:

```bash
git add -A
git commit -m "fix: close Phase 2a spec-coverage gaps"
```
