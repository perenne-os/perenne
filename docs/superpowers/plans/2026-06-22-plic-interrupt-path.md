# PLIC interrupt path Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline, per the user's preference for this project) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Drive the PLIC and handle a supervisor external interrupt so the U-mode virtio-rng entropy component blocks on its device's interrupt (capability-gated `wait_irq`) instead of polling the virtqueue used ring.

**Architecture:** The kernel owns the PLIC (claim/complete, per-context enable). On the virtio-rng IRQ the handler claims, **masks** the source, wakes the bound task, and completes; the driver then acks the device and loops. The component holds an `Interrupt(irq)` capability and calls `wait_irq` (which unmasks + blocks).

**Tech Stack:** Rust `no_std`, RISC-V (riscv64gc), QEMU. Host tests: `cargo test -p kernel-arch-riscv64`. Bare: `cargo build -p kernel --target riscv64gc-unknown-none-elf`.

**Spec:** `docs/superpowers/specs/2026-06-22-plic-interrupt-path-design.md`

**SPIKE-VERIFIED facts:** PLIC base `0x0c00_0000`; hart 0 S-mode **context 1**; priority `base+irq*4`, enable `base+0x2000+ctx*0x80+(irq/32)*4`, threshold `base+0x200000+ctx*0x1000`, claim/complete `base+0x200004+ctx*0x1000`. For IRQ 8/ctx 1: `+0x20`, `+0x2080` bit 8, `+0x201000`, `+0x201004`. RNG (slot `0x10008000`) = **IRQ 8** (DTB `interrupts`, slot index + 1). virtio `InterruptStatus`=`0x060`, `InterruptACK`=`0x064` (bit 0 = used-buffer).

**Existing-code facts:** `dt::MachineInfo` has `virtio_mmio:[usize;8]`/`virtio_mmio_count` and is built only in `dt::parse`. `csr` has `pub unsafe fn sie_enable_timer()` (`csrs sie, 1<<5`). `mem::init(ram_end, uart_base)` stores `UART_MMIO_BASE` and `map_kernel_sections` maps the UART page R-W-G; called in `kmain` as `mem::init(machine.ram_base + machine.ram_size, machine.uart_base)`. `trap::decode` maps `(true,5)→SupervisorTimer`, `_→Unknown`; a test `unknown_interrupt_keeps_interrupt_flag` asserts `decode(INTERRUPT_BIT|9)==Unknown{true,9}`. `sched` has `park_current(state)`, `find_blocked`, `SCHED`. `Syscall` ends `…Reply, Getrandom, Unknown`; decode maps 1..9. The entropy component polls `loop { dma_fence(); if dma_r16(used+2)==idx { break } }`; it holds `Endpoint(ENTROPY_EP)` at cap slot 0 (`ENTROPY_CAP`). `kmain` discovers `rng_base = virtio::find_rng(&machine.virtio_mmio[..n])` (MMU off) and spawns the entropy component inside `if let Some(rng) = rng_base { … }`.

---

## Task 1: discover the PLIC base + virtio IRQs

**Files:**
- Modify: `arch/riscv64/src/dt.rs`
- Modify: `arch/riscv64/src/virtio.rs` (the `irq_for_base` helper)

- [ ] **Step 1: Extend the fixture test (failing)**

In `arch/riscv64/src/dt.rs`, add to `parses_qemu_virt` (after the virtio_mmio asserts):

```rust
        assert_eq!(mi.plic_base, 0x0c00_0000, "PLIC base");
        assert!(mi.virtio_mmio_irq[..mi.virtio_mmio_count].contains(&8), "the 0x10008000 slot is IRQ 8");
```

In `arch/riscv64/src/virtio.rs`, add to its `tests` module:

```rust
    #[test]
    fn irq_for_base_maps_a_slot_to_its_irq() {
        let bases = [0x1000_1000usize, 0x1000_8000];
        let irqs = [1u32, 8];
        assert_eq!(irq_for_base(&bases, &irqs, 0x1000_8000), Some(8));
        assert_eq!(irq_for_base(&bases, &irqs, 0xdead), None);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p kernel-arch-riscv64 dt:: virtio::`
Expected: FAIL — `plic_base`/`virtio_mmio_irq`/`irq_for_base` do not exist.

- [ ] **Step 3: Add the fields + parsing in `dt.rs`**

Add to `MachineInfo` (after `virtio_mmio_count`):

