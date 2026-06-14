# Kernel — Phase 3a Design: Privilege Drop to User Space

- **Date:** 2026-06-14
- **Status:** Draft — awaiting review
- **Scope of this document:** Phase 3a only — the first transition to
  U-mode, a minimal syscall path, and proof that the privilege boundary
  enforces. Phase 3 ("security spine") is decomposed (2026-06-14) into
  **3a (this doc)** → **3b (capabilities + IPC)** → **3c (PQC primitive)**,
  mirroring the 2a/2b/2c decomposition. Capabilities need an unprivileged
  component to *hold* them, so privilege transition comes first.
  Predecessors: the [2a trap spec](2026-06-10-phase-2a-traps-design.md),
  the [2b memory spec](2026-06-10-phase-2b-memory-design.md), and the
  [2c scheduling spec](2026-06-14-phase-2c-scheduling-design.md), which
  explicitly deferred the dedicated trap stack and `sscratch` stack-swap
  to "Phase 3 user space."

---

## 1. Goal

The kernel drops to **U-mode** (the unprivileged level, "ring 3") for the
first time and runs a task that can touch the kernel *only* through a
syscall. A single user task runs in U-mode, makes syscalls via `ecall`,
exits cleanly — and, when it reaches for memory it does not own, the
privilege boundary stops it and the kernel **contains** the task instead
of dying.

**You learn:** the U/S privilege boundary (`sstatus.SPP`/`SPIE`, `sret`
into U-mode), the `sscratch`-based **trap-stack swap** (the hart arrives
from a trap on whatever stack the user was using — untrusted — and must
land on a trusted kernel stack before it touches anything), `ecall` from
U-mode as the syscall mechanism (`scause = 8`, "environment call from
U-mode"), the syscall ABI, the **confused-deputy** problem (the kernel
must not be tricked into dereferencing a user-supplied pointer into kernel
memory), and the PTE **`U` bit** that marks a page user-accessible.

**Done when** `./tools/test-qemu.ps1` observes, in one boot, *in addition
to all Phase 2a/2b/2c milestones* (greeting, breakpoint recovery, paging
line, W^X block, frame round-trip, ≥ 2 ticks, cooperative round-robin,
preemption):

1. a **U-mode `print` syscall round-trip** — a line printed only because
   the kernel serviced an `ecall` from U-mode, after which control returns
   to U-mode (the user task keeps running);
2. a **clean `exit`** — the user task calls the `exit` syscall and control
   returns to the kernel;
3. **boundary enforcement** — a U-mode task that loads from a kernel-only
   (non-`U`) page faults and is *contained* (terminated; the kernel
   survives and prints a containment line), which is distinct from 2b's
   recover-and-skip of an S-mode W^X probe.

## 2. Non-goals (deferred)

