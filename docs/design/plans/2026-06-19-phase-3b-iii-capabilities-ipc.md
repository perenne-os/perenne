# Phase 3b-iii: Capabilities + synchronous IPC + blocking — Implementation Plan

> **For agentic workers:** Implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let two isolated U-mode components communicate only through capability-checked synchronous IPC: a per-task capability table names an endpoint, `send`/`recv` rendezvous (blocking until the peer arrives) transfer a register-only message, and a task without the capability is rejected.

**Architecture:** A capability is an index into the calling task's own `caps` table (`Capability::Endpoint(id)`); the kernel looks it up (`cap_lookup`) — unforgeable, since a task holds only indices it was granted. An endpoint is a symbolic id; its wait queue is the set of tasks `Blocked` on it (scanned). `send`/`recv` either deliver-and-wake a waiting peer or `block_current()` (mirrors `yield_now` + the satp swap). The message rides registers only — no memory copy — so `switch_context` and the trap asm stay unchanged.

**Tech Stack:** Rust `no_std`, RISC-V (riscv64gc), QEMU virt + OpenSBI, PowerShell smoke test. Host tests: `cargo test -p kernel-arch-riscv64`. Bare build: `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf`; kernel binary: `cargo build -p kernel --target riscv64gc-unknown-none-elf`.

**Spec:** `docs/design/specs/2026-06-19-phase-3b-iii-capabilities-ipc-design.md`

**Reference — final demo:** Run queue = `server`, `client`, `rogue`, `idle` (spawn order; `server` runs first). `server` `recv`s and blocks; `client` `send`s `badge=0x42`; the kernel delivers and wakes the server, which exits with the value (`0x42` = 66). `rogue` lacks the endpoint capability, so its `send` is rejected (returns `usize::MAX`) and it exits 7. Each U-mode task runs in its own address space (3b-ii `build_user_space`); only `server`/`client` are granted the endpoint capability.

---

## Task 1: `cap.rs` — capabilities (pure, host-tested)

**Files:**
- Create: `arch/riscv64/src/cap.rs`
- Modify: `arch/riscv64/src/lib.rs` (declare the module)

- [ ] **Step 1: Create `cap.rs` with the failing test**

Create `arch/riscv64/src/cap.rs`:

```rust
//! Capabilities: unforgeable per-task authority tokens.
//!
//! A capability is an *index* into the calling task's own capability table
//! (its CSpace, on `task::Task`). A U-mode task holds only indices it was
//! granted; it cannot fabricate a `Capability` or name a kernel object it
//! was never given — that is what makes the token unforgeable. The "check"
//! a syscall performs is simply [`cap_lookup`] returning `Some`.
//!
//! Pure here (host-tested). The tables live on tasks; the IPC rendezvous
//! that consumes capabilities lives in `sched`.

/// Identifies a synchronous IPC endpoint (a rendezvous point). Symbolic:
/// there is no separate kernel object in 3b-iii — an endpoint's "wait
/// queue" is the set of tasks blocked on this id.
pub type EndpointId = usize;

/// Authority over one kernel object. One type today; more (memory, IRQ,
/// task control) arrive in later phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// Authority to `send` to / `recv` from the endpoint with this id.
    Endpoint(EndpointId),
}

/// Look up capability `idx` in `caps`; if it is an endpoint capability,
/// return its id. `None` if the index is out of range, the slot is empty,
/// or (in future) the capability is the wrong type — i.e. the check failed.
pub fn cap_lookup(caps: &[Option<Capability>], idx: usize) -> Option<EndpointId> {
    match caps.get(idx) {
        Some(Some(Capability::Endpoint(id))) => Some(*id),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_up_a_granted_endpoint() {
        let caps = [None, Some(Capability::Endpoint(7)), None];
        assert_eq!(cap_lookup(&caps, 1), Some(7));
    }

    #[test]
    fn rejects_an_empty_slot() {
        let caps: [Option<Capability>; 3] = [None, None, None];
        assert_eq!(cap_lookup(&caps, 1), None);
    }

    #[test]
    fn rejects_an_out_of_range_index() {
        let caps = [Some(Capability::Endpoint(0))];
        assert_eq!(cap_lookup(&caps, 5), None);
    }
}
```

- [ ] **Step 2: Declare the module in `lib.rs`**

In `arch/riscv64/src/lib.rs`, add after the `syscall` module declaration (around line 37):

