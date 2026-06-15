# Phase 3b-i: U-mode tasks in the run queue — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fold U-mode tasks into the 2c scheduler so kernel and user tasks share one round-robin run queue, with per-task privilege carried by the saved trapframe.

**Architecture:** A U-mode task becomes an ordinary scheduler slot. `spawn_user` forges a first-run `Context` that `switch_context` "returns" into via `user_trampoline` (3a's `enter_user_asm`, renamed), which `sret`s to U-mode. After first entry, every task resumes through the unchanged `switch_context` → trap-return path, which restores `sepc`/`sstatus` (privilege) from the trapframe. Termination becomes a scheduler op (`exit_current`) instead of 3a's return-to-`kmain`. A new `yield` syscall lets U-mode tasks cooperate; a one-shot kernel detector proves a U-mode task gets preempted.

**Tech Stack:** Rust `no_std`, RISC-V (riscv64gc), QEMU virt + OpenSBI, PowerShell smoke test. Host unit tests via `cargo test -p kernel-arch-riscv64`.

**Spec:** `docs/superpowers/specs/2026-06-14-phase-3b-i-user-scheduling-design.md`

**Reference — final demo behavior (Task 6 builds this):** Run queue = `idle` (kernel, cooperative) + four U-mode tasks `ping`, `pong` (print + `yield` ×2 then `exit(0)`), `hog` (2 cooperative rounds, then spin forever, never yields/exits), `bad` (loads a kernel address → killed). Spawn order makes `ping` run first; `bad` dies in the first cycle (containment, scheduler survives); `ping`/`pong` finish and exit cleanly; `hog` then holds the CPU so the first timer tick (~1 s) preempts a **U-mode** task, firing the detector line. `idle` keeps the system alive afterward.

---

## Task 1: `TaskState::Exited` + pure user-context forge

**Files:**
- Modify: `arch/riscv64/src/task.rs`
- Test: `arch/riscv64/src/task.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `arch/riscv64/src/task.rs`:

```rust
    #[test]
    fn forge_user_context_sets_launch_fields() {
        // tramp/entry/user_sp/kstack are arbitrary addresses for the test;
        // 16-alignment is applied to the two stack pointers.
        let c = forge_user_context(0xAAAA, 0xBBBB, 0x1_0008, 0x2_0008, 0xCAFE);
        assert_eq!(c.ra, 0xAAAA, "ra = trampoline");
        assert_eq!(c.sp, 0x2_0000, "sp = kstack_top, 16-aligned");
        assert_eq!(c.s[0], 0xBBBB, "s0 = user entry (-> sepc)");
        assert_eq!(c.s[1], 0x1_0000, "s1 = user sp, 16-aligned");
        assert_eq!(c.s[2], 0xCAFE, "s2 = sstatus");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p kernel-arch-riscv64 forge_user_context`
Expected: FAIL — `cannot find function forge_user_context`.

- [ ] **Step 3: Add the `Exited` state and the forge function**

In `arch/riscv64/src/task.rs`, extend `TaskState` (add the variant; update the doc comment):

```rust
/// A task's scheduling state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    /// Runnable, not currently on the CPU.
    Ready,
    /// Currently on the CPU (exactly one task at a time, single hart).
    Running,
    /// Terminated (clean `exit` or a fatal U-mode fault). Never scheduled
    /// again; the slot is skipped by `pick_next`. Stacks are not reclaimed
    /// (reaping is deferred).
    Exited,
}
```

Then add, just below `user_sstatus`:

```rust
/// Forge the first-run [`Context`] of a U-mode task. The first
/// `switch_context` into this context "returns" into `tramp`
/// (`sched::user_trampoline`), which reads `s0`/`s1`/`s2` and `sret`s to
/// U-mode. Both stack pointers are rounded down to a 16-byte boundary
/// (RISC-V ABI). Pure (host-tested); `sched::spawn_user` supplies `tramp`.
pub fn forge_user_context(
    tramp: usize,
    entry: usize,
    user_sp: usize,
    kstack_top: usize,
    sstatus: usize,
) -> Context {
    let mut c = Context::zeroed();
    c.ra = tramp;
    c.sp = kstack_top & !0xF;
    c.s[0] = entry;
    c.s[1] = user_sp & !0xF;
    c.s[2] = sstatus;
    c
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p kernel-arch-riscv64 forge_user_context`
Expected: PASS.

- [ ] **Step 5: Run the whole arch test suite (nothing regressed)**

Run: `cargo test -p kernel-arch-riscv64`
Expected: PASS (existing `context_layout`, `user_sstatus_*`, `pick_next` tests still green).

- [ ] **Step 6: Commit**

```bash
git add arch/riscv64/src/task.rs
git commit -m "feat(task): TaskState::Exited and pure forge_user_context"
```

---

## Task 2: `yield` syscall (number 3)

**Files:**
- Modify: `arch/riscv64/src/syscall.rs`
- Test: `arch/riscv64/src/syscall.rs` (`tests` module)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `arch/riscv64/src/syscall.rs`:

```rust
    #[test]
    fn decodes_yield_syscall() {
        assert_eq!(decode_syscall(3), Syscall::Yield);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p kernel-arch-riscv64 decodes_yield_syscall`
Expected: FAIL — no variant `Yield`.

- [ ] **Step 3: Add the `Yield` variant, decode arm, `Outcome`, and dispatch arm**

In `arch/riscv64/src/syscall.rs`, extend the `Syscall` enum:

```rust
    /// `exit(code)` — terminate the calling task.
    Exit,
    /// `yield()` — give up the CPU to the next ready task.
    Yield,
    /// An unrecognized syscall number (a user bug, not a kernel bug).
    Unknown(usize),
```

Extend `decode_syscall`:

```rust
    match a7 {
        1 => Syscall::Print,
        2 => Syscall::Exit,
        3 => Syscall::Yield,
        n => Syscall::Unknown(n),
    }
```

Extend the gated `Outcome` enum:

```rust
#[cfg(target_arch = "riscv64")]
pub enum Outcome {
    /// Resume the user task (the handler advances `sepc` past the `ecall`).
    Resume,
    /// The task asked to exit with this code.
    Exit(usize),
    /// The task asked to yield; the handler advances `sepc`, then reschedules.
    Yield,
}
```

Extend the `match` in `dispatch`:

```rust
        Syscall::Exit => Outcome::Exit(a0),
        Syscall::Yield => Outcome::Yield,
        Syscall::Unknown(_) => {
            frame.regs[9] = usize::MAX; // -1: unknown syscall
            Outcome::Resume
        }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p kernel-arch-riscv64 decodes_yield_syscall`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/syscall.rs
git commit -m "feat(syscall): add yield syscall (a7=3)"
```

---

## Task 3: scheduler — `spawn_user`, `exit_current`, `user_trampoline`, drop the standalone path

**Files:**
- Modify: `arch/riscv64/src/sched.rs`
- Test: `arch/riscv64/src/sched.rs` (`tests` module)

- [ ] **Step 1: Write the failing test (pick_next skips Exited)**

Add to the `tests` module in `arch/riscv64/src/sched.rs`:

```rust
    #[test]
    fn pick_next_skips_exited_slots() {
        // current = 0 running; slot 1 Exited, slot 2 Ready -> pick 2.
        let mut s = three_tasks(0);
        s.tasks[1].as_mut().unwrap().state = TaskState::Exited;
        assert_eq!(s.pick_next(), 2);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p kernel-arch-riscv64 pick_next_skips_exited_slots`
Expected: FAIL — no variant `TaskState::Exited` is in scope until Task 1 is built; if Task 1 is done it FAILS only if logic is wrong. With Task 1 done and `pick_next` unchanged it should actually PASS (Exited != Ready is skipped). Run it: if PASS, this test simply locks in the behavior — proceed.

> Note: `pick_next` already returns only `Ready` slots, so `Exited` is skipped for free. This test pins that guarantee against future edits. No `pick_next` change is needed.

- [ ] **Step 3: Bump `MAX_TASKS`**

In `arch/riscv64/src/sched.rs`, replace the `MAX_TASKS` constant and its doc comment:

```rust
/// Maximum concurrent tasks: the 3b-i demo runs five (idle + four U-mode
/// tasks) plus one slot of headroom.
pub const MAX_TASKS: usize = 6;
```

Update `Scheduler::new` to initialize six slots:

```rust
    pub const fn new() -> Self {
        Self { tasks: [None, None, None, None, None, None], current: 0 }
    }
```

- [ ] **Step 4: Rename `enter_user_asm` to `user_trampoline`**

In the gated `global_asm!` block, rename the label and its doc comment. Replace the block that currently defines `enter_user_asm` with:

```rust
// First entry into U-mode for a freshly spawned U-mode task. Reached via
// `switch_context` "returning" into it with the context `spawn_user`
// forged: s0 = user entry (-> sepc), s1 = user sp, s2 = user sstatus,
// sp = this task's kernel/trap-stack top (we run on it now). We arm
// sscratch with that top (so the user's first trap swaps onto it), load the
// user CSRs, switch to the user stack, and `sret`. The U-mode analogue of
// `task_trampoline`.
#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(
    r#"
    .section .text
    .global user_trampoline
user_trampoline:
    csrw sscratch, sp       # sscratch = trap-stack top (for the U->S trap)
    csrw sepc, s0           # user entry point
    csrw sstatus, s2        # SPP = 0, SPIE = 1
    mv sp, s1               # switch to the user stack
    sret                    # -> U-mode at the entry point
"#
);

#[cfg(target_arch = "riscv64")]
extern "C" {
    fn user_trampoline();
}
```

- [ ] **Step 5: Replace `enter_user`/`terminate_user`/`USER_RETURN`/`USER_EXIT` with `spawn_user`/`exit_current`**

Delete the `USER_RETURN`, `USER_EXIT` statics and the entire `enter_user` and `terminate_user` functions. Keep `use crate::task::ExitReason;` (still used by `exit_current`). Add, in their place:

```rust
#[cfg(target_arch = "riscv64")]
use crate::task::ExitReason;

/// Register a U-mode task: forge a `Ready` slot whose first `switch_context`
/// lands in `user_trampoline` and `sret`s to U-mode. `entry` must live in a
/// `U`-mapped executable page; `user_sp` is the top of a `U`-mapped stack;
/// `kstack_top` is the top of a trusted kernel stack the task's traps land
/// on. Panics if there is no free slot (a static configuration bug).
#[cfg(target_arch = "riscv64")]
pub fn spawn_user(
    name: &'static str,
    entry: extern "C" fn() -> !,
    user_sp: usize,
    kstack_top: usize,
) {
    SCHED.with(|s| {
        let slot = s
            .tasks
            .iter()
            .position(Option::is_none)
            .expect("scheduler full");
        let context = crate::task::forge_user_context(
            user_trampoline as *const () as usize,
            entry as *const () as usize,
            user_sp,
            kstack_top,
            crate::task::user_sstatus(crate::csr::sstatus_read()),
        );
        s.tasks[slot] = Some(Task {
            context,
            state: TaskState::Ready,
            stack_top: kstack_top,
            name,
        });
    });
}

/// Terminate the running task and switch to the next ready one. Called from
/// the trap handler for the `exit` syscall (`Exited`) and fatal U-mode
/// faults (`Killed`). Prints the outcome, marks the current slot `Exited`,
/// and `switch_context`s away — never resuming the dead slot, so its trap
/// frame and stacks are simply abandoned (reaping is deferred). The idle
/// task is always `Ready`, so a successor always exists.
#[cfg(target_arch = "riscv64")]
pub fn exit_current(reason: ExitReason) -> ! {
    // SAFETY: runs inside the trap handler with interrupts off; single hart.
    let switch = SCHED.with(|s| {
        let current = s.current;
        match reason {
            ExitReason::Exited(code) => {
                crate::println!("sched: task '{}' exited (code {code})", s.tasks[current].as_ref().unwrap().name)
            }
            ExitReason::Killed(cause) => {
                crate::println!("sched: task '{}' killed by {cause:?}", s.tasks[current].as_ref().unwrap().name)
            }
        }
        s.tasks[current].as_mut().unwrap().state = TaskState::Exited;
        let next = s.pick_next();
        assert_ne!(next, current, "exit_current: no runnable successor (idle must be Ready)");
        s.tasks[next].as_mut().unwrap().state = TaskState::Running;
        s.current = next;
        // Save into the dying slot's own context (never resumed).
        let old = core::ptr::addr_of_mut!(s.tasks[current].as_mut().unwrap().context);
        let new = core::ptr::addr_of!(s.tasks[next].as_ref().unwrap().context);
        (old, new)
    });
    // SAFETY: distinct 'static contexts; the old (dying) slot is never run
    // again so its saved state is irrelevant; control resumes in `next`.
    unsafe { switch_context(switch.0, switch.1) };
    unreachable!("exit_current resumes in the next task, never here")
}
```

- [ ] **Step 6: Verify the arch crate still builds for both targets and host tests pass**

Run: `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf`
Expected: builds (no references to the deleted `enter_user`/`terminate_user`/`enter_user_asm` remain — they are updated in Task 4 / Task 6; if the build fails only because `kernel` (the binary) still calls them, that is expected and fixed in Task 6. Build the arch lib specifically here.)

Run: `cargo test -p kernel-arch-riscv64`
Expected: PASS (includes `pick_next_skips_exited_slots`).

- [ ] **Step 7: Commit**

```bash
git add arch/riscv64/src/sched.rs
git commit -m "feat(sched): spawn_user, exit_current, user_trampoline; drop standalone enter_user"
```

---

## Task 4: trap handler — yield, scheduler-based termination, U-mode preemption proof

**Files:**
- Modify: `arch/riscv64/src/trap.rs`

- [ ] **Step 1: Add the one-shot U-mode-preemption flag**

In `arch/riscv64/src/trap.rs`, near `EXPECTING_WX_FAULT` (which already imports `AtomicBool`/`Ordering`), add:

```rust
/// One-shot: set the first time a timer tick preempts a task that was
/// running in U-mode. The smoke test greps for the line this gates — the
/// 3b-i proof that a U-mode task is schedulable *and* preemptible.
#[cfg(target_arch = "riscv64")]
static USER_PREEMPTED: AtomicBool = AtomicBool::new(false);
```

- [ ] **Step 2: Announce U-mode preemption in the timer arm**

In `trap_handler`, replace the `Cause::SupervisorTimer` arm with:

```rust
        Cause::SupervisorTimer => {
            crate::timer::on_tick();
            // 3b-i proof: the first tick that interrupts a U-mode task is
            // announced once. `from_user` reads the interrupted frame's SPP.
            if from_user(frame) && !USER_PREEMPTED.swap(true, Ordering::AcqRel) {
                crate::println!("sched: U-mode task preempted by timer");
            }
            // Tick-policy hook: preempt the running task.
            crate::sched::preempt();
        }
```

- [ ] **Step 3: Route `exit`/`yield` and U-mode faults through the scheduler**

Replace the `Cause::UserEcall` arm with:

```rust
        Cause::UserEcall => {
            // A syscall from U-mode. ecall does not advance the PC, so for
            // Resume/Yield we step sepc past the 4-byte ecall; Exit never
            // returns here (exit_current switches to the next task).
            match crate::syscall::dispatch(frame) {
                crate::syscall::Outcome::Resume => frame.sepc += 4,
                crate::syscall::Outcome::Yield => {
                    frame.sepc += 4;
                    crate::sched::yield_now();
                }
                crate::syscall::Outcome::Exit(code) => {
                    crate::sched::exit_current(crate::task::ExitReason::Exited(code))
                }
            }
        }
```

Replace the three U-mode fault routings (the `terminate_user` calls) with `exit_current`:

```rust
        Cause::InstructionPageFault | Cause::LoadPageFault if from_user(frame) => {
            // A U-mode task reached for memory it does not own: contain it.
            crate::sched::exit_current(crate::task::ExitReason::Killed(cause));
        }
        Cause::InstructionPageFault => fatal("instruction page fault", frame),
        Cause::LoadPageFault => fatal("load page fault", frame),
        Cause::StorePageFault => {
            if from_user(frame) {
                crate::sched::exit_current(crate::task::ExitReason::Killed(Cause::StorePageFault));
            } else if EXPECTING_WX_FAULT.swap(false, Ordering::AcqRel) {
                crate::println!("trap: W^X store fault at {:#x} (probe)", frame.stval);
                frame.sepc += instruction_len_at(frame.sepc);
            } else {
                fatal("store page fault", frame);
            }
        }
```

- [ ] **Step 4: Verify the arch crate builds for the bare target and host tests pass**

Run: `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf`
Expected: builds (all `terminate_user` references gone; `exit_current`/`yield_now` exist).

Run: `cargo test -p kernel-arch-riscv64`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/trap.rs
git commit -m "feat(trap): yield + scheduler-based termination; announce U-mode preemption"
```

---

## Task 5: update the smoke test to the 3b-i milestones (red)

**Files:**
- Modify: `tools/test-qemu.ps1`

- [ ] **Step 1: Replace the milestone patterns and messages**

In `tools/test-qemu.ps1`, replace the `$mustMatch` array with the 3b-i milestones (2a/2b kept verbatim; the 3a standalone-user and 2c A/B/C lines are replaced by the new run-queue demo):

```powershell
$mustMatch = @(
    "hello world",
    "trap: breakpoint",
    "survived breakpoint",
    "paging: sv39 on",
    "wx: rodata write blocked",
    "frames: alloc/free ok",
    "(?s)user: ping 0.*user: pong 0.*user: ping 1.*user: pong 1",
    "sched: task 'ping' exited \(code 0\)",
    "sched: task 'pong' exited \(code 0\)",
    "sched: task 'bad' killed by LoadPageFault",
    "sched: U-mode task preempted by timer",
    "tick: 2(?!\d)"
)
```

Also update the header comment block and the two result messages to describe the 3b-i milestones (replace the Phase 2c/3a sentences with: "the Phase 3b-i milestones — two U-mode tasks round-robin via the yield syscall and exit cleanly, a bad U-mode task is contained while the scheduler keeps running, and a U-mode task is preempted by the timer").

- [ ] **Step 2: Run the smoke test — expect RED**

Run: `./tools/test-qemu.ps1`
Expected: FAIL — the binary still has the 3a/2c demo, so the new `user: ping`/`sched: task ...`/`U-mode task preempted` lines are missing. (If the build itself fails because `kernel` still calls the deleted `enter_user`, that is also expected here and fixed in Task 6.)

- [ ] **Step 3: Commit (red test pinned)**

```bash
git add tools/test-qemu.ps1
git commit -m "test: smoke test asserts 3b-i run-queue milestones (red)"
```

---

## Task 6: kmain demo — idle + four U-mode tasks in one run queue (green)

**Files:**
- Modify: `kernel/src/main.rs`

- [ ] **Step 1: Update the greeting and imports**

In `kernel/src/main.rs`, change the phase string:

```rust
        println!("{GREETING} from {PROJECT_NAME} - Phase 3b-i (hart {hartid})");
```

Update the `use` line to drop `ExitReason` (no longer used in `kmain`) and keep the rest:

```rust
    use kernel_arch_riscv64::{mem, println, sched, timer, trap};
```

- [ ] **Step 2: Replace the 3a standalone-user block and the 2c A/B/C spawns**

Replace everything from the `// Phase 3a: run the embedded U-mode program ...` comment down to and including `sched::enter()` (the end of `kmain`) with:

```rust
        // Phase 3b-i: one run queue holding a kernel idle task and four
        // U-mode tasks. ping/pong cooperate via the yield syscall and exit
        // cleanly; bad reaches into kernel memory and is contained; hog
        // never yields, so the first timer tick preempts a U-mode task.
        // Spawn order puts ping in slot 0, so enter() runs it first.
        // addr_of! forms each static stack's top address WITHOUT a reference
        // (no unsafe, no static_mut_refs lint) — the existing 2c/3a pattern.
        sched::spawn_user("ping", user_ping,
            core::ptr::addr_of!(US_PING) as usize + USER_STACK_SIZE,
            core::ptr::addr_of!(KS_PING) as usize + TASK_STACK);
        sched::spawn_user("pong", user_pong,
            core::ptr::addr_of!(US_PONG) as usize + USER_STACK_SIZE,
            core::ptr::addr_of!(KS_PONG) as usize + TASK_STACK);
        sched::spawn_user("hog", user_hog,
            core::ptr::addr_of!(US_HOG) as usize + USER_STACK_SIZE,
            core::ptr::addr_of!(KS_HOG) as usize + TASK_STACK);
        sched::spawn_user("bad", user_bad,
            core::ptr::addr_of!(US_BAD) as usize + USER_STACK_SIZE,
            core::ptr::addr_of!(KS_BAD) as usize + TASK_STACK);
        sched::spawn("idle", idle, core::ptr::addr_of!(IDLE_STACK) as usize + TASK_STACK);

        timer::start();
        println!("(scheduler starting; heartbeat ~1/s; exit QEMU with Ctrl-A then X)");
        sched::enter()
```

- [ ] **Step 3: Replace the stack statics**

Replace the 3a/2c stack statics (`STACK_A`/`STACK_B`/`STACK_C`, `TRAP_STACK`, `USER_STACK`, `USER_MSG`) with per-task kernel and user stacks. Keep `TASK_STACK` and `USER_STACK_SIZE`. Add type aliases for the closures above:

```rust
    /// Per-task kernel stack size (also the trap stack a U-mode task's
    /// traps land on). 16 KiB; per-task guard pages stay deferred.
    const TASK_STACK: usize = 16 * 1024;
    type KStack = [u8; TASK_STACK];

    /// One kernel/trap stack per U-mode task, plus one for the idle task.
    static mut KS_PING: KStack = [0; TASK_STACK];
    static mut KS_PONG: KStack = [0; TASK_STACK];
    static mut KS_HOG: KStack = [0; TASK_STACK];
    static mut KS_BAD: KStack = [0; TASK_STACK];
    static mut IDLE_STACK: KStack = [0; TASK_STACK];

    /// U-mode task stacks live in `.user_data` so mem::init maps them RW-U.
    const USER_STACK_SIZE: usize = 8 * 1024;
    type UStack = [u8; USER_STACK_SIZE];
    #[link_section = ".user_data"]
    static mut US_PING: UStack = [0; USER_STACK_SIZE];
    #[link_section = ".user_data"]
    static mut US_PONG: UStack = [0; USER_STACK_SIZE];
    #[link_section = ".user_data"]
    static mut US_HOG: UStack = [0; USER_STACK_SIZE];
    #[link_section = ".user_data"]
    static mut US_BAD: UStack = [0; USER_STACK_SIZE];

    /// Messages the U-mode tasks ask the kernel to print. In `.user_data`
    /// (R-U) so the confused-deputy guard accepts their pointers and the
    /// SUM-window copy can read them. Each ends in '\n'; lengths are exact.
    #[link_section = ".user_data"]
    static PING_MSG: [[u8; 13]; 2] = [*b"user: ping 0\n", *b"user: ping 1\n"];
    #[link_section = ".user_data"]
    static PONG_MSG: [[u8; 13]; 2] = [*b"user: pong 0\n", *b"user: pong 1\n"];
    #[link_section = ".user_data"]
    static HOG_MSG: [[u8; 13]; 2] = [*b"user: hog  0\n", *b"user: hog  1\n"];
```

- [ ] **Step 4: Add the `yield` syscall stub and keep `sys_print`/`sys_exit`**

Keep the existing `sys_print` and `sys_exit` stubs unchanged. Add a `sys_yield` stub next to them:

```rust
    /// Yield syscall (a7 = 3): give up the CPU; returns when rescheduled.
    ///
    /// # Safety
    /// Always sound; the kernel reschedules and resumes this task later.
    #[inline(always)]
    unsafe fn sys_yield() {
        core::arch::asm!("ecall", in("a7") 3usize, options(nostack));
    }
```

- [ ] **Step 5: Replace the user/worker task functions**

Remove `user_good`, `user_bad` (3a), `task_a`/`task_b`/`task_c`, `worker`, and the `HOGGING` static. Add the 3b-i tasks (all `.user_text`, R-X-U, calling only inlined syscall stubs so they never touch a non-U page), plus the kernel `idle` task:

```rust
    /// Cooperating U-mode task: print two lines, yielding to its peer
    /// between each, then exit cleanly.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn user_ping() -> ! {
        // SAFETY: PING_MSG is in .user_data (R-U); we pass addresses the
        // kernel validates and reads. yield/exit are always sound.
        unsafe {
            sys_print(PING_MSG[0].as_ptr(), PING_MSG[0].len());
            sys_yield();
            sys_print(PING_MSG[1].as_ptr(), PING_MSG[1].len());
            sys_yield();
            sys_exit(0)
        }
    }

    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn user_pong() -> ! {
        // SAFETY: see `user_ping`.
        unsafe {
            sys_print(PONG_MSG[0].as_ptr(), PONG_MSG[0].len());
            sys_yield();
            sys_print(PONG_MSG[1].as_ptr(), PONG_MSG[1].len());
            sys_yield();
            sys_exit(0)
        }
    }

    /// The hog: two cooperative rounds, then spin forever without yielding.
    /// The timer must preempt it (it is the sole non-yielder once ping/pong
    /// exit), which proves a U-mode task is preemptible.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn user_hog() -> ! {
        // SAFETY: see `user_ping`.
        unsafe {
            sys_print(HOG_MSG[0].as_ptr(), HOG_MSG[0].len());
            sys_yield();
            sys_print(HOG_MSG[1].as_ptr(), HOG_MSG[1].len());
            sys_yield();
        }
        loop {
            core::hint::spin_loop();
        }
    }

    /// The misbehaving U-mode task: read a kernel address. The kernel page
    /// is non-U, so the U-mode load faults and the kernel contains the task
    /// (it never reaches the exit below); the scheduler keeps running.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn user_bad() -> ! {
        // 0x80200000 is the kernel .text base (mapped R-X-G, no U bit).
        let kernel_addr = 0x8020_0000 as *const u8;
        // SAFETY: the deliberate boundary violation; the volatile read
        // faults in U-mode before completing. Control never returns here.
        let _ = unsafe { core::ptr::read_volatile(kernel_addr) };
        unsafe { sys_exit(0) } // unreachable: the read above faults first
    }

    /// The idle kernel (S-mode) task: cooperatively yield when other tasks
    /// are ready; `wfi`-sleep when it is the only runnable task. Never
    /// exits, so it is always a valid successor for `exit_current` and keeps
    /// the system alive after every U-mode task has finished.
    extern "C" fn idle() -> ! {
        loop {
            sched::yield_now();
            // SAFETY: wfi just waits for the next interrupt (the timer).
            unsafe { core::arch::asm!("wfi") };
        }
    }
```

Keep `park()` (used by the panic handler) and the panic handler unchanged.

- [ ] **Step 6: Build the kernel binary**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: builds with no errors (no references to deleted items; `spawn_user`/`spawn`/`sys_yield` all resolve).

- [ ] **Step 7: Run the smoke test — expect GREEN**

Run: `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS` — all 12 patterns matched within 30 s, including the ping/pong interleave, both clean exits, the contained `bad` task, and the U-mode preemption line.

- [ ] **Step 8: Commit**

```bash
git add kernel/src/main.rs
git commit -m "feat: Phase 3b-i live - U-mode tasks in the run queue, yield syscall, scheduled containment"
```

---

## Task 7: docs — short learning note, roadmap, glossary; final verification

**Files:**
- Create: `docs/learning/0007-user-scheduling.md`
- Modify: `docs/roadmap/roadmap.md`
- Modify: `docs/glossary.md`
- Modify: `docs/learning/README.md` (index, if it lists notes)

- [ ] **Step 1: Write a short learning note**

Create `docs/learning/0007-user-scheduling.md` — a brief summary (per project preference: summary, not tutorial):

```markdown
# 0007 — U-mode tasks in the run queue (Phase 3b-i)

**One-line:** kernel and U-mode tasks now share one round-robin run queue.

## What changed
- 3a ran a U-mode task *standalone* (`enter_user` parked `kmain`, `sret`ed,
  `terminate_user` returned). 3b-i folds that into the 2c scheduler: a
  U-mode task is an ordinary slot.
- `spawn_user` forges a first-run `Context` that `switch_context` "returns"
  into via `user_trampoline` (3a's `enter_user_asm`, renamed) — the U-mode
  analogue of `task_trampoline`.
- `TaskState::Exited` + `exit_current` replace `terminate_user`:
  termination is now "mark Exited, switch to the next task," not a return
  to `kmain`. Containment is strictly stronger — the scheduler survives and
  keeps running the other tasks.
- A `yield` syscall (a7 = 3) lets U-mode tasks cooperate (they can't call
  the scheduler directly).

## The key idea
Per-task privilege rides the **saved trapframe** for free: `switch_context`
and the trap entry/exit asm are unchanged. Every task resumes through the
trap return, which restores `sepc`/`sstatus` (privilege) and re-arms
`sscratch` from the per-task kernel-stack top. The only new entry path is
the *first* run of a U-mode task.

## Proof (smoke test)
Two U-mode tasks round-robin via `yield` and exit cleanly; a bad U-mode
task is contained while the scheduler keeps running; the first timer tick
preempts a U-mode task. Next: 3b-ii (per-address-space isolation).
```

- [ ] **Step 2: Update the roadmap**

In `docs/roadmap/roadmap.md`, replace the `### Phase 3b — Capabilities & IPC` heading block with a decomposition note and the three sub-phases (keep 3c as-is). Use:

```markdown
### Phase 3b — Capabilities & IPC

Decomposed (2026-06-14) into three sub-phases, mirroring 2a/2b/2c and
3a/3b/3c. The scheduling substrate comes first (something must *hold* a
capability and *be* a component), then isolation, then the capability/IPC
finale. Each sub-phase gets its own design → plan → build cycle.

#### Phase 3b-i — U-mode tasks in the run queue  *(done — 2026-06-14)*

- **Goal:** kernel and U-mode tasks share one round-robin run queue;
  per-task privilege is carried by the saved trapframe.
- **You learn:** forging a U-mode task's first run, scheduler-based
  termination, the `yield` syscall (see [learning note 0007](../learning/0007-user-scheduling.md)).
- **Done when:** `./tools/test-qemu.ps1` observes two U-mode tasks
  round-robin via `yield` and exit cleanly, a bad U-mode task contained
  while the scheduler keeps running, and a U-mode task preempted by the
  timer — all in one boot, alongside the 2a/2b/2c milestones.

#### Phase 3b-ii — Per-address-space isolation

- **Goal:** each component gets its own `satp`; the kernel is mapped into
  every address space; `satp` swaps on context switch.
- **Done when:** two tasks cannot read each other's memory.

#### Phase 3b-iii — Capabilities + synchronous IPC + blocking

- **Goal:** unforgeable capability tokens, capability-checked syscalls, a
  synchronous send/recv endpoint, and blocking/wait-queue task states.
- **Done when:** two isolated components communicate only through
  capability-checked IPC.
```

- [ ] **Step 3: Add glossary entries for the genuinely new terms**

In `docs/glossary.md`, add entries (alphabetical, matching the file's existing format) for: **`yield` syscall** (a U-mode task cooperatively gives up the CPU through the kernel), **task `Exited` state** (a terminated slot the scheduler skips; stacks not yet reclaimed), and **`user_trampoline`** (the first-run entry that `sret`s a freshly spawned U-mode task into U-mode — the U-mode analogue of `task_trampoline`). Reuse the existing 3a privilege/trap-stack/`sscratch` entries.

- [ ] **Step 4: Full verification — host tests + smoke test together**

Run: `cargo test -p kernel-arch-riscv64`
Expected: PASS (all host unit tests).

Run: `cargo build --workspace`
Expected: builds (host stub + libs).

Run: `./tools/check-references.ps1`
Expected: PASS (no broken doc cross-references — the new learning note and roadmap links resolve).

Run: `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS`.

- [ ] **Step 5: Commit**

```bash
git add docs/learning/0007-user-scheduling.md docs/roadmap/roadmap.md docs/glossary.md docs/learning/README.md
git commit -m "docs: Phase 3b-i learning note, roadmap decomposition, glossary terms"
```

---

## Done-when checklist (maps to spec §1)

- [ ] Two U-mode tasks (`ping`/`pong`) round-robin via the `yield` syscall and both exit cleanly — smoke patterns: interleave + two `exited (code 0)` lines.
- [ ] A U-mode task (`hog`) is preempted by the timer — smoke pattern: `U-mode task preempted by timer`.
- [ ] A bad U-mode task is contained while the scheduler keeps running — smoke pattern: `killed by LoadPageFault` (and the surrounding milestones still print).
- [ ] A kernel (`idle`, S-mode) task and U-mode tasks coexist in one run queue — they do, by construction; the boot completing all milestones proves it.
- [ ] All 2a/2b milestones still observed; host unit tests green.
```
