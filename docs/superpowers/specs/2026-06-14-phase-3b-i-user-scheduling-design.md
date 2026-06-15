# Kernel — Phase 3b-i Design: U-mode tasks in the run queue

- **Date:** 2026-06-14
- **Status:** Draft — awaiting review
- **Scope of this document:** Phase 3b-i only — folding U-mode tasks into
  the 2c scheduler so kernel and user tasks share one run queue, with
  per-task privilege carried through the context switch and trap return.

---

## 0. Where 3b-i sits

Phase 3 ("security spine") was decomposed (2026-06-14) into **3a** →
**3b** → **3c**. Phase 3b ("capabilities & IPC") is itself large — it
bundles multi-task U-mode scheduling, per-address-space isolation,
capabilities, IPC, and blocking — so it is further decomposed, mirroring
the 2a/2b/2c and 3a/3b/3c splits, into:

- **3b-i (this doc) — U-mode tasks in the run queue.** The scheduling
  substrate: two U-mode tasks schedule alongside each other (and a kernel
  task) in one run queue. Without it there is no "two components" to
  isolate or connect.
- **3b-ii — Per-address-space isolation.** Each component gets its own
  `satp`; the kernel is mapped into every address space; `satp` swaps on
  context switch.
- **3b-iii — Capabilities + synchronous IPC + blocking.** Unforgeable
  capability tokens, capability-checked syscalls, a synchronous send/recv
  endpoint, and the blocking/wait-queue task states.

