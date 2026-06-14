# Phase 2c — Basic Scheduling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run three in-kernel tasks and switch the CPU between them — cooperatively (`yield_now()`) first, then preemptively (the Phase 2a timer tick), proven live in QEMU.

**Architecture:** Two new modules in the arch crate following the `trap.rs`/`mem` pattern — pure logic (the round-robin run queue, the `Context`/`Task` types) ungated and host-tested, hardware access (the `switch_context` assembly, the static `Scheduler`, the CSR pokes) gated to `target_arch = "riscv64"`. A task is a kernel thread with a static stack and a saved callee-saved `Context`. Voluntary `yield_now()` saves only callee-saved registers via `switch_context`; involuntary preemption reuses the same primitive from inside the timer trap handler, where `__trap_entry` has already saved the full 31-GPR `TrapFrame`. Interrupt state travels correctly across switches because `yield_now` restores the *prior* SIE on resume and new tasks enter through a trampoline that enables interrupts.

**Tech Stack:** Rust `no_std`, RISC-V (riscv64gc), QEMU virt + OpenSBI, PowerShell test scripts.

**Spec:** `docs/superpowers/specs/2026-06-14-phase-2c-scheduling-design.md`

**Conventions reminder:** commits use the house style (`feat(arch):`, `test:`, `docs:`), no Claude co-author line, identity Kathir <kathirpsmy@gmail.com>. Host tests: `cargo test` (whole workspace, runs on the Windows host). Cross-build: `cargo build -p kernel --target riscv64gc-unknown-none-elf`. Integration: `./tools/test-qemu.ps1`.

**Key invariants to preserve:**
- The `Context` field order is a binary contract with the `switch_context` assembly — a host `size_of`/`offset_of` test pins it, exactly as `TrapFrame` is pinned.
- The scheduler critical section (picking the next task + the switch) runs with interrupts disabled so a timer tick can't re-enter the `SingleHartCell` mid-switch.
- A task always runs with interrupts enabled: new tasks via the trampoline, resumed tasks via `yield_now` restoring the SIE it saw on entry.

---

### Task 1: Failing QEMU smoke test (test-first)

**Files:**
- Modify: `tools/test-qemu.ps1`

- [ ] **Step 1: Update the header comment**

In `tools/test-qemu.ps1`, replace the header comment (lines 1–6) with:

```powershell
# Boot smoke test: cross-builds the kernel, boots it headless under QEMU
# (riscv64 virt + OpenSBI), captures the serial console, and asserts the
# Phase 2a milestones (greeting, survived breakpoint, >= 2 timer ticks),
# the Phase 2b milestones (Sv39 paging on, W^X blocking a rodata write, a
# frame alloc/free round-trip), and the Phase 2c milestones (three tasks
# round-robin cooperatively, then a non-yielding task is preempted).
# Usage: ./tools/test-qemu.ps1     (exit code 0 = pass, 1 = fail)
```

- [ ] **Step 2: Add the Phase 2c patterns**

Replace the `$mustMatch` array with (the `(?s)` ordering regex proves a
real round-robin rotation — the three tasks' first and second steps
appear in order A,B,C,A,B,C, which a single thread could not produce;
`preempted the hog` proves a task ran *after* the hog stopped yielding,
which only a timer preemption can cause):

```powershell
$mustMatch = @(
    "hello world",
    "trap: breakpoint",
    "survived breakpoint",
    "paging: sv39 on",
    "wx: rodata write blocked",
    "frames: alloc/free ok",
    "(?s)sched: A step 0.*sched: B step 0.*sched: C step 0.*sched: A step 1.*sched: B step 1.*sched: C step 1",
    "preempted the hog",
    "tick: 2(?!\d)"
)
```

- [ ] **Step 3: Update the PASS message**

Replace the PASS line:

```powershell
    Write-Host "BOOT TEST PASS: 2a + 2b milestones plus cooperative round-robin and preemption all observed." -ForegroundColor Green
```

- [ ] **Step 4: Run the test to verify it fails on exactly the new patterns**

Run: `./tools/test-qemu.ps1`
Expected: `BOOT TEST FAIL: missing within 30s:` listing the two new
patterns (the `(?s)...` rotation regex and `preempted the hog`). All
seven 2a/2b patterns still observed, proving the current kernel still
boots.

- [ ] **Step 5: Commit**

```powershell
git add tools/test-qemu.ps1
git commit -m "test: extend QEMU smoke test for Phase 2c scheduling milestones (failing)"
```

---

### Task 2: `sstatus` disable accessor (returns prior state)