```rust
    pub virtio_mmio_count: usize,
    /// Each virtio-mmio slot's IRQ (`interrupts`), parallel to `virtio_mmio`.
    pub virtio_mmio_irq: [u32; 8],
    /// The PLIC base (`riscv,plic0` node's `reg`).
    pub plic_base: usize,
```

In `parse`, add accumulators near the virtio ones:

```rust
    let mut node_is_virtio = false;
    let mut virtio_mmio = [0usize; 8];
    let mut virtio_count = 0usize;
    let mut virtio_mmio_irq = [0u32; 8];
    let mut node_virtio_irq: Option<u32> = None;
    let mut node_is_plic = false;
    let mut plic_base: usize = 0;
```

In `FDT_BEGIN_NODE`, reset the per-node flags:

```rust
                node_is_virtio = false;
                node_virtio_irq = None;
                node_is_plic = false;
```

In `FDT_END_NODE`, commit (replace the existing virtio commit block):

```rust
                if node_is_virtio {
                    if let Some(b) = node_reg {
                        if virtio_count < virtio_mmio.len() {
                            virtio_mmio[virtio_count] = b;
                            virtio_mmio_irq[virtio_count] = node_virtio_irq.unwrap_or(0);
                            virtio_count += 1;
                        }
                    }
                }
                if node_is_plic {
                    if let Some(b) = node_reg {
                        plic_base = b;
                    }
                }
```

In `FDT_PROP`, after the `virtio,mmio` compatible check, add the `interrupts` read and the PLIC match:

```rust
                if pname == b"interrupts" && len >= 4 {
                    node_virtio_irq = Some(be_u32(val, 0)?);
                }
                if pname == b"compatible" && val.windows(11).any(|w| w == b"riscv,plic0") {
                    node_is_plic = true;
                }
```

Add the fields to the returned struct:

```rust
        virtio_mmio_count: virtio_count,
        virtio_mmio_irq,
        plic_base,
    })
```

- [ ] **Step 4: Add `irq_for_base` in `virtio.rs`**

In `arch/riscv64/src/virtio.rs`, add after `is_rng`:

```rust
/// Map a discovered virtio-mmio base to its IRQ, using the parallel base/irq
/// arrays from the device tree. `None` if the base is not among them.
pub fn irq_for_base(bases: &[usize], irqs: &[u32], base: usize) -> Option<u32> {
    bases.iter().position(|&b| b == base).and_then(|i| irqs.get(i).copied())
}
```

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p kernel-arch-riscv64 dt:: virtio::`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add arch/riscv64/src/dt.rs arch/riscv64/src/virtio.rs
git commit -m "feat(dt): discover the PLIC base and virtio-mmio IRQs"
```

---

## Task 2: the PLIC driver + external-interrupt enable

**Files:**
- Create: `arch/riscv64/src/plic.rs`
- Modify: `arch/riscv64/src/lib.rs`
- Modify: `arch/riscv64/src/csr.rs`

- [ ] **Step 1: Create `arch/riscv64/src/plic.rs`**

