# Kernel — Phase 3b-iii Design: Capabilities + synchronous IPC + blocking

- **Date:** 2026-06-19
- **Status:** Draft — awaiting review
- **Scope of this document:** Phase 3b-iii only — a per-task capability
  table, a synchronous IPC endpoint with `send`/`recv`, and blocking /
  wait-queue task states, so two isolated U-mode components communicate
  *only* through capability-checked IPC. This completes Phase 3b.

---

## 0. Where 3b-iii sits

Phase 3b ("capabilities & IPC") was decomposed (2026-06-14) into **3b-i**
(U-mode tasks in the run queue — done 2026-06-15) → **3b-ii**
(per-address-space isolation — done 2026-06-19) → **3b-iii** (this doc).
3b-i gave us multiple U-mode tasks in one run queue; 3b-ii made each task's
memory private (its own `satp`). 3b-iii is the finale: the only way two
isolated components can now interact is an explicit, capability-checked
message channel through the kernel.

Predecessors: the [3b-ii spec](2026-06-19-phase-3b-ii-address-space-isolation-design.md)
(§6 filed capability-checked IPC + blocking as the next step) and the
[3b-i spec](2026-06-14-phase-3b-i-user-scheduling-design.md) (the scheduler,
`TaskState`, the syscall path, `exit_current`/`yield_now`).

## 1. Goal

Two isolated U-mode components rendezvous through a kernel **endpoint**:
one `recv`s, one `send`s, and the kernel transfers a small register message
across their address spaces — but only if the caller holds a **capability**
naming the endpoint. A task without the capability cannot send. A task that
arrives first **blocks** until its peer arrives.

**You learn (kept brief):** capabilities as unforgeable, per-task table
*indices* (a task can only name kernel objects it was granted, and can't
fabricate a reference); the synchronous **rendezvous** (sender and receiver
meet, message transfers, both proceed); and **blocking** — parking a task
inside a syscall and resuming it, with the message delivered into its saved
trap frame, when the peer arrives.

**Done when** `./tools/test-qemu.ps1` observes, in one boot, *in addition
to the 2a/2b milestones* (greeting, breakpoint recovery, paging line, W^X
block, frame round-trip, ≥ 2 ticks):

1. **Capability-checked cross-address-space IPC with blocking** — the
   `server` task `recv`s first and blocks (kernel logs it); the `client`
   task `send`s a value; the kernel delivers it and wakes the server; the
   server returns the value and exits with it as its code — proving the
   value crossed from the client's address space to the server's through
   the endpoint capability, after a real block/wake.
2. **Capability enforcement** — a `rogue` task that lacks the endpoint
   capability calls `send`, is rejected, and exits with a distinct code.
3. **Clean completion** — the `client` exits cleanly; each task ran in its
   own address space (3b-ii), sharing nothing but the endpoint.

## 2. Non-goals (deferred)

- **Capability delegation / transfer through IPC** — the seL4 hallmark
  (sending a capability *in* a message to grant authority) is **not** in
  3b-iii. Capabilities are installed at boot and never move. This is the
  single biggest deferral and a natural future phase.
- **Multiple capability/object types, fine-grained rights, revocation** —
  one type only: `Endpoint`. No per-cap rights bits, no revoke.
- **Byte-buffer / long messages, scatter-gather** — the message is a fixed
  set of machine words carried in registers (§3.4); no memory is touched,
  so no cross-AS buffer copy and no pointer validation. (The static
  `.user_data` bounds check from 3a/3b-ii stays, simply unused by IPC.)
- **Call/reply (RPC) semantics** — only one-way `send` and `recv`. A
  request/response protocol is built by the user from two messages; a
  kernel `call` (atomic send-then-block-for-reply) is deferred.
- **Multiple endpoints in the demo** — the mechanism supports several
  endpoint ids, but the demo uses one.
- **Reaping** — endpoints are symbolic ids (no object to free); exited
  tasks' stacks / page tables / cap tables are still not reclaimed
  (deferred since 2c).
- **Priorities / fairness** — the wait "queue" is the set of `Blocked`
  tasks, matched by scanning the task array in slot order (good enough for
  `MAX_TASKS` = 6); no priority or strict FIFO guarantee is promised.
- **ASIDs** — full TLB flush per switch, unchanged from 3b-ii.

## 3. Design

### 3.1 Components

Arch-crate pattern: pure logic ungated and host-testable; SCHED/CSR access
gated to `target_arch = "riscv64"`.