```rust
/// Capabilities: unforgeable per-task authority tokens (pure types and the
/// lookup, host-tested). The tables live on tasks; the IPC rendezvous that
/// consumes them lives in `sched`.
pub mod cap;
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p kernel-arch-riscv64 cap`
Expected: PASS (3 new tests). Then `cargo test -p kernel-arch-riscv64` → all green.

- [ ] **Step 4: Commit**

```bash
git add arch/riscv64/src/cap.rs arch/riscv64/src/lib.rs
git commit -m "feat(cap): Capability type + cap_lookup (pure, host-tested)"
```

---

## Task 2: task IPC types + scheduler constructors

**Files:**
- Modify: `arch/riscv64/src/task.rs`
- Modify: `arch/riscv64/src/sched.rs` (+ its `tests` module)

This task adds the new `Task` fields and IPC types and updates every `Task`
constructor (so the crate keeps building), but NOT the IPC rendezvous logic
(Task 3). The old 3b-ii demo keeps running after this task.

- [ ] **Step 1: Add IPC types and `Task` fields in `task.rs`**

In `arch/riscv64/src/task.rs`, add an import at the top of the file (after the module doc, near other `use`s — there may be none yet; add it):

```rust
use crate::cap::Capability;
```

Add these types (place them above the `Task` struct):

```rust
/// Number of capability slots in each task's table (its CSpace).
pub const CAP_SLOTS: usize = 4;

/// A register-only IPC message: a badge plus three data words, transferred
/// sender→receiver with no memory access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Message {
    pub badge: usize,
    pub data: [usize; 3],
}

impl Message {
    /// The all-zero message (a task's default in-flight slot).
    pub const EMPTY: Self = Self { badge: 0, data: [0; 3] };
}

/// Which side of an IPC rendezvous a blocked task is waiting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcRole {
    Send,
    Recv,
}
```

Extend `TaskState` with the `Blocked` variant (keep the derives):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    /// Runnable, not currently on the CPU.
    Ready,
    /// Currently on the CPU (exactly one task at a time, single hart).
    Running,
    /// Terminated; skipped by `pick_next`, stacks not reclaimed.
    Exited,
    /// Waiting at an IPC rendezvous on `endpoint` as a sender or receiver.
    /// Non-`Ready`, so `pick_next` skips it until a peer wakes it.
    Blocked { endpoint: crate::cap::EndpointId, role: IpcRole },
}
```

Add the two fields to `Task`:

```rust
pub struct Task {
    pub context: Context,
    pub state: TaskState,
    /// Top of the task's static stack (informational; `context.sp` is
    /// what actually drives execution).
    pub stack_top: usize,
    pub name: &'static str,
    /// The `satp` (address space) to load when this task runs. Kernel tasks
    /// carry the master kernel `satp`; U-mode tasks carry their private one.
    pub satp: usize,
    /// This task's capability table (CSpace). A syscall names a kernel
    /// object by an index into this table — the only authority it has.
    pub caps: [Option<Capability>; CAP_SLOTS],
    /// In-flight IPC message: a task's outbox while it blocks sending, or
    /// its inbox when a sender delivers to it.
    pub message: Message,
}
```

- [ ] **Step 2: Update the `sched` test helper and add the failing `Blocked` test**

In `arch/riscv64/src/sched.rs`, the gated `Context` import line is:

```rust
#[cfg(any(target_arch = "riscv64", test))]
use crate::task::Context;
```

Add the IPC type imports right after it (available in `test` and `riscv64`,
matching that pattern — the test helper and the gated code both need them):

```rust
#[cfg(any(target_arch = "riscv64", test))]
use crate::task::{Message, IpcRole, CAP_SLOTS};
```

Update the test helper to set the new fields:

```rust
    fn task(name: &'static str, state: TaskState) -> Task {
        Task {
            context: Context::zeroed(),
            state,
            stack_top: 0,
            name,
            satp: 0,
            caps: [None; CAP_SLOTS],
            message: Message::EMPTY,
        }
    }
```

Add a test (in the `tests` module) that `pick_next` skips a `Blocked` slot:

```rust
    #[test]
    fn pick_next_skips_blocked_slots() {
        // current = 0 running; slot 1 Blocked on recv, slot 2 Ready -> pick 2.
        let mut s = three_tasks(0);
        s.tasks[1].as_mut().unwrap().state =
            TaskState::Blocked { endpoint: 0, role: IpcRole::Recv };
        assert_eq!(s.pick_next(), 2);
    }