**Files:**
- Modify: `arch/riscv64/src/csr.rs`

- [ ] **Step 1: Add `sstatus_disable_interrupts`**

Append to `arch/riscv64/src/csr.rs`:

```rust
/// Disable supervisor interrupts (`sstatus.SIE`, bit 1), returning
/// whether they were previously enabled. The scheduler uses the return
/// value to restore the caller's prior interrupt state after a context
/// switch, so a task resumed from inside a trap handler does not
/// accidentally run with interrupts unmasked mid-trap-return.
///
/// `csrrci` atomically reads the old `sstatus` and clears the bit.
///
/// # Safety
/// Disabling interrupts is always memory-safe, but the caller owns the
/// obligation to re-enable them (or restore the returned state) so the
/// hart does not stay deaf to the timer forever.
#[inline]
pub unsafe fn sstatus_disable_interrupts() -> bool {
    let prev: usize;
    unsafe {
        asm!("csrrci {}, sstatus, 0x2", out(reg) prev, options(nostack, nomem));
    }
    prev & 0x2 != 0
}
```

- [ ] **Step 2: Cross-build**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: success (the function is unused until Task 5; an unused-warning
is acceptable for this commit).

- [ ] **Step 3: Commit**

```powershell
git add arch/riscv64/src/csr.rs
git commit -m "feat(arch): sstatus disable accessor returning prior interrupt state"
```

---

### Task 3: Task and context types — pure (TDD)

**Files:**
- Create: `arch/riscv64/src/task.rs`
- Modify: `arch/riscv64/src/lib.rs`

- [ ] **Step 1: Declare the module**

In `arch/riscv64/src/lib.rs`, after the `pub mod mem;` block, add:

```rust
/// Tasks and their saved register context. Pure types (host-testable);
/// the context-switch assembly and the scheduler statics live in `sched`.
pub mod task;
```

- [ ] **Step 2: Write the failing test**

Create `arch/riscv64/src/task.rs`:

```rust
//! Tasks (in-kernel threads) and the register context a switch saves.
//!
//! A `Context` is the *callee-saved* register set — `ra`, `sp`, and
//! `s0..s11`. That is all a voluntary switch must preserve: the RISC-V
//! C calling convention says the caller already saved anything else it
//! cares about across a call, and `switch_context` (in `sched`) is
//! reached by a call. Involuntary preemption is different — it saves the
//! full 31-GPR `TrapFrame` in the trap entry assembly — but the *switch*
//! itself still only juggles this callee-saved block.
//!
//! Pure here (host-testable). The field order is a binary contract with
//! the `switch_context` assembly; the test at the bottom pins it.

/// The saved callee-saved registers of a parked task.
///
/// Field order and size are the contract with `sched`'s `switch_context`
/// assembly: `ra` at offset 0, `sp` at 8, then `s0..s11` at 16..104.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Context {
    /// Return address — where `switch_context`'s `ret` resumes the task.
    pub ra: usize,
    /// The task's stack pointer.
    pub sp: usize,
    /// Callee-saved `s0..s11` (x8, x9, x18..x27).
    pub s: [usize; 12],
}

impl Context {
    /// An all-zero context. `spawn` (in `sched`) fills `ra`/`sp`/`s[0]`
    /// to forge the first run of a never-run task.
    pub const fn zeroed() -> Self {
        Self { ra: 0, sp: 0, s: [0; 12] }
    }
}

/// A task's scheduling state. Only the two states Phase 2c needs:
/// blocking/sleeping and exit arrive with the concepts that need them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    /// Runnable, not currently on the CPU.
    Ready,
    /// Currently on the CPU (exactly one task at a time, single hart).
    Running,
}

/// An in-kernel task: its parked context, its state, the top of its
/// static stack (kept for diagnostics), and a name for the demo output.
#[derive(Debug)]
pub struct Task {
    pub context: Context,
    pub state: TaskState,
    /// Top of the task's static stack (informational; `context.sp` is
    /// what actually drives execution).
    pub stack_top: usize,
    pub name: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_layout_matches_switch_asm() {
        // If any of these change, sched's switch_context assembly must
        // change with them (and vice versa).
        assert_eq!(core::mem::size_of::<Context>(), 112);
        assert_eq!(core::mem::offset_of!(Context, ra), 0);
        assert_eq!(core::mem::offset_of!(Context, sp), 8);
        assert_eq!(core::mem::offset_of!(Context, s), 16);
    }

    #[test]
    fn zeroed_context_is_all_zero() {
        let c = Context::zeroed();
        assert_eq!(c.ra, 0);
        assert_eq!(c.sp, 0);
        assert_eq!(c.s, [0; 12]);
    }
}
```

