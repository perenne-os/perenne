# Kernel — Phase 2c Design: Basic Scheduling

- **Date:** 2026-06-14
- **Status:** Draft — awaiting review
- **Scope of this document:** Phase 2c only (context switching between
  in-kernel tasks, cooperative then preemptive). Decomposition context
  lives in the [2a spec](2026-06-10-phase-2a-traps-design.md) and the
  [roadmap](../../roadmap/roadmap.md); memory groundwork is in the
  [2b spec](2026-06-10-phase-2b-memory-design.md).

---

## 1. Goal

The kernel runs several **in-kernel tasks** (kernel threads) and switches
the CPU between them. Switching is built in two layers: first
**cooperative** (`yield_now()`, a voluntary hand-off), then **preemptive**
(the Phase 2a timer tick forcibly deschedules a running task). A
round-robin **run queue** decides who runs next.

**You learn:** what a context switch actually saves and restores, why a
*voluntary* switch saves only the callee-saved registers while an
*involuntary* (interrupt-driven) one inherits the full trap frame, how a
run queue picks the next task, and the tick-policy hook that turns the
heartbeat into a scheduler clock.

**Done when** `./tools/test-qemu.ps1` observes, in one boot, *in addition
to all Phase 2a/2b milestones* (greeting, breakpoint recovery, paging
line, W^X block, frame round-trip, ≥ 2 ticks):

1. a **cooperative round-robin** among 3 tasks — an interleaving of their
   output that only real context switching can produce, and
2. a **preemption** — a task running a tight loop that never yields is
   descheduled by a timer tick, and another task runs.

## 2. Non-goals (deferred)

- **Task exit / reaping** — tasks run forever in 2c. What happens to a
  finished task's stack and its removal from the run queue
  (`Exited`/zombie states, stack reclamation) is its own concept; deferred
  to a later phase. A task entry function that *returns* is a 2c bug and
  panics.
- **Blocking / sleeping / wait queues** — no task ever waits on anything
  yet, so the task state machine is only `Ready`/`Running`. Blocking
  arrives with the first thing worth blocking on (IPC, Phase 3).
- **Kernel heap / `GlobalAlloc`** — task stacks are `static` arrays in
  `.bss`, not heap allocations, so 2c needs no dynamic allocator. The heap
  is introduced when something genuinely needs runtime-sized allocation.
- **Priorities / fairness beyond round-robin** — every task is equal and
  always ready; plain round-robin is enough to prove the mechanism. A
  real policy (priorities, time-slice accounting) is a later concern.
- **Dedicated trap stack** — traps run on the current task's kernel stack
  (see §3.5). A separate trap stack and `sscratch`-based stack swapping
  are a Phase 3 concern, needed when user/supervisor privilege transitions
  arrive; there is no such transition in 2c (all tasks are S-mode).
- **Per-task stack guard pages** — only the boot/idle stack keeps the 2b
  guard page. Per-task guards add unmapped pages and mapping logic without
  teaching anything new; deferred (see §3.5).
- **SMP / multi-hart scheduling** — single hart. The scheduler lives in
  the same `SingleHartCell` primitive 2b introduced; multi-hart run queues
  and load balancing are far future.

## 3. Design

### 3.1 Components

New `task` and `sched` modules in the arch crate, following the
`trap.rs`/`mem` pattern: pure logic ungated (host-testable), hardware
access (the switch assembly, the static scheduler instance) gated to
`target_arch = "riscv64"`. The run-queue *selection math* is pure and
host-tested; only the asm and the static state are gated.

| Component | Location | Responsibility |
|-----------|----------|----------------|
| Task & context | `arch/riscv64/src/task.rs` *(new)* | `Task` (context + state + stack + name), `Context` (callee-saved register block), `TaskState`, and the forged initial context for a never-run task. |
| Scheduler core | `arch/riscv64/src/sched.rs` *(new)* | Pure run-queue: fixed `[Option<Task>; MAX_TASKS]` + `current` index, round-robin `pick_next` (host-tested). The gated static `Scheduler` in a `SingleHartCell`, `spawn`, `enter` (first switch via the bootstrap context), `yield_now`, and `schedule_from_trap`. |
| Context switch asm | `arch/riscv64/src/sched.rs` (gated) | `switch_context(old: *mut Context, new: *const Context)`: save `ra`/`sp`/`s0–s11` to `*old`, load from `*new`, `ret`. ~30 lines, field-order contract with `Context`. |
| Timer hook | `arch/riscv64/src/timer.rs` | `on_tick()` keeps the heartbeat, then calls the scheduler's preemption entry — the "tick-policy hook". |
| Trap integration | `arch/riscv64/src/trap.rs` | The `SupervisorTimer` arm drives preemption: after `timer::on_tick()`, the next task's frame is resumed via `sret` (frame/sp swap). |
| Kernel binary | `kernel/src/main.rs` | `kmain`: after 2b init, `spawn` 3 tasks, then enter the scheduler; the demo proves cooperative rotation and one preemption. |