```

- [ ] **Step 3: Set the new fields in `spawn` and `spawn_user`, and return the slot**

In `spawn`, change the signature to return `usize` and populate the fields:

```rust
pub fn spawn(name: &'static str, entry: extern "C" fn() -> !, stack_top: usize) -> usize {
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
            satp: crate::mem::kernel_satp(),
            caps: [None; CAP_SLOTS],
            message: Message::EMPTY,
        });
        slot
    })
}
```

In `spawn_user`, change the signature to return `usize` and populate the
fields (keep the `satp` parameter from 3b-ii):

```rust
pub fn spawn_user(
    name: &'static str,
    entry: extern "C" fn() -> !,
    user_sp: usize,
    kstack_top: usize,
    satp: usize,
) -> usize {
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
            satp,
            caps: [None; CAP_SLOTS],
            message: Message::EMPTY,
        });
        slot
    })
}
```

- [ ] **Step 4: Add `grant_cap`**

Add to `sched.rs` (gated), near `spawn_user`:

```rust
/// Install `cap` at `cap_slot` in the capability table of the task in
/// scheduler `slot`. Called at boot to hand a task its initial authority.
#[cfg(target_arch = "riscv64")]
pub fn grant_cap(slot: usize, cap_slot: usize, cap: crate::cap::Capability) {
    SCHED.with(|s| {
        s.tasks[slot]
            .as_mut()
            .expect("grant_cap: empty task slot")
            .caps[cap_slot] = Some(cap);
    });
}
```

- [ ] **Step 5: Build and test**

Run: `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf`
Expected: SUCCESS. (`main.rs` calls `spawn`/`spawn_user` and ignores the new `usize` return value — that still compiles, so the kernel binary also builds and the old 3b-ii demo still runs.)

Run: `cargo test -p kernel-arch-riscv64`
Expected: all green (incl. `pick_next_skips_blocked_slots`).

- [ ] **Step 6: Commit**

```bash
git add arch/riscv64/src/task.rs arch/riscv64/src/sched.rs
git commit -m "feat(task,sched): IPC task types (Message/IpcRole/Blocked/caps), grant_cap, spawn returns slot"
```

---

## Task 3: the IPC rendezvous — `ipc_send`, `ipc_recv`, `block_current`

**Files:**
- Modify: `arch/riscv64/src/sched.rs`

All additions are gated (`target_arch = "riscv64"`); they're `pub` so the
unused-until-Task-4 state produces no warnings.

- [ ] **Step 1: Add the cap import and the small step enums**

In `arch/riscv64/src/sched.rs`, add a gated import (near the other gated
`use`s, e.g. after the `ExitReason` import):

```rust
#[cfg(target_arch = "riscv64")]
use crate::cap::{cap_lookup, EndpointId};
```

Add the gated helper types and the trap-frame writer (place them just above
where you'll add the IPC functions):

```rust
/// The result of inspecting the rendezvous for a `recv`.
#[cfg(target_arch = "riscv64")]
enum RecvStep {
    BadCap,
    Got(Message),
    Block(EndpointId),
}

/// The result of inspecting the rendezvous for a `send`.
#[cfg(target_arch = "riscv64")]
enum SendStep {
    BadCap,
    Delivered,
    Block(EndpointId),
}

/// Write a received [`Message`] into the ABI return registers of `frame`:
/// a0 = badge, a1..a3 = data (`regs[9]` = a0, `regs[10]` = a1, …).
#[cfg(target_arch = "riscv64")]
fn write_message(frame: &mut crate::trap::TrapFrame, msg: Message) {
    frame.regs[9] = msg.badge;
    frame.regs[10] = msg.data[0];
    frame.regs[11] = msg.data[1];
    frame.regs[12] = msg.data[2];
}