- [ ] **Step 3: Run the tests to verify they pass**

These tests assert properties of the types just written, so they pass
immediately (the "failing" state for pure type definitions is a compile
error if the types are wrong).

Run: `cargo test -p kernel-arch-riscv64 task`
Expected: PASS (`context_layout_matches_switch_asm`, `zeroed_context_is_all_zero`).

- [ ] **Step 4: Cross-build**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: success (unused-code warnings acceptable until Task 5/7 wire it).

- [ ] **Step 5: Commit**

```powershell
git add arch/riscv64/src/lib.rs arch/riscv64/src/task.rs
git commit -m "feat(arch): Task and callee-saved Context types with layout test"
```

---

### Task 4: Round-robin run queue — pure (TDD)

**Files:**
- Create: `arch/riscv64/src/sched.rs`
- Modify: `arch/riscv64/src/lib.rs`

- [ ] **Step 1: Declare the module**

In `arch/riscv64/src/lib.rs`, after the `pub mod task;` block, add:

```rust
/// Scheduling: the round-robin run queue (pure, host-testable) plus the
/// gated context-switch assembly and the static scheduler instance.
pub mod sched;
```

- [ ] **Step 2: Write the failing tests**

Create `arch/riscv64/src/sched.rs` with the pure core and tests; the
`pick_next` body is `todo!()` for now:

```rust
//! Scheduling: a fixed-size round-robin run queue plus the context
//! switch that makes it real.
//!
//! Pure here: the `Scheduler` struct and `pick_next` (host-tested). The
//! gated section below adds the static `SCHED` instance, the
//! `switch_context` assembly, `spawn`, `yield_now`, `enter`, and
//! `preempt` — everything that touches CSRs, assembly, or the live
//! console.

use crate::task::{Context, Task, TaskState};

/// Maximum concurrent tasks: the three demo tasks plus one slot of
/// headroom. The bootstrap (`kmain`) context is NOT a slot — it is a
/// throwaway `Context` used only for the first switch (see `enter`), so
/// the rotation is purely among the spawned tasks.
pub const MAX_TASKS: usize = 4;

/// The run queue: a fixed array of optional tasks and the index of the
/// one currently running.
pub struct Scheduler {
    tasks: [Option<Task>; MAX_TASKS],
    current: usize,
}

impl Scheduler {
    /// An empty scheduler; `spawn` fills slots and `enter` starts it.
    pub const fn new() -> Self {
        Self { tasks: [None, None, None, None], current: 0 }
    }

    /// Index of the next task to run after `current`, round-robin: scan
    /// forward (wrapping) for the next `Ready` slot, skipping empty
    /// slots and the current task. Returns `current` if nobody else is
    /// ready — the caller then keeps running.
    pub fn pick_next(&self) -> usize {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(name: &'static str, state: TaskState) -> Task {
        Task { context: Context::zeroed(), state, stack_top: 0, name }
    }

    /// Three ready tasks in slots 0..3, slot 3 empty; `current` running.
    fn three_tasks(current: usize) -> Scheduler {
        let mut s = Scheduler::new();
        s.tasks[0] = Some(task("A", TaskState::Ready));
        s.tasks[1] = Some(task("B", TaskState::Ready));
        s.tasks[2] = Some(task("C", TaskState::Ready));
        s.current = current;
        s.tasks[current].as_mut().unwrap().state = TaskState::Running;
        s
    }

    #[test]
    fn rotates_to_the_next_ready_task() {
        let s = three_tasks(0);
        assert_eq!(s.pick_next(), 1);
    }

    #[test]
    fn wraps_around_and_skips_empty_slots() {
        // current = 2; slot 3 is empty, so the next ready is slot 0.
        let s = three_tasks(2);
        assert_eq!(s.pick_next(), 0);
    }

    #[test]
    fn single_task_returns_itself() {
        let mut s = Scheduler::new();
        s.tasks[0] = Some(task("solo", TaskState::Running));
        s.current = 0;
        assert_eq!(s.pick_next(), 0);
    }

    #[test]
    fn no_other_ready_task_returns_current() {
        // All three present but the other two are Running (artificial) —
        // no Ready peer, so pick_next keeps the current task.
        let mut s = three_tasks(0);
        s.tasks[1].as_mut().unwrap().state = TaskState::Running;
        s.tasks[2].as_mut().unwrap().state = TaskState::Running;
        assert_eq!(s.pick_next(), 0);
    }

    #[test]
    fn full_rotation_visits_every_task_once() {
        let mut s = three_tasks(0);
        let mut order = alloc_order(&mut s);
        order.sort_unstable();
        assert_eq!(order, [0, 1, 2]);
    }

    // Simulate three cooperative yields and record who runs.
    fn alloc_order(s: &mut Scheduler) -> [usize; 3] {
        let mut seen = [0usize; 3];
        for slot in seen.iter_mut() {
            let next = s.pick_next();
            s.tasks[s.current].as_mut().unwrap().state = TaskState::Ready;
            s.tasks[next].as_mut().unwrap().state = TaskState::Running;
            s.current = next;
            *slot = next;
        }
        seen
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p kernel-arch-riscv64 sched`
Expected: FAIL — every test hits `todo!()` in `pick_next`.