### 3.2 Task & context representation

A `Task` is a kernel thread — a static kernel stack plus a saved context:

```
#[repr(C)]
struct Context {        // callee-saved only: the cooperative-switch frame
    ra,                 // return address — where switch_context resumes
    sp,                 // the task's stack pointer
    s: [usize; 12],     // s0..s11
}                       // 14 × 8 = 112 bytes; field order IS the asm contract

enum TaskState { Ready, Running }

struct Task {
    context: Context,           // saved registers while not running
    state: TaskState,
    stack: &'static mut [u8],   // a slice of the static STACKS array
    name: &'static str,         // identifies the task in demo output
}
```

- **Stacks** are `static mut STACKS: [[u8; STACK_SIZE]; MAX_TASKS]` in
  `.bss`. `STACK_SIZE` is 16 KiB. No heap — exactly the 2b-deferred-heap
  promise kept.
- **The forged initial context** is the instructive part: a task that has
  never run still needs a `Context` the switch can restore *into*. `spawn`
  builds one with `ra` = the task's entry function, `sp` = top of its
  stack (16-aligned per the RISC-V ABI), `s0–s11` = 0. The first switch
  into the task "returns" (`ret` in the asm) straight into its entry
  point. There is no saved frame to restore the first time — we
  manufacture one.
- A `#[repr(C)]` + `size_of` host test pins the `Context` layout, exactly
  as `TrapFrame` does, because the field order is a contract with the
  switch assembly.

### 3.3 The run queue (pure core)

```
struct Scheduler {
    tasks: [Option<Task>; MAX_TASKS],   // MAX_TASKS = 4 (3 demo + headroom)
    current: usize,                     // index of the running task
}
```

- The run queue holds exactly the **3 demo tasks** (`current` starts at
  the first). The bootstrap/`kmain` context is *not* a queue slot — it is
  a standalone save-once `Context` used only for the first switch (§3.4),
  so the rotation is purely 3-way and the done-when "among 3 tasks" is
  unambiguous. `MAX_TASKS = 4` leaves one slot of headroom.
- **Policy: round-robin.** `pick_next(current)` scans from `current + 1`
  (wrapping) for the next `Ready` slot. With every task always ready this
  is a clean rotation — proving the queue is a real queue, not an A↔B
  toggle.
- **Pure, host-tested:** the selection math takes `current` and the slot
  occupancy/state and returns the next index — no pointers, no CSRs. Tests
  cover 3-task rotation, wrap-around, skipping the current task, and the
  single-task case (returns itself, i.e. nobody else to run).