/// First task (in slot order) blocked on `ep` as `role`, if any. The
/// endpoint's wait queue is implicit: the set of `Blocked` tasks.
#[cfg(target_arch = "riscv64")]
fn find_blocked(s: &Scheduler, ep: EndpointId, role: IpcRole) -> Option<usize> {
    let want = TaskState::Blocked { endpoint: ep, role };
    s.tasks
        .iter()
        .position(|slot| slot.as_ref().is_some_and(|t| t.state == want))
}
```

- [ ] **Step 2: Add `block_current`**

```rust
/// Block the current task at an IPC rendezvous and switch to the next
/// runnable task. The caller has NOT yet changed the task's state; this
/// marks it `Blocked { endpoint, role }`, switches away, and returns only
/// when a peer wakes it (sets it `Ready`). Called from the trap handler
/// with interrupts off (like `exit_current`). The always-`Ready` idle task
/// guarantees a successor exists.
#[cfg(target_arch = "riscv64")]
fn block_current(endpoint: EndpointId, role: IpcRole) {
    let switch = SCHED.with(|s| {
        let current = s.current;
        s.tasks[current].as_mut().unwrap().state =
            TaskState::Blocked { endpoint, role };
        let next = s.pick_next();
        assert_ne!(next, current, "block_current: no runnable successor (idle must be Ready)");
        s.tasks[next].as_mut().unwrap().state = TaskState::Running;
        s.current = next;
        let old = core::ptr::addr_of_mut!(s.tasks[current].as_mut().unwrap().context);
        let new = core::ptr::addr_of!(s.tasks[next].as_ref().unwrap().context);
        let next_satp = s.tasks[next].as_ref().unwrap().satp;
        (old, new, next_satp)
    });
    // SAFETY: as in yield_now/exit_current — distinct 'static contexts, the
    // kernel is mapped in both address spaces, single hart, interrupts off.
    // Execution resumes here when a peer wakes this task.
    unsafe {
        crate::csr::satp_write(switch.2);
        switch_context(switch.0, switch.1);
    }
}
```

- [ ] **Step 3: Add `ipc_recv`**

```rust
/// Service a `recv` syscall. `a0` (= `frame.regs[9]`) is the capability
/// index of the endpoint. If a sender is already waiting, take its message
/// and wake it; otherwise block until one arrives. On return the message
/// is in the ABI registers (or `a0 = usize::MAX` if the capability check
/// failed).
#[cfg(target_arch = "riscv64")]
pub fn ipc_recv(frame: &mut crate::trap::TrapFrame) {
    let cap_idx = frame.regs[9];
    let step = SCHED.with(|s| {
        let cur = s.current;
        let ep = match cap_lookup(&s.tasks[cur].as_ref().unwrap().caps, cap_idx) {
            Some(ep) => ep,
            None => return RecvStep::BadCap,
        };
        match find_blocked(s, ep, IpcRole::Send) {
            Some(si) => {
                let msg = s.tasks[si].as_ref().unwrap().message;
                s.tasks[si].as_mut().unwrap().state = TaskState::Ready;
                RecvStep::Got(msg)
            }
            None => {
                crate::println!("ipc: '{}' blocks on recv", s.tasks[cur].as_ref().unwrap().name);
                RecvStep::Block(ep)
            }
        }
    });
    match step {
        RecvStep::BadCap => frame.regs[9] = usize::MAX,
        RecvStep::Got(msg) => write_message(frame, msg),
        RecvStep::Block(ep) => {
            block_current(ep, IpcRole::Recv);
            // Woken: a sender stored our inbox and set us Ready.
            let msg = SCHED.with(|s| s.tasks[s.current].as_ref().unwrap().message);
            write_message(frame, msg);
        }
    }
}
```

- [ ] **Step 4: Add `ipc_send`**

```rust
/// Service a `send` syscall. `a0` = capability index, `a1` = badge,
/// `a2..a4` = data. If a receiver is waiting, deliver to it and wake it;
/// otherwise block until one arrives. `a0` becomes `0` on success or
/// `usize::MAX` if the capability check failed.
#[cfg(target_arch = "riscv64")]
pub fn ipc_send(frame: &mut crate::trap::TrapFrame) {
    let cap_idx = frame.regs[9];
    let msg = Message {
        badge: frame.regs[10],
        data: [frame.regs[11], frame.regs[12], frame.regs[13]],
    };
    let step = SCHED.with(|s| {
        let cur = s.current;
        let ep = match cap_lookup(&s.tasks[cur].as_ref().unwrap().caps, cap_idx) {
            Some(ep) => ep,
            None => {
                crate::println!(
                    "ipc: '{}' send rejected (no capability)",
                    s.tasks[cur].as_ref().unwrap().name
                );
                return SendStep::BadCap;
            }
        };
        match find_blocked(s, ep, IpcRole::Recv) {
            Some(ri) => {
                crate::println!(
                    "ipc: '{}' -> '{}' badge {:#x}",
                    s.tasks[cur].as_ref().unwrap().name,
                    s.tasks[ri].as_ref().unwrap().name,
                    msg.badge
                );
                s.tasks[ri].as_mut().unwrap().message = msg;
                s.tasks[ri].as_mut().unwrap().state = TaskState::Ready;
                SendStep::Delivered
            }
            None => {
                s.tasks[cur].as_mut().unwrap().message = msg;
                SendStep::Block(ep)
            }
        }
    });
    match step {
        SendStep::BadCap => frame.regs[9] = usize::MAX,
        SendStep::Delivered => frame.regs[9] = 0,
        SendStep::Block(ep) => {
            block_current(ep, IpcRole::Send);
            frame.regs[9] = 0; // a receiver took our message and woke us
        }
    }
}
```

- [ ] **Step 5: Build and test**

Run: `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf`
Expected: SUCCESS (no warnings — the new `pub fn`s are unused until Task 4 but `pub` items aren't dead-code-warned).

Run: `cargo test -p kernel-arch-riscv64`
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add arch/riscv64/src/sched.rs
git commit -m "feat(sched): synchronous IPC rendezvous (ipc_send/ipc_recv) + block_current"
```