- [ ] **Step 4: Implement `pick_next`**

Replace the `todo!()` body:

```rust
    pub fn pick_next(&self) -> usize {
        for offset in 1..=MAX_TASKS {
            let i = (self.current + offset) % MAX_TASKS;
            if let Some(t) = &self.tasks[i] {
                if t.state == TaskState::Ready {
                    return i;
                }
            }
        }
        self.current
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p kernel-arch-riscv64 sched`
Expected: PASS (all five `pick_next` tests).

- [ ] **Step 6: Commit**

```powershell
git add arch/riscv64/src/lib.rs arch/riscv64/src/sched.rs
git commit -m "feat(arch): round-robin run queue pure core with host tests"
```

---

### Task 5: Context switch, trampoline, and the scheduler API (gated)

**Files:**
- Modify: `arch/riscv64/src/sched.rs`

This task is the heart of the phase. Everything here is gated to
`target_arch = "riscv64"`, so it compiles into the kernel binary but is
invisible to host `cargo test`. Verification is the cross-build now and
the live smoke test in Task 7.

- [ ] **Step 1: Add the context-switch and trampoline assembly**

Append to `arch/riscv64/src/sched.rs`:

```rust
// The actual switch. Saves the callee-saved set of the running task into
// `*old` (a0), loads it from `*new` (a1), and `ret`s — which resumes
// wherever `new.ra` points. Offsets match `task::Context`:
// ra@0, sp@8, s0..s11 @ 16..104. Caller-saved registers are deliberately
// NOT saved: the Rust calling convention already treats them as clobbered
// across this call.
#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(
    r#"
    .section .text
    .global switch_context
switch_context:
    sd ra,  0(a0)
    sd sp,  8(a0)
    sd s0,  16(a0)
    sd s1,  24(a0)
    sd s2,  32(a0)
    sd s3,  40(a0)
    sd s4,  48(a0)
    sd s5,  56(a0)
    sd s6,  64(a0)
    sd s7,  72(a0)
    sd s8,  80(a0)
    sd s9,  88(a0)
    sd s10, 96(a0)
    sd s11, 104(a0)
    ld ra,  0(a1)
    ld sp,  8(a1)
    ld s0,  16(a1)
    ld s1,  24(a1)
    ld s2,  32(a1)
    ld s3,  40(a1)
    ld s4,  48(a1)
    ld s5,  56(a1)
    ld s6,  64(a1)
    ld s7,  72(a1)
    ld s8,  80(a1)
    ld s9,  88(a1)
    ld s10, 96(a1)
    ld s11, 104(a1)
    ret

    # First entry of a never-run task. switch_context loaded sp = stack
    # top, s0 = the task's entry function, ra = here. We enable interrupts
    # (so this task is preemptible — a freshly entered task has no prior
    # yield_now to restore SIE for it) and jump into the entry. If the
    # entry ever returns (it is typed `-> !`, so it should not), fall
    # through to a loud panic.
    .global task_trampoline
task_trampoline:
    csrsi sstatus, 0x2
    jalr s0
    call task_has_returned
"#
);

#[cfg(target_arch = "riscv64")]
extern "C" {
    fn switch_context(old: *mut Context, new: *const Context);
    fn task_trampoline();
}

/// Reached only if a task's entry function returns — a Phase 2c bug
/// (entries must loop forever; task exit is deferred).
#[cfg(target_arch = "riscv64")]
#[no_mangle]
extern "C" fn task_has_returned() -> ! {
    panic!("task entry returned (task exit is not implemented in Phase 2c)");
}
```

- [ ] **Step 2: Add the static scheduler and `spawn`**

Append to `arch/riscv64/src/sched.rs`:

```rust
/// The kernel's one scheduler. Lives in a `SingleHartCell` (the same
/// re-entry-guarded primitive `mem` introduced): one hart, and the
/// guard fires if a future change lets the scheduler re-enter itself.
#[cfg(target_arch = "riscv64")]
static SCHED: crate::mem::SingleHartCell<Scheduler> =
    crate::mem::SingleHartCell::new(Scheduler::new());

/// Register a task: forge a `Context` whose first switch lands in
/// `task_trampoline` with `sp` = top of `stack` and `s0` = `entry`.
/// Panics if there is no free slot — a static configuration bug.
///
/// `stack_top` is the (exclusive) top address of a static stack array;
/// it is rounded down to a 16-byte boundary per the RISC-V ABI.
#[cfg(target_arch = "riscv64")]
pub fn spawn(name: &'static str, entry: extern "C" fn() -> !, stack_top: usize) {
    SCHED.with(|s| {
        let slot = s
            .tasks
            .iter()
            .position(Option::is_none)
            .expect("scheduler full");
        let mut context = Context::zeroed();
        context.ra = task_trampoline as *const () as usize;
        context.sp = stack_top & !0xF;
        context.s[0] = entry as usize;
        s.tasks[slot] = Some(Task {
            context,
            state: TaskState::Ready,
            stack_top,
            name,
        });
    });
}
```

- [ ] **Step 3: Add `yield_now`, `enter`, and `preempt`**

Append to `arch/riscv64/src/sched.rs`:

```rust
/// Voluntarily give up the CPU to the next ready task. Returns when this
/// task is scheduled again. A no-op if nobody else is ready.
///
/// The pick-and-switch runs with interrupts disabled so a timer tick
/// cannot re-enter `SCHED` mid-switch. On resume we restore the SIE the
/// caller had on entry: a task that yielded with interrupts on resumes
/// with them on, while a task preempted from inside the trap handler
/// (SIE already off) resumes with them off and lets the trap return
/// re-enable them atomically via `sret`.
#[cfg(target_arch = "riscv64")]
pub fn yield_now() {
    // SAFETY: disabling interrupts is always safe; we restore the prior
    // state below (or, for a switch, on resume).
    let had_interrupts = unsafe { crate::csr::sstatus_disable_interrupts() };

    let switch = SCHED.with(|s| {
        let current = s.current;
        let next = s.pick_next();
        if next == current {
            return None;
        }
        s.tasks[current].as_mut().unwrap().state = TaskState::Ready;
        s.tasks[next].as_mut().unwrap().state = TaskState::Running;
        s.current = next;
        let old = core::ptr::addr_of_mut!(s.tasks[current].as_mut().unwrap().context);
        let new = core::ptr::addr_of!(s.tasks[next].as_ref().unwrap().context);
        Some((old, new))
    });

    if let Some((old, new)) = switch {
        // SAFETY: both pointers address `Context`s inside the 'static
        // SCHED, valid for the whole program. Single hart + the disabled
        // interrupts above mean no other code aliases them while the
        // assembly runs. Execution resumes here when this task is picked
        // again.
        unsafe { switch_context(old, new) };
    }

    if had_interrupts {
        // SAFETY: re-enabling is the inverse of the disable above; a trap
        // handler is installed and the timer is armed by the time any
        // task runs.
        unsafe { crate::csr::sstatus_enable_interrupts() };
    }
}

/// Start scheduling: switch into the first spawned task. Never returns —
/// the bootstrap (`kmain`) context is abandoned, which is correct because
/// a kernel never returns from `kmain`. The first switch needs an `old`
/// `Context` to save into; we use a throwaway on this stack that is never
/// restored (xv6's scheduler-context trick in miniature).
#[cfg(target_arch = "riscv64")]
pub fn enter() -> ! {
    // SAFETY: returned state ignored — `enter` never returns, and the
    // first task re-enables interrupts in `task_trampoline`.
    let _ = unsafe { crate::csr::sstatus_disable_interrupts() };

    let first = SCHED.with(|s| {
        s.current = 0;
        let t = s.tasks[0].as_mut().expect("enter() with no spawned task");
        t.state = TaskState::Running;
        core::ptr::addr_of!(t.context)
    });

    let mut throwaway = Context::zeroed();
    // SAFETY: `first` addresses a Context in the 'static SCHED; the
    // throwaway is a valid (never-restored) save target on this stack.
    unsafe { switch_context(core::ptr::addr_of_mut!(throwaway), first) };
    unreachable!("enter() never returns to the bootstrap context");
}

/// Preemption entry, called from the timer trap handler. The interrupted
/// task's full 31-GPR state is already saved in its `TrapFrame` on its
/// stack; this just parks the handler's continuation and runs the next
/// task via the same primitive as a voluntary yield.
#[cfg(target_arch = "riscv64")]
pub fn preempt() {
    yield_now();
}
```

