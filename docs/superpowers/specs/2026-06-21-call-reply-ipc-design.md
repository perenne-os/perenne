# Kernel — Design: call/reply IPC

- **Date:** 2026-06-21
- **Status:** Draft — awaiting review
- **Scope of this document:** add request/response (call/reply) IPC to the
  capability microkernel, so a server can return a value to the specific task
  that called it — and convert the RTC component to use it. Fully
  QEMU-testable.

---

## 0. Where this sits

The kernel has synchronous one-way IPC (3b-iii): a client `send`s a
register-only `Message`, a server `recv`s it. There is no way for a server to
*answer* a caller, so the existing components work around it: the RTC server
reports the time via its **exit code**, and the entropy component uses a
one-way `send` to a consumer. Call/reply closes that gap — the canonical RPC
pattern — and is the natural next IPC step the roadmap names.

It builds directly on the existing rendezvous (`ipc_send`/`ipc_recv`,
`block_current`, `find_blocked`, `TaskState::Blocked`, `Message`, `IpcRole`).

## 1. Goal

A client issues `call(ep, request)` — atomically sending a request to an
endpoint and blocking for the reply — and a server, after `recv`ing the
request, issues `reply(response)` to answer that exact caller. The kernel
binds each reply to its caller (a "current caller" recorded on the server when
it receives a Call); no new capability type is needed, and a server can only
ever reply to whoever just called it.

**You learn (kept brief):** how request/response RPC is built from the
one-way rendezvous — `call` is "send + await reply" as one atomic step, and
`reply` is a targeted wake of the recorded caller; and why a synchronous,
single-hart, one-call-at-a-time kernel can bind replies with a single
back-pointer instead of seL4's one-shot reply capabilities.

**Done when** `./tools/test-qemu.ps1` observes, alongside every existing
milestone:

1. **A reply crosses back to the caller** — the RTC component is now a real
   server (`loop { recv; read clock; reply(time) }`); the client `call`s it
   and exits with the returned time:
   `sched: task 'client' exited (code <15+ digit nanoseconds>)`. The server
   keeps running (it no longer exits), and a rogue lacking the endpoint
   capability is still refused.

And off the bare target:

2. **Host unit tests** — the two new syscall numbers decode correctly.

## 2. Non-goals (deferred)

- **One-shot reply capabilities** (seL4-style) — deferred. With
  caller-tracking, a server handles **one outstanding call at a time** and
  cannot save/defer/forward a reply. That is sufficient for our synchronous,
  single-hart servers; reply-caps are the future generalization.
- **Timeouts / reply reclamation** — if a server crashes before replying, its
  caller stays blocked (`AwaitingReply`) indefinitely. No timeout mechanism in
  this phase (the demo server does not crash; self-healing of a stuck caller
  is future work).
- **Multi-message / memory-carrying calls** — the request and reply are the
  existing register-only `Message` (badge + 3 data words). No buffers.
- **Converting the entropy component** — it is a one-way *producer*, not a
  request/response server; it keeps `send`.
- **Pipelined / out-of-order replies** — a server must `reply` before its next
  `recv` (the recorded caller is overwritten otherwise).

## 3. Design

### 3.1 Components

| Component | Location | Responsibility |
|-----------|----------|----------------|
| `IpcRole::Call`, `TaskState::AwaitingReply`, `Task.caller` | `arch/riscv64/src/task.rs` | The new wait states and the server→caller back-pointer. |
| `Syscall::Call` / `Syscall::Reply` + decode/dispatch | `arch/riscv64/src/syscall.rs` | Two new syscall numbers (`a7 = 7` / `a7 = 8`); pure decode (host-tested) + dispatch to `sched`. |
| `ipc_call`, `ipc_reply`, caller-binding in `ipc_recv` | `arch/riscv64/src/sched.rs` | The rendezvous: `call` (send + block for reply), `recv` binding a Call's caller, `reply` waking it. |
| RTC conversion | `kernel/src/main.rs` | `rtc_server` loops `recv`→read→`reply`; `rtc_client` `call`s and exits with the returned time. |

### 3.2 States and data

- `IpcRole::Call` — a caller queued at an endpoint, waiting for a server to
  pick up its request. A server's `recv` matches a waiting `Send` **or**
  `Call` peer; the difference is what happens on delivery.
- `TaskState::AwaitingReply` — a caller whose request has been delivered, now
  blocked until the server replies. It is on no endpoint queue (so no second
  `recv` can re-match it) and `pick_next` skips it.