| Component | Location | Responsibility |
|-----------|----------|----------------|
| `Capability`, `cap_lookup` | `arch/riscv64/src/cap.rs` *(new)* | Pure: `enum Capability { Endpoint(EndpointId) }`; `cap_lookup(caps, idx) -> Option<EndpointId>` (in-range + occupied + correct type). Host-tested. |
| `Message`, `TaskState::Blocked`, `Task.caps`, `Task.message` | `arch/riscv64/src/task.rs` | `Message { badge: usize, data: [usize; 3] }` (Copy); `Blocked { endpoint: EndpointId, role: IpcRole }` (`IpcRole { Send, Recv }`); a per-task cap table `caps: [Option<Capability>; CAP_SLOTS]` and an in-flight `message` slot. |
| `ipc_send`, `ipc_recv`, `block_current`, `grant_cap`; `spawn`/`spawn_user` return the slot | `arch/riscv64/src/sched.rs` | The rendezvous over `SCHED`: scan for a blocked peer on the endpoint; deliver-and-wake or block-and-switch. `grant_cap(slot, cap_slot, cap)` installs a capability. |
| `Send`/`Recv` decode + dispatch routing | `arch/riscv64/src/syscall.rs` | `decode_syscall` adds 4→`Send`, 5→`Recv`; `dispatch` routes them to `sched::ipc_send`/`ipc_recv` (which read/write `frame` and may block), returning `Outcome::Resume`. |
| Demo | `kernel/src/main.rs` | `server`, `client`, `rogue`, `idle`; boot installs the endpoint cap into `server`/`client` only. |

### 3.2 Capabilities as unforgeable table indices

Each task owns a small fixed capability table, `caps: [Option<Capability>;
CAP_SLOTS]` (`CAP_SLOTS` = 4). A capability is `Capability::Endpoint(id)`.
Syscalls name an endpoint by a **capability index** into the *calling
task's own* table; the kernel does `cap_lookup(&task.caps, idx)`:

```
cap_lookup(caps, idx):
    caps.get(idx)        // in range?
        .and_then(|slot| *slot)      // occupied?
        .map(|Capability::Endpoint(id)| id)   // (only type today)
```

Unforgeability is structural: a U-mode task holds only *indices* into a
table the kernel populated; it cannot manufacture a `Capability` or point
at a kernel object it was never granted. The "capability check" is simply
this lookup returning `Some`. `cap_lookup` is pure and host-tested
(in-range/occupied/out-of-range; the wrong-type arm exists for when more
types arrive).

Capabilities are installed at boot: `sched::grant_cap(slot, cap_slot, cap)`
writes `caps[cap_slot]` of the task in scheduler `slot`. `spawn`/`spawn_user`
return the assigned slot so `kmain` can grant.

### 3.3 The endpoint, the wait queue, and the rendezvous

An **endpoint** is a symbolic `EndpointId` (a `usize`); there is no separate
object to allocate. Its **wait queue** is implicit: the set of tasks whose
state is `Blocked { endpoint: this, .. }`, found by scanning the (tiny)
task array. This keeps the kernel object model minimal while still being a
real per-endpoint queue.

Synchronous rendezvous, both directions (all under `SCHED.with`, interrupts
off in the trap handler):

- **`recv`, a sender is waiting** (`Blocked{ep, Send}`): take that sender's
  `message`, set the sender `Ready`, return the message to the receiver.
  Neither blocks.
- **`recv`, no sender:** mark self `Blocked{ep, Recv}` and `block_current()`
  (switch away). When a sender later delivers, it writes this task's
  `message` and sets it `Ready`; on resume `recv` reads `self.message` and
  returns it.
- **`send`, a receiver is waiting** (`Blocked{ep, Recv}`): write the
  receiver's `message`, set it `Ready`, return ok. Neither blocks.
- **`send`, no receiver:** store the message in `self.message`, mark self
  `Blocked{ep, Send}`, `block_current()`. A later `recv` takes it and wakes
  us; on resume `send` returns ok.

The demo exercises the **receiver-blocks** path (server `recv`s first); the
sender-blocks path is implemented and symmetric.

### 3.4 Blocking inside a syscall — `block_current`

`block_current()` mirrors `yield_now` (disable interrupts,
`satp_write(next.satp)`, `switch_context`) with one difference: the current
task has already been marked `Blocked`, so `pick_next` (which returns only
`Ready` slots) never re-picks it until something sets it `Ready`. The
**idle** task is always `Ready`, so a successor always exists — the system
cannot fully deadlock even if every user task blocks.

The blocked task's `TrapFrame` remains on its kernel stack (the trap entry
saved it; `switch_context` preserves the stack). When the task is woken and
rescheduled, control returns *inside* the `ipc_recv`/`ipc_send` call (the
`frame: &mut TrapFrame` argument is still valid — it lives on this kernel
stack). The handler then writes the message into `frame.regs` and returns
normally; the trap return restores the (now message-bearing) frame and
`sret`s, delivering the message in registers. This is the same "resume
through the trap return" property 3b-i/3b-ii rely on, so `switch_context`
and the trap entry/exit assembly are **unchanged** again.

### 3.5 Syscall ABI

Two new syscalls (the surface is now `print`=1, `exit`=2, `yield`=3,
`send`=4, `recv`=5):

| # | Name | Args | Returns |
|---|------|------|---------|
| 4 | `send` | a0 = cap index, a1 = badge, a2–a4 = data[3] | a0 = 0 ok, or `usize::MAX` if the cap index is empty/out of range/not an endpoint |
| 5 | `recv` | a0 = cap index | a0 = badge, a1–a3 = data (on a valid cap); `usize::MAX` in a0 if the cap is invalid |