- [ ] **Step 4: Cross-build**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: success. Unused-code warnings on `spawn`/`enter`/`preempt` are
acceptable — Task 6 wires `preempt` and Task 7 wires the rest.

- [ ] **Step 5: Run host tests (the pure core must still pass)**

Run: `cargo test -p kernel-arch-riscv64`
Expected: PASS (the gated code is invisible to the host; the `task` and
`sched::pick_next` tests still pass).

- [ ] **Step 6: Commit**

```powershell
git add arch/riscv64/src/sched.rs
git commit -m "feat(arch): context switch, task trampoline, and scheduler API"
```

---

### Task 6: Drive preemption from the timer tick

**Files:**
- Modify: `arch/riscv64/src/trap.rs`

- [ ] **Step 1: Call the scheduler from the timer arm**

In `arch/riscv64/src/trap.rs`, in `trap_handler`, replace the
`Cause::SupervisorTimer` arm:

```rust
        Cause::SupervisorTimer => {
            crate::timer::on_tick();
            // Tick-policy hook: preempt the running task. The full
            // register set is already saved in this TrapFrame; preempt()
            // parks the handler's continuation and runs the next task.
            crate::sched::preempt();
        }
```

- [ ] **Step 2: Cross-build and run host tests**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf` — expected: success.
Run: `cargo test -p kernel-arch-riscv64` — expected: PASS.

- [ ] **Step 3: Commit**

```powershell
git add arch/riscv64/src/trap.rs
git commit -m "feat(arch): preempt the running task on each timer tick"
```

---

### Task 7: Wire kmain — spawn tasks, run the demo, smoke test goes green

**Files:**
- Modify: `kernel/src/main.rs`

- [ ] **Step 1: Update imports and the module header**

In `kernel/src/main.rs`, update the `use` line inside `mod bare` to add
`sched`:

```rust
    use kernel::GREETING;
    use kernel_arch_riscv64::{mem, println, sched, timer, trap};
    use kernel_common::PROJECT_NAME;
```

Also update the greeting's phase label and the module doc header where it
names a phase — change the kmain greeting line from `Phase 2b` to
`Phase 2c`:

```rust
        println!("{GREETING} from {PROJECT_NAME} - Phase 2c (hart {hartid})");
```

- [ ] **Step 2: Update `kmain` to start scheduling**

Replace the body of `kmain` from the `timer::start();` line onward (the
breakpoint, `mem::init`, `wx_probe`, and `frame_roundtrip` calls stay
exactly as they are):

```rust
        wx_probe();
        frame_roundtrip();

        // Phase 2c: spawn three tasks and hand the CPU to the scheduler.
        // Interrupts are enabled here (timer::start) BEFORE entering, so
        // the cooperative round-robin runs in the sub-millisecond window
        // before the first tick (~1 s away); preemption then takes over.
        // addr_of! takes each static stack's address without forming a
        // reference (no unsafe needed, no static_mut_refs lint); the top
        // of the array is this task's initial stack pointer.
        sched::spawn("A", task_a, core::ptr::addr_of!(STACK_A) as usize + TASK_STACK);
        sched::spawn("B", task_b, core::ptr::addr_of!(STACK_B) as usize + TASK_STACK);
        sched::spawn("C", task_c, core::ptr::addr_of!(STACK_C) as usize + TASK_STACK);
        timer::start();
        println!("(scheduler starting; heartbeat ~1/s; exit QEMU with Ctrl-A then X)");
        sched::enter()