---

## Task 4: `send`/`recv` syscalls (decode + dispatch routing)

**Files:**
- Modify: `arch/riscv64/src/syscall.rs` (+ its `tests` module)

- [ ] **Step 1: Write the failing decode tests**

Add to the `tests` module in `arch/riscv64/src/syscall.rs`:

```rust
    #[test]
    fn decodes_send_and_recv() {
        assert_eq!(decode_syscall(4), Syscall::Send);
        assert_eq!(decode_syscall(5), Syscall::Recv);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p kernel-arch-riscv64 decodes_send_and_recv`
Expected: FAIL — no variants `Send`/`Recv`.

- [ ] **Step 3: Add the variants, decode arms, and dispatch routing**

Extend the `Syscall` enum (add between `Yield` and `Unknown`):

```rust
    /// `yield()` — give up the CPU to the next ready task.
    Yield,
    /// `send(cap, badge, data..)` — synchronous IPC send.
    Send,
    /// `recv(cap)` — synchronous IPC receive.
    Recv,
    /// An unrecognized syscall number (a user bug, not a kernel bug).
    Unknown(usize),
```

Extend `decode_syscall`:

```rust
    match a7 {
        1 => Syscall::Print,
        2 => Syscall::Exit,
        3 => Syscall::Yield,
        4 => Syscall::Send,
        5 => Syscall::Recv,
        n => Syscall::Unknown(n),
    }
```

Extend the `match` in `dispatch` (gated) — add the two arms before
`Syscall::Unknown`:

```rust
        Syscall::Yield => Outcome::Yield,
        Syscall::Send => {
            crate::sched::ipc_send(frame);
            Outcome::Resume
        }
        Syscall::Recv => {
            crate::sched::ipc_recv(frame);
            Outcome::Resume
        }
        Syscall::Unknown(_) => {
            frame.regs[9] = usize::MAX; // -1: unknown syscall
            Outcome::Resume
        }
```

- [ ] **Step 4: Build and test**

Run: `cargo test -p kernel-arch-riscv64 decodes_send_and_recv`
Expected: PASS. Then `cargo test -p kernel-arch-riscv64` → all green, and
`cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf` → SUCCESS.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/syscall.rs
git commit -m "feat(syscall): send (a7=4) and recv (a7=5) routed to the IPC rendezvous"
```

---

## Task 5: update the smoke test to the 3b-iii milestones (red)

**Files:**
- Modify: `tools/test-qemu.ps1`

- [ ] **Step 1: Replace the milestone patterns**

In `tools/test-qemu.ps1`, replace the `$mustMatch = @( ... )` array with:

```powershell
$mustMatch = @(
    "hello world",
    "trap: breakpoint",
    "survived breakpoint",
    "paging: sv39 on",
    "wx: rodata write blocked",
    "frames: alloc/free ok",
    "ipc: 'server' blocks on recv",
    "sched: task 'server' exited \(code 66\)",
    "sched: task 'rogue' exited \(code 7\)",
    "sched: task 'client' exited \(code 0\)",
    "tick: 2(?!\d)"
)
```

Update the header comment block and the two result messages to describe the
3b-iii milestone: "the Phase 3b-iii milestone — two isolated U-mode
components communicate only through capability-checked synchronous IPC (the
server blocks on recv, the client sends a value that the server receives
across address spaces and exits with), and a rogue task lacking the
endpoint capability is rejected."

- [ ] **Step 2: Run the smoke test — expect RED**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: FAIL — the kernel still runs the 3b-ii demo (ping/pong/snoop), so
the new `ipc:`/`server`/`rogue` lines are missing.

- [ ] **Step 3: Commit (red test pinned)**

```bash
git add tools/test-qemu.ps1
git commit -m "test: smoke test asserts 3b-iii capability-checked IPC (red)"
```

---

## Task 6: kmain — server/client/rogue/idle IPC demo (green)

**Files:**
- Modify: `kernel/src/main.rs`

Context: `main.rs` currently builds per-task address spaces for ping/pong/
hog/snoop with page-aligned `UStack`/`UData`/`Snoop` statics and `sys_print`/
`sys_exit`/`sys_yield` stubs (3b-ii). You will replace the demo with four
tasks that exercise IPC, add `sys_send`/`sys_recv`, and grant capabilities.

- [ ] **Step 1: Update imports and the greeting**

Change the greeting line:

```rust
        println!("{GREETING} from {PROJECT_NAME} - Phase 3b-iii (hart {hartid})");