`Message = { badge: usize, data: [usize; 3] }`, carried **only in
registers** — the kernel copies it task→task with no memory access, so
there is no buffer pointer to validate. Register/`TrapFrame` mapping is the
3a convention (`regs[n-1]` = `x_n`; a0 = `regs[9]`, a1 = `regs[10]`, …,
a7 = `regs[16]`).

`dispatch` decodes the number and calls `sched::ipc_send(frame)` /
`sched::ipc_recv(frame)`, which read the ABI registers, do the rendezvous
(possibly blocking), write results into `frame`, and return
`Outcome::Resume`. The trap handler's `UserEcall` arm is unchanged
(`Resume` → advance `sepc` past the `ecall`). A blocked-then-woken syscall
also ends in `Resume`, so `sepc` advances exactly once.

### 3.6 The demo and its proofs

`kmain` (after the 2a/2b probes) builds each U-mode task in its own address
space (3b-ii `build_user_space`), grants the endpoint capability to
`server` and `client` only, then `enter`s. Spawn order: `server`,
`client`, `rogue`, `idle`.

- **`server`** (slot 0): `recv(cap=0)` → no sender yet → **blocks** (kernel
  logs `ipc: 'server' blocks on recv`); on wake, returns `badge` and
  `exit(badge)`.
- **`client`** (slot 1): `send(cap=0, badge=0x42)` → finds the blocked
  server → delivers (kernel logs `ipc: 'client' -> 'server' badge 0x42`),
  wakes it; `exit(0)`.
- **`rogue`** (slot 2): has **no** endpoint cap; `send(cap=0, …)` →
  `cap_lookup` fails → `a0 = MAX` (kernel logs `ipc: 'rogue' send rejected
  (no capability)`); `exit(7)`.
- **`idle`** (slot 3): the kernel idle task (3b-i), keeps the system alive.

Done-when proofs (smoke-test lines): `ipc: 'server' blocks on recv`;
`sched: task 'server' exited (code 66)` (0x42 = 66 — the value the client
sent, received end-to-end by the server in its own address space);
`sched: task 'rogue' exited (code 7)` (capability enforcement);
`sched: task 'client' exited (code 0)`.

### 3.7 Error handling summary

| Failure | Behavior |
|---------|----------|
| `send`/`recv` with an empty/out-of-range/wrong-type cap index | `a0 = usize::MAX`, resume; no block, no transfer (the enforcement path). |
| Every user task blocked at once | The always-`Ready` idle task runs; no total deadlock (a task with no peer simply stays blocked — that is correct synchronous-IPC behavior, not a kernel fault). |
| Unknown syscall number | `a0 = usize::MAX`, resume (unchanged). |
| Fatal U-mode fault / S-mode W^X probe | Unchanged (3b-i/3b-ii / 2b paths). |
| Re-entrant scheduler/trap | The `SingleHartCell` tripwire still applies. |

## 4. Testing

Test-first, per house discipline:

- **Host unit tests** (pure cores):
  - `cap_lookup`: a granted endpoint at index *i* → `Some(id)`; an empty
    slot → `None`; an out-of-range index → `None`. (A wrong-type arm is
    present for future types.)
  - `decode_syscall`: 4 → `Send`, 5 → `Recv`; existing 1/2/3/unknown stay
    green.
  - `pick_next` skips a `Blocked` slot (it is non-`Ready`); existing
    layout/`pick_next` tests stay green with the new `Task` fields threaded
    through the test helper.
- **QEMU smoke test** (`tools/test-qemu.ps1`, extended; written first,
  failing): keeps 2a/2b, adds the §3.6 proof lines (server blocks; server
  exits with the delivered value; rogue rejected/exits 7; client exits 0)
  plus `tick: 2`.

## 5. Deliverables

1. `cap.rs` (new): `Capability`, `EndpointId`, `cap_lookup` + tests.
2. `task.rs`: `Message`, `IpcRole`, `TaskState::Blocked`, `Task.caps`,
   `Task.message`.
3. `sched.rs`: `ipc_send`, `ipc_recv`, `block_current`, `grant_cap`;
   `spawn`/`spawn_user` return the slot index.
4. `syscall.rs`: `Send`/`Recv` decode + `dispatch` routing.
5. `kmain`: `server`/`client`/`rogue`/`idle` demo; boot-time cap grants;
   `sys_send`/`sys_recv` user stubs.
6. Extended QEMU smoke test + host unit tests, all green.
7. Short learning note `docs/learning/0009-capabilities-and-ipc.md`.
8. Roadmap: 3b-iii (and Phase 3b overall) marked done with date.
9. Glossary: capability, endpoint, IPC, synchronous/rendezvous, blocking —
   only genuinely new terms.

## 6. Open questions (for later phases)

- **Capability delegation (next):** sending a capability through IPC to
  grant authority dynamically — the move from "caps installed at boot" to
  a real capability system.
- **Call/reply + a reply capability:** efficient RPC; one-use reply caps.
- **More object types & rights:** memory, IRQ, task-control capabilities;
  per-cap rights and revocation.
- **Reaping:** freeing an exited task's page table, stacks, and cap table
  (and, once endpoints are objects, endpoints) without racing a switch.
- **Bounded queues / priorities:** when many tasks contend one endpoint.