```

Note: `kmain` now ends in `sched::enter()` (which is `-> !`) instead of
`park()`. `park()` stays in the file — it is still used by the panic
handler.

- [ ] **Step 3: Add the task stacks, the hog flag, and the task bodies**

Inside `mod bare`, after the `frame_roundtrip` function, add:

```rust
    use core::sync::atomic::{AtomicBool, Ordering};

    /// Per-task kernel stack size. 16 KiB is ample for these print loops;
    /// per-task guard pages are deferred (see the Phase 2c spec §3.5).
    const TASK_STACK: usize = 16 * 1024;

    static mut STACK_A: [u8; TASK_STACK] = [0; TASK_STACK];
    static mut STACK_B: [u8; TASK_STACK] = [0; TASK_STACK];
    static mut STACK_C: [u8; TASK_STACK] = [0; TASK_STACK];

    /// Set by task C when it stops yielding. A and B only ever observe it
    /// as `true` if a timer preemption schedules them while C hogs the
    /// CPU — which is exactly the preemption proof.
    static HOGGING: AtomicBool = AtomicBool::new(false);

    extern "C" fn task_a() -> ! {
        worker("A")
    }

    extern "C" fn task_b() -> ! {
        worker("B")
    }

    /// The cooperative citizens: two visible steps yielding between each
    /// (proving round-robin), then spin yielding. When C starts hogging,
    /// the next time preemption lets this task run it prints the proof
    /// line once, then goes quiet.
    fn worker(name: &str) -> ! {
        for n in 0..2 {
            println!("sched: {name} step {n}");
            sched::yield_now();
        }
        loop {
            if HOGGING.load(Ordering::Acquire) {
                println!("sched: {name} preempted the hog");
                loop {
                    sched::yield_now();
                }
            }
            sched::yield_now();
        }
    }

    /// The hog: two cooperative steps, then a tight loop that NEVER
    /// yields. Without preemption the kernel would be stuck here forever;
    /// the timer tick is what lets A and B run again.
    extern "C" fn task_c() -> ! {
        for n in 0..2 {
            println!("sched: C step {n}");
            sched::yield_now();
        }
        println!("sched: C hogging (no yield)");
        HOGGING.store(true, Ordering::Release);
        loop {
            core::hint::spin_loop();
        }
    }
```

- [ ] **Step 4: Run the full smoke test**

Run: `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS: 2a + 2b milestones plus cooperative round-robin and preemption all observed.` — exit code 0.

If it fails, the script prints the serial log. Debug with the
systematic-debugging skill before editing — likely culprits and checks:
- Rotation regex missing → confirm the cooperative steps print in
  A,B,C,A,B,C order (a switch bug would scramble or freeze them).
- `preempted the hog` missing → confirm `timer::start()` runs before
  `enter()` and the `SupervisorTimer` arm calls `sched::preempt()`; a
  hang here means the tick is not preempting the tight loop.
- A QEMU hang with no 2c output → suspect `switch_context` offsets or the
  forged `Context`; re-check against `task::Context`'s layout test.

- [ ] **Step 5: Run the host tests**

Run: `cargo test`
Expected: PASS (whole workspace).

- [ ] **Step 6: Commit**

```powershell
git add kernel/src/main.rs
git commit -m "feat: Phase 2c live - three tasks, cooperative round-robin, timer preemption"
```

---

### Task 8: Documentation

**Files:**
- Create: `docs/learning/0005-scheduling-and-context-switching.md`
- Modify: `docs/learning/README.md`
- Modify: `docs/glossary.md`
- Modify: `docs/roadmap/roadmap.md`

- [ ] **Step 1: Write the learning note**

Create `docs/learning/0005-scheduling-and-context-switching.md`. The
draft below is real content to adapt — extend any section with what
actually surprised you during implementation:

```markdown
# 0005 — Scheduling and context switching (Phase 2c)

How the kernel learned to run more than one thing: parking a task's
registers, handing the CPU to another, and letting the timer take the CPU
back from a task that won't give it up.

## A task is just a saved set of registers

A "task" sounds heavy, but in a kernel it is mostly a stack plus a place
to store registers when it is not running. Our `Context` is only the
*callee-saved* set — `ra`, `sp`, and `s0..s11`. That is a deliberate
contrast with the trap frame, which saves all 31 GPRs.

## Why two switch paths save different amounts

The insight of the phase: it depends on whether the switch is *voluntary*.
`yield_now()` is reached by a normal function call, and the RISC-V calling
convention says the caller already preserved any caller-saved register it
still needs. So the switch only has to save the callee-saved set — 14
registers. Preemption is involuntary: a timer interrupt freezes the task
mid-instruction, having volunteered nothing, so the *full* 31-GPR state
must be saved — and it already is, by the trap entry assembly from Phase
2a. So preemption reuses the very same `switch_context`; the trap frame
does the heavy lifting underneath it.

## The first run of a task that has never run

`switch_context` restores registers and `ret`s — but a brand-new task has
no saved registers to restore. We forge a `Context`: `ra` points at a
small trampoline, `sp` at the top of the task's stack, and `s0` holds the
real entry function. The first switch "returns" into the trampoline, which
jumps to the entry. There is nothing to restore the first time — we
manufacture a plausible past.