```rust
//! The RISC-V PLIC (platform-level interrupt controller) — routes device
//! interrupts to the kernel. Pure offset arithmetic is host-tested; the gated
//! functions access the PLIC MMIO (mapped by `mem::init` into every tree).
//!
//! We target hart 0's S-mode context (context 1 on QEMU `virt`). The source
//! enable bit is toggled per-IRQ: `wait_irq` unmasks, the handler masks on
//! claim (the device line stays asserted until the U-mode driver acks it).

/// hart 0's S-mode interrupt context on QEMU `virt`.
pub const CONTEXT: usize = 1;

/// Byte offset of source `irq`'s priority register.
pub const fn priority_offset(irq: u32) -> usize {
    irq as usize * 4
}
/// Byte offset of the enable *word* holding `irq`'s bit for `ctx`.
pub const fn enable_offset(ctx: usize, irq: u32) -> usize {
    0x2000 + ctx * 0x80 + (irq as usize / 32) * 4
}
/// Byte offset of `ctx`'s priority threshold register.
pub const fn threshold_offset(ctx: usize) -> usize {
    0x20_0000 + ctx * 0x1000
}
/// Byte offset of `ctx`'s claim/complete register.
pub const fn claim_offset(ctx: usize) -> usize {
    0x20_0004 + ctx * 0x1000
}

#[cfg(target_arch = "riscv64")]
use core::sync::atomic::{AtomicUsize, Ordering};

#[cfg(target_arch = "riscv64")]
static PLIC_BASE: AtomicUsize = AtomicUsize::new(0);

#[cfg(target_arch = "riscv64")]
fn base() -> usize {
    PLIC_BASE.load(Ordering::Acquire)
}
// SAFETY (all): the PLIC base is mapped R-W into every address space by
// `mem::init`/`map_kernel_sections` before any of these run, and `init` has
// stored a non-zero base.
#[cfg(target_arch = "riscv64")]
unsafe fn r(off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((base() + off) as *const u32) }
}
#[cfg(target_arch = "riscv64")]
unsafe fn w(off: usize, v: u32) {
    unsafe { core::ptr::write_volatile((base() + off) as *mut u32, v) };
}

/// Record the PLIC base and accept any priority on our context (threshold 0).
/// Leaves all sources disabled (the enable bit is managed by `enable`/`disable`).
#[cfg(target_arch = "riscv64")]
pub fn init(plic_base: usize) {
    PLIC_BASE.store(plic_base, Ordering::Release);
    unsafe { w(threshold_offset(CONTEXT), 0) };
}

/// Give `irq` a non-zero priority so it can be delivered.
#[cfg(target_arch = "riscv64")]
pub fn set_priority(irq: u32, priority: u32) {
    unsafe { w(priority_offset(irq), priority) };
}

/// Unmask `irq` for our context.
#[cfg(target_arch = "riscv64")]
pub fn enable(irq: u32) {
    let off = enable_offset(CONTEXT, irq);
    unsafe { w(off, r(off) | (1 << (irq % 32))) };
}

/// Mask `irq` for our context.
#[cfg(target_arch = "riscv64")]
pub fn disable(irq: u32) {
    let off = enable_offset(CONTEXT, irq);
    unsafe { w(off, r(off) & !(1 << (irq % 32))) };
}

/// Claim the highest-priority pending interrupt for our context (0 = none).
#[cfg(target_arch = "riscv64")]
pub fn claim() -> u32 {
    unsafe { r(claim_offset(CONTEXT)) }
}

/// Signal completion of `irq` to the PLIC.
#[cfg(target_arch = "riscv64")]
pub fn complete(irq: u32) {
    unsafe { w(claim_offset(CONTEXT), irq) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_match_the_spike_verified_layout() {
        assert_eq!(priority_offset(8), 0x20);
        assert_eq!(enable_offset(1, 8), 0x2080);
        assert_eq!(threshold_offset(1), 0x20_1000);
        assert_eq!(claim_offset(1), 0x20_1004);
    }
}
```

- [ ] **Step 2: Declare the module in `lib.rs`**

In `arch/riscv64/src/lib.rs`, add after the `pub mod entropy;` declaration:

```rust
/// The PLIC interrupt controller: pure offset arithmetic (host-tested) + the
/// gated claim/complete/enable driver for the kernel's external interrupts.
pub mod plic;
```

- [ ] **Step 3: Add `sie_enable_external` in `csr.rs`**

In `arch/riscv64/src/csr.rs`, after `sie_enable_timer`:

```rust
/// Enable supervisor external interrupts (`sie.SEIE`, bit 9).
///
/// # Safety
/// Only call once a trap handler and the PLIC are set up to service them.
#[cfg(target_arch = "riscv64")]
pub unsafe fn sie_enable_external() {
    unsafe { asm!("csrs sie, {}", in(reg) 1usize << 9, options(nostack, nomem)) };
}
```

- [ ] **Step 4: Run tests + bare build**

Run: `cargo test -p kernel-arch-riscv64 plic::`
Expected: PASS — `offsets_match_the_spike_verified_layout`.
Run: `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf`
Expected: SUCCESS.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/plic.rs arch/riscv64/src/lib.rs arch/riscv64/src/csr.rs
git commit -m "feat(plic): PLIC driver (host-tested offsets) + sie external-interrupt enable"
```

---

## Task 3: map the PLIC into every address space

**Files:**
- Modify: `arch/riscv64/src/mem/mod.rs`
- Modify: `kernel/src/main.rs` (the `mem::init` call)

- [ ] **Step 1: Add a `PLIC_MMIO_BASE` static**

In `arch/riscv64/src/mem/mod.rs`, near `UART_MMIO_BASE`:

```rust
/// MMIO base of the PLIC (from the device tree), saved by [`init`] so
/// [`map_kernel_sections`] maps its pages into the master table and every
/// per-task tree — the external interrupt handler touches the PLIC while a
/// user task's `satp` is active. Zero before `init`.
#[cfg(target_arch = "riscv64")]
static PLIC_MMIO_BASE: AtomicUsize = AtomicUsize::new(0);
```

- [ ] **Step 2: Map the PLIC pages in `map_kernel_sections`**

In `map_kernel_sections`, after the UART mapping block (inside the `unsafe`):

```rust
        // The PLIC: priority/pending/enable around the base (3 pages) and the
        // context-1 threshold/claim page. R-W-G, kernel-only, in every tree.
        let plic = PLIC_MMIO_BASE.load(Ordering::Acquire);
        if plic != 0 {
            paging::map_range(root, plic, plic + 0x3000, PTE_R | PTE_W | PTE_G);
            paging::map_range(root, plic + 0x20_1000, plic + 0x20_2000, PTE_R | PTE_W | PTE_G);
        }