Predecessor: the [3a user-mode spec](2026-06-14-phase-3a-user-mode-design.md),
whose §2 deferred exactly this work ("U-mode tasks in the round-robin run
queue") and whose §6 filed it as an open question. Also builds directly on
the [2c scheduling spec](2026-06-14-phase-2c-scheduling-design.md).

## 1. Goal

The kernel runs **kernel and U-mode tasks in a single round-robin run
queue**. Today (3a) a U-mode task runs *standalone*: `kmain` calls
`enter_user`, which parks `kmain`'s context, `sret`s to U-mode, and
`terminate_user` switches back so `enter_user` *returns* to `kmain`. That
is a separate machinery from the 2c scheduler. 3b-i **collapses the two
into one**: a U-mode task becomes an ordinary scheduler slot, entered and
resumed by the same `switch_context` every other task uses, and terminated
by a scheduler operation rather than a return to `kmain`.

**You learn (kept brief):** how *per-task privilege* is carried across a
context switch for free — the trap return already restores `sepc`/`sstatus`
(and therefore the privilege level) from each task's saved trapframe — and
how a "first run" of a U-mode task is forged so the very first
`switch_context` lands in a trampoline that `sret`s to U-mode (the U-mode
analogue of 2c's `task_trampoline`).

**Done when** `./tools/test-qemu.ps1` observes, in one boot, *in addition
to all Phase 2a/2b/2c milestones* (greeting, breakpoint recovery, paging
line, W^X block, frame round-trip, ≥ 2 ticks, cooperative round-robin,
preemption):

1. **Two U-mode tasks round-robin** via a new `yield` syscall and **both
   exit cleanly** (each prints, yields to the other, and calls `exit`;
   the scheduler reports each exit and keeps running).
2. **A U-mode hog is preempted** — a non-yielding U-mode task is
   interrupted by the timer and another task runs after it (the U-mode
   analogue of 2c's preemption proof).
3. **A bad U-mode task is contained while the scheduler keeps running** —
   it touches a kernel page, is killed, and *the other tasks continue*
   (strictly stronger than 3a's contain-and-return-to-`kmain`).
4. **A kernel (S-mode) task and U-mode tasks coexist** in the one run
   queue — each resumed at its correct privilege. This is the per-task
   privilege proof.

## 2. Non-goals (deferred to later sub-phases)

- **Per-address-space isolation** — 3b-i keeps the single 2b/3a kernel
  page table (`satp` unchanged); user tasks get *separate stacks* but share
  the address space. Distinct `satp` per component is **3b-ii**.
- **Capabilities, IPC, blocking** — no capability tokens, no message
  passing, no blocked/waiting task state. `TaskState` gains only `Exited`
  here (§3.3). All of that is **3b-iii**.
- **Stack reclamation / reaping** — an `Exited` task's stacks are not
  reclaimed; the slot stays `Exited` and is skipped forever (deferred since
  2c/3a).
- **Dynamic pointer validation** — `print` keeps the 3a *static* bounds
  guard. The page-table-walk version arrives with dynamic regions in
  **3b-iii**.
- **Guard pages / per-hart trap stacks / SMP** — one hart, one statically
  sized kernel stack per task; guard pages stay deferred (2c §3.5).

## 3. Design

### 3.1 Components

Following the established arch-crate pattern (pure logic ungated and
host-testable; hardware/asm gated to `target_arch = "riscv64"`):

| Component | Location | Responsibility |
|-----------|----------|----------------|
| `TaskState::Exited` | `arch/riscv64/src/task.rs` | The 3a-deferred terminal state. `pick_next` skips any non-`Ready` slot, so `Exited` slots are skipped without further change. |
| `spawn_user` | `arch/riscv64/src/sched.rs` | Forge a `Ready` U-mode slot whose first `switch_context` lands in `user_trampoline` and `sret`s to U-mode. Mirrors `spawn` (kernel tasks). |
| `user_trampoline` | `arch/riscv64/src/sched.rs` (gated asm) | Today's `enter_user_asm`, renamed; body unchanged. First-run entry for a U-mode task. |
| `exit_current` | `arch/riscv64/src/sched.rs` | Replaces `terminate_user`: mark current `Exited`, print the `ExitReason`, pick next, `switch_context` away. Never resumes the dead slot. |
| (deleted) | `arch/riscv64/src/sched.rs` | `enter_user`, `terminate_user`, `USER_RETURN`, `USER_EXIT` — the standalone `kmain` round-trip. |
| `yield` syscall | `arch/riscv64/src/syscall.rs` | `Syscall::Yield` (a7 = 3) and `Outcome::Yield`. |
| Trap dispatch | `arch/riscv64/src/trap.rs` | `Exit → exit_current(Exited)`; `Yield → advance sepc, yield_now()`; U-mode fatal fault → `exit_current(Killed(cause))`. |
| Demo | `kernel/src/main.rs` | Idle kernel task + two cooperating U-mode tasks + a hog + a bad task; per-task kernel and user stacks; `kmain` ends at `sched::enter()`. |

### 3.2 Per-task privilege is carried for free

Through 2c, `switch_context` saved/restored only the callee-saved set and
resumed via `ret`. That already suffices for **every** task — kernel or
user — because the privilege level is not part of the callee-saved context;
it lives in the **trapframe**. Concretely:

- A task only ever leaves the CPU from inside the trap handler (timer
  preemption, the `yield` syscall, `exit`, or a fault) — except a kernel
  task's voluntary `yield_now`, which is the 2c path. In the trap case the
  task's full 31-GPR state, including `sepc` and `sstatus` (which encodes
  the privilege via `SPP`), is already saved in its trapframe at the top of
  its own kernel stack.
- `switch_context` parks the *handler's* callee-saved continuation in the
  task's `Context` and loads the next task's. On resume it returns up
  through the handler frames into the **existing** trap-return assembly,
  which restores the trapframe — `sepc`, `sstatus` (privilege!), all GPRs —
  and re-arms `sscratch` from `sp + 288` (this task's kernel-stack top,
  because its trapframe sits at the top of its own kernel stack). Then
  `sret` returns at the saved privilege.

**Therefore `switch_context` and the trap entry/exit assembly are
unchanged.** The "per-task privilege through the switch" that 3a deferred
is an emergent property of (a) each task trapping onto its own kernel stack
and (b) resuming through the trap return. The only genuinely new mechanism
is the *first* entry of a never-run U-mode task (§3.4).

### 3.3 `TaskState::Exited` and `exit_current`

`TaskState` gains `Exited`. `exit_current(reason: ExitReason)` is called
from the trap handler for both a clean `exit` syscall and a fatal U-mode
fault:

```
exit_current(reason):
    print the ExitReason diagnostic line (Exited{code} / Killed{cause})
    mark the current slot Exited
    next = pick_next()            // skips Exited (non-Ready) slots
    switch_context(&current.context, &next.context)   // never resumes current
    unreachable!()                // control continues in `next`
```

It saves into the dying slot's own `Context` (harmless — the slot is
`Exited` and never picked again), so no throwaway is needed. Because the
**idle task is always `Ready`** (§3.6), `pick_next` always returns a
runnable successor; an assert guards the "no successor" impossibility.

This replaces 3a's `terminate_user`, which switched back to a parked
`kmain` context. There is no parked `kmain` context any more: `kmain` ends
at `sched::enter()` and never returns (the 2c shape).

### 3.4 Spawning and entering a U-mode task

`spawn_user(name, entry, user_sp, kstack_top)` forges a `Ready` slot the
same way `spawn` forges a kernel task, but pointed at `user_trampoline`:

```
context.ra   = user_trampoline          // first switch_context "returns" here
context.sp   = kstack_top               // the task's kernel/trap stack top
context.s[0] = entry                    // -> sepc (U-mode entry, in a U page)
context.s[1] = user_sp                  // -> the user stack top (U page)
context.s[2] = user_sstatus(read())     // SPP = 0, SPIE = 1 (task::user_sstatus)
state        = Ready
```

`user_trampoline` is 3a's `enter_user_asm`, unchanged:

```
user_trampoline:
    csrw sscratch, sp     # sscratch = this task's kernel-stack top
    csrw sepc, s0         # user entry
    csrw sstatus, s2      # SPP = 0, SPIE = 1
    mv   sp, s1           # switch to the user stack
    sret                  # -> U-mode
```

After this first entry the task is indistinguishable from any other in the
run queue: it traps onto its kernel stack and resumes through the path in
§3.2. `enter()` (2c) and the round-robin are otherwise unchanged.

### 3.5 The `yield` syscall

U-mode tasks cannot call the scheduler directly, and timer ticks (~1 Hz)
are too coarse to interleave two tasks legibly within a short boot. So a
third syscall lets a U-mode task cooperatively give up the CPU:

| # | Name | Args | Behavior |
|---|------|------|----------|
| 3 | `yield` | — | Give up the CPU to the next ready task; resume here later. |

Dispatch returns `Outcome::Yield`. The trap handler advances `sepc` past
the `ecall` **first** (so the task resumes *after* it), then calls
`sched::yield_now()` — the very same primitive 2c uses for preemption.
Inside the trap handler interrupts are already off, so `yield_now` resumes
with them off and the trap return re-enables them atomically via `sret`
(exactly the 2c preemption case). No new scheduling mechanism is needed.

The syscall surface is now: `print` (1), `exit` (2), `yield` (3).

### 3.6 The demo and the idle task

`kmain` (after the 2a/2b unchanged probes) spawns, then `sched::enter()`:

- **`idle`** — a *kernel* (S-mode) task: `loop { yield_now(); wfi }`. It
  yields immediately when other tasks are `Ready` (so it never disturbs the
  cooperative ping-pong) and `wfi`-sleeps when it is the only `Ready` task.
  It never exits, so it is always a valid `exit_current` successor (§3.3)
  and keeps the system alive after every user task has exited.
- **`user_ping` / `user_pong`** — two U-mode tasks: print a line via
  `print`, `yield` to the other, repeat a couple of times, then `exit(0)`.
  Proves done-when #1 and #4.
- **`user_hog`** — a U-mode task that prints once then loops without
  yielding; the timer preempts it and another task runs after. Proves
  done-when #2.
- **`user_bad`** — a U-mode task that loads from a kernel page; it is
  killed and the scheduler keeps running the others. Proves done-when #3.

Each U-mode task has its **own** static kernel/trap stack (kernel memory,
never mapped `U`) and its **own** user stack (in `.user_data`, mapped
`RW-U`). The single shared `TRAP_STACK` of 3a is removed. `MAX_TASKS` is
raised to cover idle + the four user tasks plus headroom.

The exact print lines and their ordering are pinned by the smoke test and
finalized in the implementation plan; the spec fixes only *what* must be
observable (the four done-when proofs).

### 3.7 Error handling summary

| Failure | Behavior |
|---------|----------|
| Unknown syscall number | `a0 = -1`, resume (unchanged). |
| `print` pointer fails validation | `a0 = -1`, nothing read/printed (unchanged static guard). |
| `exit` syscall | `exit_current(Exited{code})`; scheduler continues. |
| Fatal U-mode fault (non-`U` load/store/fetch, illegal instr.) | `exit_current(Killed{cause})`; task contained, scheduler continues. |
| U-mode entry returns instead of calling `exit` | Faults (no valid return address) → contained as above. |
| S-mode W^X probe (2b) | Skip-and-resume, unchanged; never routed to `exit_current`. |
| `exit_current` finds no successor | `unreachable` — the idle task is always `Ready`; an assert documents it. |
| Re-entrant scheduler / trap | The `SingleHartCell` tripwire (2b/2c) still applies. |

## 4. Testing

Test-first, per house discipline:

- **QEMU smoke test** (`tools/test-qemu.ps1`, extended; written first,
  failing): keeps every 2a/2b/2c pattern, and adds the four done-when
  proofs — two U-mode tasks interleaving via `yield` and both exiting, the
  hog preempted, the bad task contained with the scheduler surviving, and a
  kernel task coexisting with U-mode tasks.
- **Host unit tests** (pure cores):
  - **syscall decode:** `decode_syscall(3) == Yield`; existing 1/2/unknown
    cases stay green.
  - **`spawn_user` forge:** the forged `Context` has `ra = user_trampoline`,
    `sp = kstack_top`, `s[0]/s[1]` set; the forged `sstatus`
    (`task::user_sstatus`) has `SPP = 0`, `SPIE = 1` (existing
    `user_sstatus` tests already cover the bit math).
  - **layout / `pick_next`:** existing `Context`/`TrapFrame` layout and
    `pick_next` tests stay green; add a case that `pick_next` skips an
    `Exited` slot.

## 5. Deliverables

1. `task.rs` `TaskState::Exited`; `sched.rs` `spawn_user`,
   `user_trampoline` (renamed), `exit_current`, and deletion of
   `enter_user`/`terminate_user`/`USER_RETURN`/`USER_EXIT`; `syscall.rs`
   `Yield`; `trap.rs` dispatch updates.
2. `kmain` demo: idle + two cooperating U-mode tasks + hog + bad task,
   per-task kernel and user stacks, ending at `sched::enter()`.
3. Extended QEMU smoke test + host unit tests, all green.
4. A **short** learning note `docs/learning/0007-user-scheduling.md` —
   a summary, not a tutorial (per project preference).
5. Roadmap updated: Phase 3b decomposed into 3b-i/ii/iii; 3b-i marked done
   with date.
6. Glossary entries only for genuinely new terms (e.g. `yield` syscall,
   task `Exited` state); reuse 3a's privilege/trap-stack entries.

## 6. Open questions (for later sub-phases)

- **Per-address-space isolation (3b-ii):** when each task gets its own
  `satp`, where does the `satp` swap sit relative to `switch_context`, and
  how is the kernel (and the trap vector/stack) mapped into every address
  space so the trap path still works?
- **Capability-checked IPC (3b-iii):** the `yield`/`exit`/`print` surface
  grows a capability-checked `send`/`recv`; `TaskState` grows blocked/
  waiting states and wait queues.
- **Stack reclamation / reaping:** still deferred — what reclaims an
  `Exited` task's kernel and user stacks without racing a switch.
- **Idle policy:** a cooperative-yield idle task is a demo-grade choice;
  a real scheduler distinguishes "no runnable task" from "idle is just
  another slot."