```

Update the `use` line to add `cap::Capability` (keep the rest):

```rust
    use kernel_arch_riscv64::{cap::Capability, mem, println, sched, timer, trap};
```

- [ ] **Step 2: Replace the spawn block in `kmain`**

Replace the entire 3b-ii spawn block (from the `// Phase 3b-ii:` comment
through `sched::enter()`) with:

```rust
        // Phase 3b-iii: two isolated U-mode components communicate ONLY
        // through a capability-checked synchronous endpoint. Each task runs
        // in its own address space (3b-ii). server/client are granted the
        // endpoint capability at boot; rogue is not. Spawn order puts server
        // in slot 0, so enter() runs it first — it recv's and blocks until
        // the client sends. These tasks don't print, so they have no user
        // data page (the (0, 0) data region maps nothing).
        use core::mem::size_of;
        let ustack = |base: usize| (base, base + size_of::<UStack>());
        const NO_DATA: (usize, usize) = (0, 0);

        let su = ustack(core::ptr::addr_of!(US_SERVER) as usize);
        let server = sched::spawn_user("server", server_task, su.1,
            core::ptr::addr_of!(KS_SERVER) as usize + TASK_STACK,
            mem::build_user_space(su, NO_DATA));
        sched::grant_cap(server, EP_CAP, Capability::Endpoint(EP0));

        let cu = ustack(core::ptr::addr_of!(US_CLIENT) as usize);
        let client = sched::spawn_user("client", client_task, cu.1,
            core::ptr::addr_of!(KS_CLIENT) as usize + TASK_STACK,
            mem::build_user_space(cu, NO_DATA));
        sched::grant_cap(client, EP_CAP, Capability::Endpoint(EP0));

        // rogue gets NO endpoint capability — its send must be rejected.
        let ru = ustack(core::ptr::addr_of!(US_ROGUE) as usize);
        let _rogue = sched::spawn_user("rogue", rogue_task, ru.1,
            core::ptr::addr_of!(KS_ROGUE) as usize + TASK_STACK,
            mem::build_user_space(ru, NO_DATA));

        sched::spawn("idle", idle, core::ptr::addr_of!(KS_IDLE) as usize + TASK_STACK);

        timer::start();
        println!("(scheduler starting; heartbeat ~1/s; exit QEMU with Ctrl-A then X)");
        sched::enter()
```

- [ ] **Step 3: Replace the static stacks (drop the 3b-ii data pages)**

Replace the 3b-ii statics block (the `KStack`/`KS_*`, `UStack`/`US_*`,
`UData`/`page_with`/`UD_*`, `*_LEN`, and `Snoop`/`SNOOP_TARGET` definitions)
with:

```rust
    /// The demo endpoint id and the capability-table slot it is installed in.
    const EP0: usize = 0;
    const EP_CAP: usize = 0;

    /// Per-task kernel stack size (also the trap stack a U-mode task's
    /// traps land on). 16 KiB; per-task guard pages stay deferred.
    const TASK_STACK: usize = 16 * 1024;
    type KStack = [u8; TASK_STACK];
    static mut KS_SERVER: KStack = [0; TASK_STACK];
    static mut KS_CLIENT: KStack = [0; TASK_STACK];
    static mut KS_ROGUE: KStack = [0; TASK_STACK];
    static mut KS_IDLE: KStack = [0; TASK_STACK];

    /// A page-aligned U-mode stack (2 pages), so each task's stack occupies
    /// its own pages — the unit of isolation (3b-ii). These tasks pass the
    /// whole IPC message in registers, so they need no user data page.
    const USER_STACK_SIZE: usize = 8 * 1024;
    #[repr(C, align(4096))]
    struct UStack([u8; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_SERVER: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_CLIENT: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_ROGUE: UStack = UStack([0; USER_STACK_SIZE]);
```