```

- [ ] **Step 3: Take and store the PLIC base in `init`**

Change `init`'s signature and store the base (before the page table is built):

```rust
pub fn init(ram_end: usize, uart_base: usize, plic_base: usize) {
    use paging::{PTE_G, PTE_R, PTE_W};
    // SAFETY: ... (unchanged)
    unsafe {
        UART_MMIO_BASE.store(uart_base, Ordering::Release);
        PLIC_MMIO_BASE.store(plic_base, Ordering::Release);
```

(Leave the rest of `init` unchanged.)

- [ ] **Step 4: Update the `kmain` call**

In `kernel/src/main.rs`, change the `mem::init` call:

```rust
        mem::init(machine.ram_base + machine.ram_size, machine.uart_base, machine.plic_base);
```

- [ ] **Step 5: Build (host + bare)**

Run: `cargo test -p kernel-arch-riscv64` → expect PASS.
Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf` → expect SUCCESS.

- [ ] **Step 6: Commit**

```bash
git add arch/riscv64/src/mem/mod.rs kernel/src/main.rs
git commit -m "feat(mem): map the PLIC into every address space"
```

---

## Task 4: the `Interrupt` capability

**Files:**
- Modify: `arch/riscv64/src/cap.rs`

- [ ] **Step 1: Write the failing test**

In `arch/riscv64/src/cap.rs`, add to `tests`:

```rust
    #[test]
    fn interrupt_irq_returns_the_irq() {
        let caps = [None, Some(Capability::Interrupt(8)), Some(Capability::Randomness)];
        assert_eq!(interrupt_irq(&caps, 1), Some(8));
        assert_eq!(interrupt_irq(&caps, 2), None, "Randomness is not an Interrupt cap");
        assert_eq!(interrupt_irq(&caps, 0), None, "empty slot");
        assert_eq!(interrupt_irq(&caps, 9), None, "out of range");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p kernel-arch-riscv64 cap::`
Expected: FAIL — `Capability::Interrupt`/`interrupt_irq` do not exist.

- [ ] **Step 3: Add the variant and the lookup**

In `arch/riscv64/src/cap.rs`, extend `Capability`:

```rust
    /// Authority to call `getrandom` — draw from the kernel entropy pool.
    Randomness,
    /// Authority to `wait_irq` on this IRQ number (a device's interrupt).
    Interrupt(u32),
}
```

Add, after `has_randomness`:

```rust
/// Return the IRQ a `Interrupt` capability at `idx` authorizes waiting on.
/// `None` for an empty slot, an out-of-range index, or the wrong cap type.
pub fn interrupt_irq(caps: &[Option<Capability>], idx: usize) -> Option<u32> {
    match caps.get(idx) {
        Some(Some(Capability::Interrupt(irq))) => Some(*irq),
        _ => None,
    }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p kernel-arch-riscv64 cap::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/cap.rs
git commit -m "feat(cap): Interrupt capability + interrupt_irq lookup (host-tested)"
```

---

## Task 5: the wait/wake mechanism in `task` + `sched`

**Files:**
- Modify: `arch/riscv64/src/task.rs`
- Modify: `arch/riscv64/src/sched.rs`

- [ ] **Step 1: Add `TaskState::WaitingIrq`**

In `arch/riscv64/src/task.rs`, extend `TaskState` (after `AwaitingReply`):

```rust
    /// A caller whose request a server has picked up [...]
    AwaitingReply,
    /// Blocked in `wait_irq` for external interrupt `irq`; `pick_next` skips
    /// it until the interrupt handler wakes it.
    WaitingIrq(u32),
}
```

- [ ] **Step 2: Add `wake_irq` and `wait_irq` in `sched.rs`**

In `arch/riscv64/src/sched.rs`, add after `getrandom`:

```rust
/// Wake the task blocked in `wait_irq` for `irq` (set it `Ready`) and return
/// its name (for logging). `None` if no task is waiting — the masked source
/// stays pending and is redelivered at the next `wait_irq`.
#[cfg(target_arch = "riscv64")]
pub fn wake_irq(irq: u32) -> Option<&'static str> {
    SCHED.with(|s| {
        let want = TaskState::WaitingIrq(irq);
        let pos = s
            .tasks
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|t| t.state == want))?;
        s.tasks[pos].as_mut().unwrap().state = TaskState::Ready;
        Some(s.tasks[pos].as_ref().unwrap().name)
    })
}

/// Service a `wait_irq` syscall: if the caller holds an `Interrupt` capability
/// at index `a0` (= `frame.regs[9]`), unmask that IRQ in the PLIC and block the
/// task until the interrupt handler wakes it (`a0 = 0` on return). Otherwise
/// `a0 = usize::MAX`. Runs in the trap handler with interrupts off, so the
/// just-unmasked interrupt is delivered only after we have blocked.
#[cfg(target_arch = "riscv64")]
pub fn wait_irq(frame: &mut crate::trap::TrapFrame) {
    let cap_idx = frame.regs[9];
    let irq = SCHED.with(|s| {
        crate::cap::interrupt_irq(&s.tasks[s.current].as_ref().unwrap().caps, cap_idx)
    });
    match irq {
        None => frame.regs[9] = usize::MAX,
        Some(irq) => {
            crate::plic::enable(irq);
            park_current(TaskState::WaitingIrq(irq));
            frame.regs[9] = 0; // woken by the interrupt handler
        }
    }
}
```

- [ ] **Step 3: Build (host + bare)**

Run: `cargo test -p kernel-arch-riscv64` → expect PASS.
Run: `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf` → expect SUCCESS.

- [ ] **Step 4: Commit**

```bash
git add arch/riscv64/src/task.rs arch/riscv64/src/sched.rs
git commit -m "feat(sched): WaitingIrq state + wait_irq (unmask+block) + wake_irq"
```

---

## Task 6: handle the supervisor external interrupt

**Files:**
- Modify: `arch/riscv64/src/trap.rs`

- [ ] **Step 1: Add the `Cause` variant + decode, and fix the existing test**

In `arch/riscv64/src/trap.rs`, add to `Cause` (after `UserEcall`):

```rust
    /// `ecall` executed from U-mode (exception code 8) — a syscall.
    UserEcall,
    /// Supervisor external interrupt (interrupt code 9) — a device IRQ via the PLIC.
    SupervisorExternal,
```

Add the decode arm (after `(true, 5) => Cause::SupervisorTimer,`):

```rust
        (true, 9) => Cause::SupervisorExternal,
```

Replace the existing `unknown_interrupt_keeps_interrupt_flag` test (code 9 is no longer unknown) and add a decode test:

```rust
    #[test]
    fn unknown_interrupt_keeps_interrupt_flag() {
        // Interrupt code 1 = supervisor software interrupt; unhandled.
        assert_eq!(
            decode(INTERRUPT_BIT | 1),
            Cause::Unknown { interrupt: true, code: 1 }
        );
    }

    #[test]
    fn decodes_supervisor_external_interrupt() {
        assert_eq!(decode(INTERRUPT_BIT | 9), Cause::SupervisorExternal);
    }
```

- [ ] **Step 2: Run to verify the decode test passes (and nothing else broke)**

Run: `cargo test -p kernel-arch-riscv64 trap::`
Expected: PASS.

- [ ] **Step 3: Add the one-shot log flag and the handler arm**

In `arch/riscv64/src/trap.rs`, add a one-shot flag near `USER_PREEMPTED`:

```rust
/// One-shot: log the first external interrupt that wakes a waiting driver
/// (the smoke test greps for it).
#[cfg(target_arch = "riscv64")]
static EXTERNAL_IRQ_LOGGED: AtomicBool = AtomicBool::new(false);
```

In `trap_handler`, add the arm (after the `SupervisorTimer` arm):

```rust
        Cause::SupervisorExternal => {
            let irq = crate::plic::claim();
            if irq != 0 {
                // Mask the source: the device line stays asserted until the
                // U-mode driver acks it, so leaving it enabled would storm.
                crate::plic::disable(irq);
                if let Some(name) = crate::sched::wake_irq(irq) {
                    if !EXTERNAL_IRQ_LOGGED.swap(true, Ordering::AcqRel) {
                        crate::println!("irq: external IRQ {irq} woke '{name}'");
                    }
                }
                crate::plic::complete(irq);
            }
        }
```

- [ ] **Step 4: Build (host + bare)**

Run: `cargo test -p kernel-arch-riscv64` → expect PASS.
Run: `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf` → expect SUCCESS.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/trap.rs
git commit -m "feat(trap): handle the supervisor external interrupt (claim/mask/wake/complete)"
```

---

## Task 7: the `wait_irq` syscall number

**Files:**
- Modify: `arch/riscv64/src/syscall.rs`

- [ ] **Step 1: Write the failing test**

In `arch/riscv64/src/syscall.rs`, add to `tests`:

```rust
    #[test]
    fn decodes_wait_irq_syscall() {
        assert_eq!(decode_syscall(10), Syscall::WaitIrq);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p kernel-arch-riscv64 syscall::`
Expected: FAIL — `Syscall::WaitIrq` does not exist.

- [ ] **Step 3: Add the variant, decode, and dispatch**

Add to `Syscall` (after `Getrandom`):

```rust
    /// `getrandom(cap)` — draw 32 bytes from the kernel entropy pool.
    Getrandom,
    /// `wait_irq(cap)` — block until the device interrupt named by the cap.
    WaitIrq,
```

Add the decode arm (after `9 => Syscall::Getrandom,`):

```rust
        10 => Syscall::WaitIrq,
```

Add the dispatch arm (after the `Syscall::Getrandom` arm):

```rust
        Syscall::WaitIrq => {
            crate::sched::wait_irq(frame);
            Outcome::Resume
        }
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p kernel-arch-riscv64 syscall::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/syscall.rs
git commit -m "feat(syscall): wait_irq syscall (a7=10) decode + dispatch"
```

---

## Task 8: update the smoke test for interrupt-driven entropy (red)

**Files:**
- Modify: `tools/test-qemu.ps1`

- [ ] **Step 1: Add the external-interrupt assertion**

In `tools/test-qemu.ps1`, add to `$mustMatch` (before the `entropy: pool seeded …` line):

```powershell
    "irq: external IRQ 8 woke 'entropy'",
```

Update the header comment + PASS message to note the entropy component is now interrupt-driven: it blocks on its device's IRQ via the PLIC instead of polling.

- [ ] **Step 2: Run the smoke test — expect RED**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: FAIL — `irq: external IRQ 8 woke 'entropy'` is absent (the component still polls; no interrupt path wired).

- [ ] **Step 3: Commit (red test pinned)**

```bash
git add tools/test-qemu.ps1
git commit -m "test: smoke test asserts the entropy component is interrupt-driven (red)"
```

---

## Task 9: make the entropy component interrupt-driven (green)

**Files:**
- Modify: `arch/riscv64/src/virtio.rs` (the interrupt registers)
- Modify: `kernel/src/main.rs`

- [ ] **Step 1: Add the virtio interrupt registers**

In `arch/riscv64/src/virtio.rs`, add to the register-offset block (after `STATUS`):

```rust
pub const INTERRUPT_STATUS: usize = 0x060;
pub const INTERRUPT_ACK: usize = 0x064;
```

- [ ] **Step 2: Import `csr` and `plic`, add `IRQ_CAP`**

In `kernel/src/main.rs`, extend the arch import:

```rust
    use kernel_arch_riscv64::{cap::Capability, console, csr, dt, entropy, mem, plic, println, sched, task::Message, timer, trap, virtio};
```

After the `ENTROPY_CAP` constant, add:

```rust
    /// The entropy component's cap slot holding its `Interrupt(rng_irq)` cap.
    const IRQ_CAP: usize = 1;
```

- [ ] **Step 3: Add the `wait_irq` syscall wrapper**

In `kernel/src/main.rs`, add after `sys_getrandom`:

```rust
    /// wait_irq syscall (a7 = 10): a0 = cap index of an Interrupt capability.
    /// Blocks until the device interrupt fires; returns a0 = 0, or `usize::MAX`
    /// if the caller lacks the capability.
    ///
    /// # Safety
    /// Always sound; the kernel validates the capability and blocks us.
    #[inline(always)]
    unsafe fn sys_wait_irq(cap: usize) -> usize {
        let ret;
        core::arch::asm!(
            "ecall",
            in("a7") 10usize,
            inout("a0") cap => ret,
            options(nostack),
        );
        ret
    }
```

- [ ] **Step 4: Replace the poll loop with wait_irq + device ack**

In `kernel/src/main.rs`, in `entropy_component`, replace the poll loop:

```rust
                mmio_w(mmio, virtio::QUEUE_NOTIFY, 0);
                loop {
                    dma_fence();
                    if dma_r16(used + 2) == idx {
                        break;
                    }
                }
```

with:

```rust
                mmio_w(mmio, virtio::QUEUE_NOTIFY, 0);
                // Block until the device interrupts (the kernel wakes us),
                // then ack the device to deassert its interrupt line. The
                // device has advanced the used ring by the time we wake.
                sys_wait_irq(IRQ_CAP);
                let status = mmio_r(mmio, virtio::INTERRUPT_STATUS);
                mmio_w(mmio, virtio::INTERRUPT_ACK, status);
```

(`used` is still used in the queue setup above — `QUEUE_DEVICE_LOW/HIGH` — so it stays bound. But the `dma_r16` helper was only used by the deleted poll: delete its definition too.)

Delete the now-unused `dma_r16` helper (near the other DMA helpers):

```rust
    #[inline(always)]
    unsafe fn dma_r16(addr: usize) -> u16 {
        let v;
        core::arch::asm!("lhu {v}, 0({a})", v = out(reg) v, a = in(reg) addr, options(nostack));
        v
    }
```

- [ ] **Step 5: Set up the PLIC and grant the Interrupt cap in `kmain`**

In `kernel/src/main.rs`, in the `if let Some(rng) = rng_base { … }` block, after the existing entropy grants/launch-args (just before the block's closing `}`), add:

```rust
            // Interrupt path: route the RNG's IRQ through the PLIC and grant the
            // component the authority to wait on it (cap slot 1).
            let n = machine.virtio_mmio_count;
            let rng_irq = virtio::irq_for_base(&machine.virtio_mmio[..n], &machine.virtio_mmio_irq[..n], rng)
                .expect("rng has no IRQ in the device tree");
            plic::init(machine.plic_base);
            plic::set_priority(rng_irq, 1);
            // SAFETY: the trap handler and the PLIC are now set up to service it.
            unsafe { csr::sie_enable_external() };
            sched::grant_cap(entropy, IRQ_CAP, Capability::Interrupt(rng_irq));
```

- [ ] **Step 6: Build the kernel**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: SUCCESS.

- [ ] **Step 7: Run the smoke test — expect GREEN**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS`, including `irq: external IRQ 8 woke 'entropy'` and the `entropy: pool …` / `pqc: …` lines (now delivered via the interrupt), with all prior milestones.

Troubleshooting (diagnose, don't weaken the test):
- Hang after the entropy component starts (no `irq:` line, no pool lines) → the interrupt isn't delivered: confirm `csr::sie_enable_external()` is called, `plic::set_priority(rng_irq, 1)` ran, and the PLIC pages are mapped (Task 3). The component blocks forever in `wait_irq` if the IRQ never fires.
- `irq:` line appears but a wrong IRQ number → confirm `irq_for_base` maps `0x10008000` → 8 (Task 1).
- Storm / repeated interrupts → the device isn't being acked: confirm the `INTERRUPT_STATUS`→`INTERRUPT_ACK` write runs after `sys_wait_irq`.

- [ ] **Step 8: Commit**

```bash
git add arch/riscv64/src/virtio.rs kernel/src/main.rs
git commit -m "feat: entropy component blocks on its device IRQ via the PLIC (no more polling)"
```

---

## Task 10: docs — learning note, roadmap, glossary; final verification

**Files:**
- Create: `docs/learning/0020-plic-interrupts.md`
- Modify: `docs/roadmap/roadmap.md`
- Modify: `docs/glossary.md`
- Modify: `docs/learning/README.md`

- [ ] **Step 1: Write a short learning note**

Create `docs/learning/0020-plic-interrupts.md`:

```markdown
# 0020 — The first device interrupt (the PLIC)

**One-line:** the virtio-rng driver now *blocks* for its device's interrupt
instead of spinning a poll loop — the kernel routes the IRQ through the PLIC
and wakes it.

## What changed
- New `arch/riscv64/src/plic.rs`: drive the PLIC (claim/complete, per-context
  enable/threshold) for hart 0's S-mode context. The PLIC is mapped into every
  address space (the interrupt fires while a user task's `satp` is active).
- `Cause::SupervisorExternal` + a handler that claims the IRQ, masks it, wakes
  the bound task, and completes. `sie.SEIE` is enabled at boot.
- New `Capability::Interrupt(irq)` + a `wait_irq(cap)` syscall: the component
  unmasks its IRQ and blocks (`WaitingIrq`); the handler wakes it.
- The entropy component's used-ring poll loop becomes `wait_irq` + a device ack.

## The ideas worth keeping
1. **An interrupt controller is just MMIO: claim, then complete.** The PLIC
   tells you the highest-priority pending IRQ (claim) and you acknowledge it
   (complete); per-context enable bits and a threshold decide what reaches you.
2. **A kernel/userspace interrupt split must mask on deliver, unmask on wait.**
   The virtio line is level-triggered and stays asserted until the *driver*
   (in U-mode) acks the device — asynchronously. If the kernel completed and
   left the source enabled, it would re-fire in a storm. So the handler masks
   the source on claim; the driver acks the device and calls `wait_irq`, which
   unmasks. No interrupt is lost: a masked source stays pending.
3. **A capability gates "wait for my IRQ" too.** Same unforgeable-index check
   as IPC/getrandom — the component owns its device *and* its interrupt.

## Why a spike first
Interrupt setup is unforgiving (wrong PLIC offset/context/IRQ → silence or a
storm). A kernel-side spike verified the PLIC base, the register layout, the
S-mode context (1), and that claiming the virtio IRQ returns 8 — before any of
it was committed.

## Proof
`irq: external IRQ 8 woke 'entropy'`, then the pool is seeded and ML-KEM keyed
as before — the entropy now arrives by interrupt, not by polling.
```

- [ ] **Step 2: Update the roadmap**

In `docs/roadmap/roadmap.md`, find the "(Next candidates: …)" note after the "U-mode getrandom service" block (it lists "an interrupt-driven (PLIC) device path" first). Replace that parenthetical with:

```markdown
### PLIC interrupt path  *(done — 2026-06-22)*

- **Goal:** the first interrupt-driven device — drive the PLIC and let the
  virtio-rng component block on its IRQ instead of polling.
- **You learn:** an interrupt controller (claim/complete, per-context
  enable/threshold), and why a kernel/userspace interrupt split must mask on
  deliver and unmask on wait (the level-triggered line storms otherwise) (see
  [learning note 0020](../learning/0020-plic-interrupts.md)).
- **Done when:** `./tools/test-qemu.ps1` shows the external interrupt wake the
  entropy component (which still seeds the pool / keys ML-KEM). QEMU-only.

(Next candidates: one-shot reply capabilities for deferred/forwarded replies;
interrupt-driven UART input; richer self-healing once a filesystem exists.)
```

- [ ] **Step 3: Add glossary entries**

In `docs/glossary.md`, add after the "Interrupt" entry (only genuinely new terms):

```markdown
- **PLIC (platform-level interrupt controller)** — the RISC-V device that collects external device interrupts and routes the highest-priority pending one to a hart context. The kernel *claims* an interrupt (learns which IRQ) and *completes* it (acknowledges) via PLIC registers.
- **IRQ (interrupt request)** — a numbered interrupt line from a device (here virtio-rng is IRQ 8). The PLIC maps IRQ numbers to priorities and per-context enables.
- **claim / complete** — the PLIC handshake: reading the claim register returns (and latches) the pending IRQ; writing it back signals the handler is done.
```

- [ ] **Step 4: Update the learning-notes index**

In `docs/learning/README.md`, add after the 0019 line:

```markdown
- [0020 — The first device interrupt (the PLIC)](0020-plic-interrupts.md)
```

- [ ] **Step 5: Final verification (run all, paste results)**

Run: `cargo test -p kernel-arch-riscv64` → expect all green.
Run: `cargo build --workspace` → expect success.
Run (PowerShell): `./tools/check-references.ps1` → expect PASS.
Run (PowerShell): `./tools/test-qemu.ps1` → expect `BOOT TEST PASS`.

- [ ] **Step 6: Commit**

```bash
git add docs/learning/0020-plic-interrupts.md docs/roadmap/roadmap.md docs/glossary.md docs/learning/README.md
git commit -m "docs: PLIC interrupt path - learning note, roadmap, glossary"
```

---

## Done-when checklist (maps to spec §1)

- [ ] **An interrupt-driven device** — smoke shows `irq: external IRQ 8 woke 'entropy'` plus the `entropy: pool …` / `pqc: …` lines (data via the interrupt, not polling).
- [ ] **Host tests** — `decode(INTERRUPT_BIT|9)==SupervisorExternal`; PLIC offsets for IRQ 8/ctx 1; `interrupt_irq`; `decode_syscall(10)`; `dt` PLIC base + IRQ 8.
- [ ] `check-references` clean; `cargo build --workspace` green; `BOOT TEST PASS`.
```