## Interrupts: the subtle part

The bug that does not announce itself: if you enable interrupts at the
wrong moment, a timer tick lands in the middle of a context switch and
re-enters the scheduler. Two rules keep it correct. First, the
pick-the-next-task-and-switch critical section runs with interrupts
disabled. Second — less obvious — `yield_now` restores the interrupt state
the caller *had on entry*, rather than blindly enabling. A task that
yielded with interrupts on resumes with them on; a task preempted from
inside the trap handler (interrupts already masked) resumes with them
masked and lets `sret` re-enable them atomically. New tasks are the
exception that proves the rule: they never went through a `yield_now`, so
the trampoline enables interrupts explicitly.

## Proving preemption, not just claiming it

Cooperative scheduling is easy to fake-pass: three tasks taking turns
could just be three function calls. So the boot proves the real thing.
Task C runs a tight loop that never yields. Without preemption the kernel
would freeze on it. The timer tick descheduled it, and tasks A and B —
which only run again if something *takes* the CPU from C — print a line
that can only appear under preemption. If that line shows up, preemption
works; the smoke test greps for exactly it.

## Honest caveat carried forward

Only the boot stack has a guard page; the three task stacks do not, so a
task that overflowed its 16 KiB stack would corrupt memory silently. The
demo tasks are nowhere near that, and the real fix (per-task guard pages
and a dedicated trap stack) belongs with the Phase 3 work that introduces
user space and privilege transitions.
```

- [ ] **Step 2: Index the note**

In `docs/learning/README.md`, append to the Notes list:

```markdown
- [0005 — Scheduling and context switching (Phase 2c)](0005-scheduling-and-context-switching.md)
```

- [ ] **Step 3: Add glossary entries**

Append to `docs/glossary.md` (keeping the one-bullet-per-term format):

```markdown
- **Task / kernel thread** — an independently schedulable unit of execution inside the kernel: a stack plus a saved register context. Phase 2c runs three of them.
- **Context switch** — saving the registers of the running task and restoring another's, so the CPU resumes a different task as if it had never stopped.
- **Context (callee-saved frame)** — the minimal register set a *voluntary* switch must preserve: `ra`, `sp`, and `s0..s11`. Smaller than a trap frame because the calling convention already saved the rest.
- **Run queue** — the kernel's list of runnable tasks and the bookkeeping that picks which runs next; ours is a fixed array scanned round-robin.
- **Round-robin** — the simplest fair scheduling policy: give each ready task a turn in rotation, then start over.
- **Cooperative scheduling** — tasks keep the CPU until they voluntarily `yield`. Simple, but one selfish task can starve the rest.
- **Preemptive scheduling** — the kernel forcibly takes the CPU back (here, on a timer tick), so no task can monopolize it.
- **Yield** — a task voluntarily giving up the CPU to let another run; in this kernel, `yield_now()`.
- **Time slice / tick-policy hook** — the decision made on each timer tick about whether to preempt the running task; the point where the heartbeat becomes a scheduler clock.
- **Trampoline** — a tiny stub a new task first lands in: it sets up the task's running conditions (here, enabling interrupts) before jumping to the real entry function.
```

- [ ] **Step 4: Mark 2c done in the roadmap**

In `docs/roadmap/roadmap.md`, change the 2c heading:

```markdown
### Phase 2c — Basic scheduling  *(done — 2026-06-14)*
```

And update its "Done when" line to reflect the proof:

```markdown
- **Done when:** `./tools/test-qemu.ps1` observes three tasks round-robin cooperatively, then a non-yielding task preempted by the timer — all in one boot, alongside the 2a/2b milestones.
```

- [ ] **Step 5: Verify references and commit**

Run: `./tools/check-references.ps1` — expected: `Reference check OK`.
Run: `cargo test` — expected: PASS.

```powershell
git add docs/learning docs/glossary.md docs/roadmap/roadmap.md
git commit -m "docs: scheduling learning note, glossary terms, roadmap 2c done"
```

---

### Task 9: Final verification

- [ ] **Step 1: Full host test suite**

Run: `cargo test`
Expected: PASS, zero failures.

- [ ] **Step 2: Cross-build clean**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: success, no warnings about unused scheduler items (everything
is wired by now).

- [ ] **Step 3: Smoke test**

Run: `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS` — all nine patterns in one boot.

- [ ] **Step 4: Reference check**

Run: `./tools/check-references.ps1`
Expected: `Reference check OK`.

No commit — this task only verifies. If anything fails, fix it with the
systematic-debugging skill before declaring the phase done.
```