- [ ] **Step 4: Keep `sys_exit`; add `sys_send`/`sys_recv`; drop `sys_print`/`sys_yield`**

The IPC demo doesn't print or yield from user space. Keep `sys_exit`
unchanged. Remove `sys_print` and `sys_yield`. Add:

```rust
    /// send syscall (a7 = 4): a0 = cap index, a1 = badge, a2..a4 = data
    /// (zero here). Returns a0: 0 on success, or `usize::MAX` if the caller
    /// lacks the capability.
    ///
    /// # Safety
    /// Always sound; the kernel validates the capability and may block this
    /// task until a receiver arrives.
    #[inline(always)]
    unsafe fn sys_send(cap: usize, badge: usize) -> usize {
        let ret;
        core::arch::asm!(
            "ecall",
            in("a7") 4usize,
            inout("a0") cap => ret,
            in("a1") badge,
            in("a2") 0usize,
            in("a3") 0usize,
            in("a4") 0usize,
            options(nostack),
        );
        ret
    }

    /// recv syscall (a7 = 5): a0 = cap index. Returns the badge in a0 (the
    /// data words come back in a1..a3, unused here). Blocks until a sender
    /// arrives.
    ///
    /// # Safety
    /// Always sound; the kernel validates the capability and may block this
    /// task until a sender arrives.
    #[inline(always)]
    unsafe fn sys_recv(cap: usize) -> usize {
        let badge;
        core::arch::asm!(
            "ecall",
            in("a7") 5usize,
            inout("a0") cap => badge,
            out("a1") _,
            out("a2") _,
            out("a3") _,
            options(nostack),
        );
        badge
    }
```

- [ ] **Step 5: Replace the U-mode task functions (keep `idle`)**

Remove `user_ping`/`user_pong`/`user_hog`/`user_snoop`. Add:

```rust
    /// The server component: receive one message on the endpoint (blocking
    /// until the client sends), then exit with the received badge as its
    /// code — proving the value arrived across the address-space boundary.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn server_task() -> ! {
        // SAFETY: recv blocks until a sender arrives; we hold the endpoint
        // capability at EP_CAP. The badge is the value the client sent.
        unsafe {
            let badge = sys_recv(EP_CAP);
            sys_exit(badge)
        }
    }

    /// The client component: send one message (badge 0x42) to the endpoint,
    /// then exit cleanly. The server is waiting, so the send delivers and
    /// wakes it without blocking.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn client_task() -> ! {
        // SAFETY: we hold the endpoint capability at EP_CAP.
        unsafe {
            sys_send(EP_CAP, 0x42);
            sys_exit(0)
        }
    }

    /// The rogue: it was granted NO endpoint capability, so its send is
    /// rejected (returns usize::MAX). It exits 7 to prove the capability
    /// check enforced — it could not reach the endpoint server/client share.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn rogue_task() -> ! {
        // SAFETY: send is always sound; here it returns an error because we
        // hold no capability at EP_CAP.
        unsafe {
            let r = sys_send(EP_CAP, 0xdead);
            sys_exit(if r == usize::MAX { 7 } else { 0 })
        }
    }
```

Leave `idle` and `park`/`#[panic_handler]` unchanged.

- [ ] **Step 6: Build the kernel binary**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: SUCCESS. If `static_mut_refs` appears, an `&` slipped in where
`core::ptr::addr_of!` is needed.

- [ ] **Step 7: Run the smoke test — expect GREEN**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS` — all 11 patterns within 30 s: the server blocks
on recv, the client's `0x42` is delivered across address spaces and the
server exits `code 66`, the rogue is rejected and exits `code 7`, the client
exits `code 0`. If it FAILS, read the dumped serial output and diagnose (do
NOT weaken the patterns). Likely culprits: spawn order (server must be slot
0), a wrong cap slot/index, or a register-clobber mismatch in the stubs.

- [ ] **Step 8: Commit**

```bash
git add kernel/src/main.rs
git commit -m "feat: Phase 3b-iii live - capability-checked synchronous IPC between isolated components"
```

---

## Task 7: docs — short learning note, roadmap, glossary; final verification

**Files:**
- Create: `docs/learning/0009-capabilities-and-ipc.md`
- Modify: `docs/roadmap/roadmap.md`
- Modify: `docs/glossary.md`
- Modify: `docs/learning/README.md`

- [ ] **Step 1: Write a short learning note**

Create `docs/learning/0009-capabilities-and-ipc.md` (brief summary, per
project preference):

```markdown
# 0009 — Capabilities and synchronous IPC (Phase 3b-iii)