- `Task.caller: Option<usize>` — set on a server when it receives a Call (the
  caller's scheduler slot); read and cleared by `reply`.

### 3.3 The rendezvous

**`call(ep_cap, msg)`** (in `sched::ipc_call`, mirroring `ipc_send` but for a
two-way exchange):
1. Look up `ep` via `cap_lookup`; bad cap → return `usize::MAX`.
2. If a server is `Blocked{ep, Recv}`: deliver `msg` to it, set
   `server.caller = current`, wake the server (`Ready`); put the caller into
   `AwaitingReply` and `block_current`.
3. Else: store `msg` as the caller's outgoing message, mark it
   `Blocked{ep, Call}`, and `block_current`. (A later server `recv` picks it
   up, binds the caller, and moves it to `AwaitingReply`.)
4. On resume (the server replied), the reply `Message` is in the caller's
   `message`; return it in the ABI registers.

**`recv(ep_cap)`** (extend `ipc_recv`): when scanning for a waiting peer,
match `Blocked{ep, Send}` **or** `Blocked{ep, Call}`. On delivery:
- a `Send` peer → wake it `Ready` (one-way, as today; no caller bound).
- a `Call` peer → set `current.caller = peer`, move the peer to
  `AwaitingReply` (it stays blocked, now for the reply).
When the server itself blocks (no waiting peer) and is later woken by a Call,
the `call` path (step 2 above) sets `caller` before waking it.

**`reply(msg)`** (in `sched::ipc_reply`): take `current.caller`; if `None` (or
the caller is `Exited`), return `usize::MAX` (nothing to reply to). Otherwise
write the caller's `message = msg`, set it `Ready`, clear `current.caller`,
return `0`. The caller's `call` then returns `msg`.

`switch_context` and the trap asm are unchanged (call/reply is built entirely
from the existing block/wake primitives).

### 3.4 Syscall ABI

| Syscall | `a7` | Args | Returns |
|---------|------|------|---------|
| `call` | 7 | `a0`=ep cap, `a1`=badge, `a2..a4`=data | `a0`=reply badge, `a1..a3`=reply data (or `a0`=`usize::MAX` on bad cap) |
| `reply` | 8 | `a0`=badge, `a1..a3`=data | `a0`=0, or `usize::MAX` if no pending caller |

(`call` reuses the `send` argument layout for the request and the `recv`
return layout for the reply; `reply` reuses the `send` layout for the answer.)

### 3.5 The demo — RTC over call/reply

`rtc_server` becomes a real server:

```
loop {
    recv(EP);                 // a request (the kernel records the caller)
    let t = read_rtc();       // inline-asm MMIO (unchanged)
    reply(EP_reply = t);      // answer the caller with the time
}
```

`rtc_client`:

```
let t = call(EP, request);    // send + block; returns the time
exit(t);                      // the kernel prints "client exited (code <t>)"
```

The time now crosses **back** to the caller (instead of the server exiting
with it). The server loops, serving repeatedly; `idle` keeps the system alive
while it blocks. `rogue` (no capability) still has its `call`/`send` refused.

### 3.6 Error handling summary

| Situation | Behavior |
|-----------|----------|
| `call` with no/wrong capability | `a0 = usize::MAX`; caller not blocked. |
| `reply` with no pending caller | `a0 = usize::MAX`; no-op. |
| `reply` to a caller that has since `Exited` | No-op (skip), `a0 = usize::MAX`; server continues. |
| Server crashes before replying | Its caller stays `AwaitingReply` (deferred limitation; no timeout). |
| Plain `send`/`recv` | Unchanged (no caller bound; `reply` afterward errors). |

## 4. Testing

- **Host unit tests** (`arch/riscv64`, `cargo test`): `decode_syscall(7) ==
  Call`, `decode_syscall(8) == Reply` (and the existing numbers still decode).
- **QEMU smoke test** (`tools/test-qemu.ps1`, extended; written first,
  failing): replace the RTC server-exits assertion with
  `sched: task 'client' exited \(code \d{15,}\)` (the client now receives the
  time); keep `ipc: 'rtc' blocks on recv`, the rogue-refused line, and every
  other milestone.

## 5. Deliverables

1. `task.rs`: `IpcRole::Call`, `TaskState::AwaitingReply`, `Task.caller`
   (default `None` at every construction site).
2. `syscall.rs`: `Syscall::Call`/`Reply`, decode (`7`/`8`) + dispatch, host
   tests.
3. `sched.rs`: `ipc_call`, `ipc_reply`, and the Call-caller binding in
   `ipc_recv`.
4. `kernel/src/main.rs`: `sys_call`/`sys_reply` wrappers; `rtc_server` loops
   with `recv`+`reply`; `rtc_client` uses `call` and exits with the time.
5. Extended QEMU smoke test + host tests, all green.
6. Short learning note `docs/learning/0017-call-reply-ipc.md`.
7. Roadmap: call/reply marked done (the "next candidate" note updated).
8. Glossary: only genuinely new terms (e.g. *call/reply (RPC)*).

## 6. Open questions (for later phases)

- **One-shot reply capabilities**: deferred/forwarded replies, multiple calls
  in flight, and a server answering callers out of order.
- **Timeouts / reply reclamation** so a crashed server doesn't strand its
  caller (ties into self-healing).
- **Memory-carrying messages** (shared buffers / grants) for payloads larger
  than the register message.
- **A higher-level RPC stub** so components don't hand-roll the
  `call`/`recv`/`reply` ABI.