- The gated static instance lives in a `SingleHartCell` (2b's primitive):
  one hart, and the re-entry tripwire fires if a future change lets the
  scheduler re-enter itself.

### 3.4 The two switch paths

**Cooperative — `yield_now()`:**

```
yield_now():
    next = scheduler.pick_next(current)
    if next == current: return          // nobody else ready
    old.state = Ready; new.state = Running; current = next
    switch_context(&mut old.context, &new.context)   // the asm
```

`switch_context(old, new)` stores `ra`/`sp`/`s0–s11` into `*old`, loads
them from `*new`, and `ret`s. Saving *only* callee-saved registers is
correct because Rust's calling convention already treats caller-saved
registers as clobbered across any call — the caller of `yield_now` has
preserved anything it still needs. This is the whole point of the
two-path design.

**Preemptive — timer-driven:**

The timer interrupt already reaches `trap_handler` as
`Cause::SupervisorTimer`, which calls `timer::on_tick()`. The tick keeps
its heartbeat bookkeeping, then invokes the scheduler's preemption entry.
Because the full `TrapFrame` (all 31 GPRs) is already saved on the current
task's stack by `__trap_entry`, preemption switches by **changing which
frame `sret` returns through**: the current task's `sp` is saved into its
`Context`, the next task's saved `sp` is loaded, and trap exit pops a
*different* task's `TrapFrame`. A preempted task's frame stays parked on
its own stack until it is scheduled again.

The contrast is the lesson: a voluntary `yield` saves 14 registers because
the compiler saved the rest; an involuntary preemption inherits all 31
from the `TrapFrame` because the interrupted code volunteered nothing.

**The first switch — `enter()`:** `switch_context` needs an `old`
`Context` to save into, but the very first switch has no task to save. The
classic resolution (xv6's per-CPU scheduler context): a standalone
**bootstrap `Context`** that the first switch saves `kmain`'s registers
into and never restores. `enter()` calls
`switch_context(&mut bootstrap_ctx, &tasks[0].context)` and never returns
to `kmain`. The boot stack and `kmain`'s frame are simply abandoned —
correct, because a kernel never returns from `kmain` anyway.

### 3.5 Stack model and the trap-stack question

2b deferred the "trap-stack question" to 2c. The resolution:
**each task runs on its own kernel stack, and traps execute on the current
task's stack.** There is no separate trap stack, because 2c has no
privilege transition — all tasks are S-mode kernel threads, so there is no
moment where the hart arrives on an untrusted stack. `sscratch`-based
stack swapping and a dedicated trap stack are a Phase 3 concern (user
space).

- The **boot stack** is the bootstrap context's stack. `kmain` runs on it,
  sets everything up, spawns the 3 demo tasks onto their own static stacks,
  and calls `enter()` — which switches away and never returns (§3.4). The
  boot stack then lies dormant (still guarded by 2b's guard page); no task
  ever uses it again.
- **Preemption on a per-task stack works** because `__trap_entry` pushes
  the `TrapFrame` onto whatever stack is current — i.e. the running task's
  — and the frame/sp swap (§3.4) makes `sret` resume the next task's
  parked frame on *its* stack. The trap frame travels with the task.

**Honest caveat (carried, not hidden):** 2b's guard-page recursion caveat
(a stack overflow's own trap re-faults on the overflowed stack) is
*narrowed* but not eliminated. Only the boot/idle stack keeps a guard
page; the 3 demo task stacks have none. A demo task that overflowed its
16 KiB stack would corrupt memory below it silently. This is acceptable
for 2c: the demo tasks are tiny print loops nowhere near 16 KiB, per-task
guards add N unmapped pages and mapping logic that teaches nothing new,
and the full fix belongs with the same Phase 3 work that introduces the
dedicated trap stack. Stated as a known limitation, not a surprise.

### 3.6 Error handling summary

| Failure | Behavior |
|---------|----------|
| Spawning more than `MAX_TASKS` | Panic at `spawn` — a static configuration bug, caught at boot. |
| A task entry function returns | Panic — task exit is deferred (§2); a returning entry is a 2c bug. Entries must loop forever. |
| Re-entrant scheduler access | The `SingleHartCell` tripwire panics (2b behavior, reused). |
| Corrupt `Context` (bad field order vs asm) | Not detectable at runtime in asm; prevented by the `#[repr(C)]` + `size_of`/offset host test, mirroring `TrapFrame`. |
| `pick_next` with no ready task | Returns `current` (the caller keeps running) — cannot happen with always-ready demo tasks, but defined rather than panicking. |

## 4. Testing

Test-first, per house discipline:

- **QEMU smoke test** (`tools/test-qemu.ps1`, extended; written first,
  failing): keeps every 2a/2b pattern, and adds:
  - a **cooperative round-robin** pattern — the 3 tasks' identifying
    output in a rotation that a single thread could not produce;
  - a **preemption** pattern — evidence that a task in a tight non-yielding
    loop was descheduled by a tick and another task ran.
- **Host unit tests** (pure cores):
  - run queue: 3-task rotation, wrap-around, skip-current, single-task
    returns itself, no-ready-task returns current;
  - `Context` layout: `size_of` and field offsets pinned (asm contract),
    mirroring `TrapFrame`'s layout test.

## 5. Deliverables

1. `task.rs` and `sched.rs` in `arch/riscv64/src/` (pure run-queue +
   `Context`/`Task` types host-tested; switch asm and static scheduler
   gated), plus the `timer.rs` tick-policy hook and the `trap.rs`
   preemption integration.
2. `kmain` spawning 3 non-terminating tasks and entering the scheduler;
   the demo proving cooperative rotation and one preemption.
3. Extended QEMU smoke test + host unit tests, all green.
4. Learning note `docs/learning/0005-scheduling-and-context-switching.md`.
5. Roadmap updated: 2c marked done with date.
6. Glossary entries (task / kernel thread, context switch, context /
   callee-saved frame, run queue, round-robin, cooperative vs. preemptive
   scheduling, yield, time slice / tick-policy hook).

## 6. Open questions (for later phases)

- **Task exit and stack reclamation** (deferred from 2c): when a task can
  finish, what reclaims its stack, and how is it removed from the run
  queue without racing a switch into it?
- **Blocking and wait queues**: the first blocking primitive (likely IPC
  in Phase 3) turns `TaskState` into a real state machine and needs a
  ready-queue / wait-queue split.
- **Dedicated trap stack + per-task guard pages**: both arrive with Phase
  3 user space, when privilege transitions make "run the trap on the
  current stack" untenable and `sscratch` stack-swapping is required.
- **A real scheduling policy**: priorities, time-slice accounting, and
  fairness, once there are tasks that differ in importance.