- **Per-address-space isolation** — 3a keeps the single 2b kernel page
  table (`satp` unchanged) and marks the user task's pages `U`. A separate
  `satp` per component (true address-space switching, "kernel mapped in
  every address space") arrives in 3b, where mutually-distrusting
  components actually need distinct address spaces.
- **IPC / message passing** — there is one user task and nothing to talk
  to. Synchronous IPC between components is the core of 3b.
- **Capabilities** — 3a has a fixed two-syscall surface available to the
  one task; unforgeable capability tokens and capability-checked syscalls
  are 3b.
- **Blocking / wait queues** — no task waits on anything yet; `TaskState`
  gains only `Exited` (§3.6), not a blocked state. Blocking arrives with
  IPC (3b).
- **Full stack reclamation / reaping** — 3a termination is a state
  transition plus "the scheduler never runs it again"; reclaiming the
  task's stack frame is still deferred (it was deferred in 2c too).
- **U-mode tasks in the round-robin run queue** — the user task is entered
  standalone via `enter_user` (§3.5), *not* spawned into the 2c run queue.
  Teaching the context-switch + trap-return path to carry per-task
  privilege (SPP) is 3b's scheduler-integration work.
- **Multiple user tasks** — exactly one, to keep the focus on the boundary
  mechanism. A single dedicated trap stack therefore suffices (§3.3).
- **`sstatus.SUM`** — left 0. The kernel does not (and must not) read user
  pages by accident; the `print` syscall copies *with explicit
  validation* (§3.4), not by flipping SUM.

## 3. Design

### 3.1 Components

New `syscall` module in the arch crate, following the `trap.rs`/`mem`
pattern: pure logic ungated (host-testable), hardware access gated to
`target_arch = "riscv64"`. The privilege-transition asm extends the
existing `trap.rs` entry; the user-context forge extends `task.rs`; the
return-to-kernel path extends `sched.rs`.

| Component | Location | Responsibility |
|-----------|----------|----------------|
| User-context forge | `arch/riscv64/src/task.rs` | Build the initial state to `sret` into U-mode: `sepc` = entry, `sstatus.SPP = 0` (return to U), `SPIE = 1` (interrupts on after `sret`), user `sp` = top of the user stack. The `sstatus` bit computation is pure and host-tested. |
| `sret` entry + return path | `arch/riscv64/src/sched.rs` | `enter_user(task) -> ExitReason`: park a kernel "return" context (the 2c bootstrap-context trick), set `sscratch`, build the user trap frame, `sret` into U-mode. Returns to the caller when the user task exits or is killed. |
| Trap-stack swap | `arch/riscv64/src/trap.rs` (gated asm) | On trap entry, `csrrw sp, sscratch, sp` to swap to the kernel trap stack **only when arriving from U-mode**; `sscratch = 0` while in the kernel is the sentinel that means "already on a kernel stack, don't swap" (preserves 2c preemption). Symmetric restore on exit. |
| Syscall dispatch | `arch/riscv64/src/syscall.rs` *(new)* + `trap.rs` | The `Cause::UserEcall` arm decodes `a7` → syscall enum, dispatches, writes the return value to `a0`, and advances `sepc` past the 4-byte `ecall`. Decode and pointer validation are pure/host-tested. |
| User memory perms | `arch/riscv64/src/mem/paging.rs` | Add a `U` flag to the PTE-flags API; remap a page-aligned `.user` text section as `R-X-U` and the user stack as `RW-U`. Everything else stays non-`U`. |
| Kernel binary | `kernel/src/main.rs` | A page-aligned user entry fn placed in `.user`, a `static` user stack; after the 2c demo, `enter_user` the task and print the `ExitReason`. |

### 3.2 The privilege boundary and `sret` into U-mode

RISC-V privilege is selected on trap return by `sstatus.SPP`: `sret` drops
to U-mode when `SPP = 0`, returns to S-mode when `SPP = 1`. To launch a
user task the kernel forges the return state:

```
sepc            = user entry point (the .user function)
sstatus.SPP     = 0          // sret returns to U-mode
sstatus.SPIE    = 1          // interrupts enabled after sret (timer still ticks)
sp (user)       = top of the user stack (16-aligned, RISC-V ABI)
a0..a7, t*, ...  = 0          // a clean register file for the user task
```

`sret` then loads `sepc` into `pc`, sets the privilege to `SPP`, and
restores the prior interrupt-enable from `SPIE`. The user task begins
executing unprivileged. The `sstatus` computation (which bits to set/clear)
is pure and host-tested, mirroring how `TrapFrame`/`Context` layouts are
pinned.

### 3.3 The trap-stack swap (`sscratch`) — the heart of 3a

Through 2c, `__trap_entry` assumed `sp` already pointed at a valid kernel
stack — true, because every trap came from S-mode (the kernel itself or an
S-mode task). A trap from U-mode breaks that assumption: the hart traps
with `sp` still pointing at the **user** stack, which the kernel must not
trust or push onto.

The classic resolution (xv6, Linux) uses `sscratch` as a privilege-aware
stack pointer:

- While a **user task runs**, `sscratch` holds the top of the kernel
  **trap stack**.
- While the **kernel runs**, `sscratch` holds `0` (a sentinel).

Trap entry begins with `csrrw sp, sscratch, sp` (atomic swap of `sp` and
`sscratch`):

- From U-mode: `sp` becomes the kernel trap stack, `sscratch` now holds
  the saved user `sp` — the handler is safe.
- From S-mode: the swap puts `0` into `sp`, which the entry detects (swap
  back) and continues on the current kernel stack exactly as in 2c — so
  **2c preemption is unchanged**.

A single `static TRAP_STACK: [u8; STACK_SIZE]` serves the one user task
(§2: one task). On exit the path restores symmetrically. This is the
concrete "dedicated trap stack" that the 2c spec (§3.5) deferred to here.

### 3.4 Syscall ABI and the confused-deputy guard

The user task reaches the kernel with `ecall`, which traps as `scause = 8`
("environment call from U-mode"). Convention:

```
a7              = syscall number
a0..a5          = arguments
a0              = return value (after the kernel advances sepc past ecall)
```

`ecall` does **not** advance `sepc`; the handler adds 4 (the `ecall`
instruction width) so the user resumes *after* it, not in an infinite trap.

Two syscalls:

| # | Name  | Args            | Behavior |
|---|-------|-----------------|----------|
| 1 | `print` | `a0`=ptr, `a1`=len | Validate `[ptr, ptr+len)` lies fully within the user task's known `U`-region, then write those bytes to the console. Return bytes written, or an error code if validation fails. |
| 2 | `exit`  | `a0`=code         | Terminate the calling task (§3.6); does not return to U-mode. |

**The confused-deputy guard is the lesson of `print`.** The kernel runs
privileged; if it blindly dereferenced a user-supplied pointer, a user
task could hand it a *kernel* address and have the kernel read (and print)
memory the task can't reach — the kernel acting as a "confused deputy." So
`print` validates the range against the user task's mapped `U`-region
bounds **before** any dereference. The validation is a pure function
(`bounds`, `ptr`, `len` → ok / reject) host-tested for: in-bounds,
overrun past the region end, integer-overflow/wrap of `ptr+len`, and a
kernel pointer (outside the region) rejected. 3a validates against the
known static user-region bounds; a full page-table-walk validation (check
each page is `U`+readable) is noted for 3b when regions become dynamic.

### 3.5 Entering U-mode and coming back — `enter_user`

`switch_context` (2c) needs an `old` context to save into; the same
problem appears here in reverse — when the user task ends, the kernel
needs somewhere to resume. Reusing 2c's bootstrap-context trick:

```
enter_user(task) -> ExitReason:
    park a kernel return-context (saved here, restored on task end)
    sscratch = TRAP_STACK top
    build the user trap frame (§3.2) on the trap stack
    sret                         // -> U-mode; does not "return" normally
    // control reaches here only via the termination path (§3.6),
    // which restores the parked context; return the ExitReason
```

On `exit`, or on a fatal U-mode fault, the handler does **not** `sret`
back to the user — it restores the parked kernel context, so `enter_user`
*returns* to `kmain` with an `ExitReason` (`Exited { code }` or
`Killed { cause }`). `kmain` prints the outcome and parks. This keeps the
"a kernel never returns from the top" property while letting the user
task be a bounded episode the kernel supervises.

### 3.6 Termination and containment

`TaskState` gains `Exited`. One `terminate(reason)` path serves both:

- the `exit` syscall (`reason = Exited { code }`), and
- a **fatal U-mode fault** (`reason = Killed { cause }`) — e.g. the
  boundary-proof load from a kernel page.

`terminate` marks the task `Exited` and restores the parked kernel context
(§3.5). No stack reclamation (deferred, §2).

**Distinguishing a U-mode fault from 2b's W^X probe.** 2b's probe is an
S-mode store to `.rodata`, recovered by skipping the instruction
(`SPP = 1`). The boundary proof is a **U-mode** load from a kernel page
(`SPP = 0`). The handler branches on the trap's originating privilege:
S-mode probe → skip-and-resume (2b behavior, unchanged); U-mode fatal
fault → `terminate`. The two paths never collide.

### 3.7 User memory and the `U` bit

User code must live on pages marked `U`; mixing it onto kernel `.text`
(non-`U`, and a W^X/least-authority smell) is wrong. So:

- User code goes in a **page-aligned `.user` linker section**, mapped
  `R-X-U` — a tiny embedded "user program" (the entry fn and anything it
  calls, kept self-contained).
- The user **stack** is a `static` array whose pages are remapped `RW-U`.
- Everything kernel stays **non-`U`**, so any U-mode access to it faults —
  which *is* the boundary proof (§3.6).

This extends 2b's paging: the PTE-flags API gains a `U` flag, and `mem`
(or `kmain` after `mem::init`) remaps the `.user` and user-stack pages
with `U` set. `sstatus.SUM` stays 0 (§2).

### 3.8 Error handling summary

| Failure | Behavior |
|---------|----------|
| Unknown syscall number in `a7` | Return an error code in `a0`; the task continues (a user bug, not a kernel bug). |
| `print` pointer fails validation | Return an error code in `a0`; nothing is read or printed (confused-deputy refused). |
| Fatal U-mode fault (load/store/instruction on a non-`U` page, or illegal instruction) | `terminate(Killed{cause})` — task contained, kernel survives, containment line printed. |
| User entry function returns instead of calling `exit` | Treated as a fault/UB at the user level; the user stack has no valid return address, so it faults and is contained. (The `.user` entry is written to call `exit`.) |
| S-mode W^X probe (2b) | Unchanged: skip-and-resume; never routed to `terminate`. |
| Re-entrant scheduler / trap | The `SingleHartCell` tripwire (2b/2c) still applies. |

## 4. Testing

Test-first, per house discipline:

- **QEMU smoke test** (`tools/test-qemu.ps1`, extended; written first,
  failing): keeps every 2a/2b/2c pattern, and adds:
  - the **U-mode `print` round-trip** line (printed via the serviced
    `ecall`, with the user task still running afterward);
  - the **clean `exit`** line (control back in the kernel with the exit
    code);
  - the **contained-fault** line (U-mode kernel-page load killed the task,
    kernel survived).
- **Host unit tests** (pure cores):
  - **syscall decode:** valid numbers map to the right syscall; an unknown
    number maps to the error/unknown variant;
  - **pointer validation:** in-bounds ok; overrun past region end rejected;
    `ptr + len` wrap rejected; a kernel pointer (outside the region)
    rejected;
  - **user-context bits:** the forged `sstatus` has `SPP = 0`, `SPIE = 1`,
    and `sepc`/`sp` set as specified;
  - **layout:** any new `#[repr(C)]` struct's `size_of`/offsets pinned, as
    `TrapFrame`/`Context` are.

## 5. Deliverables

1. `syscall.rs` (new) plus the `task.rs` user-context forge, the
   `sched.rs` `enter_user`/return path, the `trap.rs` `sscratch`
   trap-stack swap and `UserEcall`/U-mode-fault arms, and the
   `paging.rs` `U`-flag + `.user`/user-stack remap.
2. A page-aligned `.user` entry function and `static` user stack in
   `kmain`; the demo proving the print round-trip, clean exit, and
   contained fault.
3. Extended QEMU smoke test + host unit tests, all green.
4. Learning note `docs/learning/0006-user-mode-and-syscalls.md`.
5. Roadmap updated: Phase 3 decomposed into 3a/3b/3c; 3a marked done with
   date.
6. Glossary entries: privilege level (U-mode / S-mode), `sret`,
   `sstatus.SPP`/`SPIE`, `sscratch`, trap stack / kernel stack, syscall,
   `ecall`, syscall ABI, confused deputy, task termination, the `U` PTE
   bit, `SUM`.

## 6. Open questions (for later phases)

- **Per-address-space isolation (3b):** when components get their own
  `satp`, how is the kernel mapped into every address space for the trap
  path, and when does `satp` swap relative to the context switch?
- **Dynamic pointer validation (3b):** once user regions are dynamic, the
  `print`-style guard becomes a page-table walk (each touched page is
  `U`+readable) rather than a static bounds check.
- **U-mode tasks in the run queue (3b):** teaching `switch_context` and
  the trap-return path to carry per-task privilege (SPP), so user tasks
  schedule alongside kernel tasks.
- **Stack reclamation / reaping:** still deferred — what reclaims an
  `Exited` task's stack without racing a switch.
- **Multiple trap stacks:** one per hart (and ultimately per task) when
  SMP and multiple user tasks arrive.