**One-line:** two isolated components now talk only through a
capability-checked synchronous endpoint — the finale of Phase 3b.

## What changed
- Each task has a small capability table (`Task.caps`). A capability is an
  *index* into your own table (`Capability::Endpoint(id)`); the kernel
  looks it up (`cap_lookup`). Unforgeable: you can only name objects you
  were granted, and can't fabricate a reference.
- An endpoint is just an id; its wait queue is the set of tasks `Blocked`
  on it (scanned). `send`/`recv` rendezvous: deliver-and-wake a waiting
  peer, or `block_current()` until one arrives.
- `TaskState::Blocked` joins Ready/Running/Exited; `block_current` mirrors
  `yield_now` (+ the satp swap). The message is register-only (badge + 3
  words) — no memory copy, so `switch_context`/trap asm stay unchanged.

## The key idea
Blocking inside a syscall = park the task (switch away) and, when the peer
delivers, write the message into the parked task's saved trap frame and let
the normal trap return (`sret`) hand it back in registers. The idle task is
always Ready, so the system never fully deadlocks.

## Proof (smoke test)
server recv's and blocks; client sends 0x42; the kernel delivers it across
address spaces; the server exits with code 66 (= 0x42). A rogue without the
endpoint capability is rejected and exits 7. Phase 3b (security spine) is
complete; next is 3c (a post-quantum-crypto primitive).
```

- [ ] **Step 2: Update the roadmap**

In `docs/roadmap/roadmap.md`, replace the `#### Phase 3b-iii — Capabilities
+ synchronous IPC + blocking` block with:

```markdown
#### Phase 3b-iii — Capabilities + synchronous IPC + blocking  *(done — 2026-06-19)*

- **Goal:** unforgeable capability tokens, capability-checked syscalls, a
  synchronous send/recv endpoint, and blocking/wait-queue task states.
- **You learn:** capabilities as unforgeable table indices, the synchronous
  rendezvous, and blocking inside a syscall (see
  [learning note 0009](../learning/0009-capabilities-and-ipc.md)).
- **Done when:** `./tools/test-qemu.ps1` observes two isolated U-mode
  components communicating only through a capability-checked endpoint (the
  server blocks on recv; the client's value crosses address spaces and the
  server exits with it) and a rogue without the capability rejected — all in
  one boot.
```

- [ ] **Step 3: Add glossary entries for the genuinely new terms**

In `docs/glossary.md`, add entries (in the file's format) for: **capability**
(an unforgeable token of authority — here an index into a task's own
capability table; you can only name objects you were granted), **endpoint**
(a synchronous IPC rendezvous point named by a capability), **IPC
(inter-process communication)** (controlled message passing between isolated
components through the kernel), and **synchronous / rendezvous** (sender and
receiver meet — whichever arrives first blocks until the other does, then
the message transfers). Reuse existing scheduling/blocking terms; don't
duplicate.

- [ ] **Step 4: Update the learning-notes index**

In `docs/learning/README.md`, add under the notes list:

```markdown
- [0009 — Capabilities and synchronous IPC (Phase 3b-iii)](0009-capabilities-and-ipc.md)
```

- [ ] **Step 5: Final verification (run all, paste results)**

Run: `cargo test -p kernel-arch-riscv64` → expect all green.
Run: `cargo build --workspace` → expect success.
Run (PowerShell): `./tools/check-references.ps1` → expect PASS (your new note + roadmap links resolve; fix any of YOUR broken references).
Run (PowerShell): `./tools/test-qemu.ps1` → expect `BOOT TEST PASS`.

- [ ] **Step 6: Commit**

```bash
git add docs/learning/0009-capabilities-and-ipc.md docs/roadmap/roadmap.md docs/glossary.md docs/learning/README.md
git commit -m "docs: Phase 3b-iii learning note, roadmap (3b complete), glossary terms"
```

---

## Done-when checklist (maps to spec §1)

- [ ] Capability-checked cross-AS IPC with blocking — `ipc: 'server' blocks on recv` + `sched: task 'server' exited (code 66)` (0x42 delivered from client's AS to server's via the endpoint cap, after a real block/wake).
- [ ] Capability enforcement — `sched: task 'rogue' exited (code 7)` (send without the cap rejected).
- [ ] Clean completion — `sched: task 'client' exited (code 0)`; all tasks ran in their own address spaces.
- [ ] `cap_lookup` + `decode_syscall(4/5)` host-tested; `pick_next` skips `Blocked`; all host tests green; `check-references` clean; `BOOT TEST PASS`.
```
