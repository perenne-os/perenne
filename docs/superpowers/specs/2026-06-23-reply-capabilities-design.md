# Kernel — Design: one-shot reply capabilities

- **Date:** 2026-06-23
- **Status:** Draft — awaiting review
- **Scope of this document:** generalize call/reply IPC so a server can hold
  multiple outstanding calls and reply to them in any order, by minting a
  one-shot **reply capability** per received Call (replacing the single
  `caller` binding). Fully QEMU-testable.

---

## 0. Where this sits

Call/reply IPC (the previous IPC phase) lets a server answer the task that
called it, but the kernel tracks a **single** caller per server (`Task.caller`).
So a server must `reply` before its next `recv` — no deferring, no multiple
calls in flight, no out-of-order replies. One-shot reply capabilities lift that
limit, the roadmap's named next IPC step.

It builds directly on call/reply (`ipc_call`/`ipc_recv`/`ipc_reply`,
`TaskState::AwaitingReply`, `Task.caller`) and the capability model
(`cap::Capability`, the per-task cap table).

## 1. Goal

When a server `recv`s a Call, the kernel installs a **`Reply` capability**
(naming the caller) into a server-chosen slot of the server's cap table. The
server replies through that capability later — after receiving other calls, in
any order. Each reply cap is **one-shot**: replying consumes it. This makes the
durable "who do I owe a reply" a capability, one per outstanding call, instead
of a single field.

**You learn (kept brief):** why a server that handles more than one call at a
time needs a *capability per outstanding reply* rather than a single binding;
how a one-shot capability is minted on receive and consumed on use; and why,
in a blocking kernel where a caller can't run (or be reused) until it is
replied, no generation/staleness guard is needed.

**Done when** `./tools/test-qemu.ps1` observes, alongside every existing
milestone:

1. **Multiple in flight, out of order** — a `deferrer` server receives two
   calls (from `dclientA` with request `0xa1` and `dclientB` with `0xb1`)
   *before replying to either*, holding two reply caps, then replies to **B
   first, then A**. The server returns `request | 0x100`, so each client exits
   with its distinct reply value: `sched: task 'dclientA' exited (code 417)`
   (0x1a1) and `sched: task 'dclientB' exited (code 433)` (0x1b1). The RTC
   component still works on the new ABI.

And off the bare target:

2. **Host unit tests** — `cap::reply_caller` lookup; `decode_syscall` for the
   (unchanged-numbered) call/recv/reply still holds.

## 2. Non-goals (deferred)

- **Forwarding a reply cap to another component** — needs capability
  delegation through IPC (moving a cap between cap tables), which the kernel
  does not have. A reply cap stays in the server that minted it. So this
  phase delivers *multiple-in-flight + deferred/out-of-order replies within one
  server*, not forwarding.
- **A generation/staleness guard** — unnecessary: a caller is `AwaitingReply`
  (blocked, cannot run, exit, or be reused) until replied, and the cap is
  consumed on use, so a `Reply` cap always names a valid waiting caller.
  (Revisit if task-slot reaping is ever added.)
- **Growing `CAP_SLOTS`** — stays 4; enough for an endpoint + a few reply caps.
  A server that exceeds its slots gets a mint failure (handled, see §3.5).
- **Changing the caller side of `call`** — `ipc_call` and `AwaitingReply` are
  unchanged; only the server's receive/reply path changes.

## 3. Design

### 3.1 Components

| Component | Location | Responsibility |
|-----------|----------|----------------|
| `Capability::Reply` + `reply_caller` | `arch/riscv64/src/cap.rs` | New cap variant `Reply(usize)` (the caller's scheduler slot); pure `reply_caller(caps, idx) -> Option<usize>` lookup (host-tested). |
| Mint-on-receive | `arch/riscv64/src/sched.rs` (`ipc_recv`) | On a received Call, install `Reply(caller)` into the server's `caps[reply_slot]` (the slot the server passed to `recv`). |
| Reply-through-cap | `arch/riscv64/src/sched.rs` (`ipc_reply`) | `reply(reply_slot, msg)`: look up the `Reply` cap, wake the caller, consume the cap. |
| ABI | `kernel/src/main.rs` | `recv` gains a `reply_slot` arg; `reply` takes a `reply_slot` (not the recorded caller). |
| RTC conversion + demo | `kernel/src/main.rs` | RTC server uses the new ABI; a `deferrer` server + `dclientA`/`dclientB` exercise two-in-flight + out-of-order. |

### 3.2 The capability

`cap.rs` gains:

```
pub enum Capability {
    Endpoint(EndpointId),
    Restart(usize),
    Randomness,
    Interrupt(u32),
    Reply(usize),        // authority to reply to the caller in this slot (one-shot)
}

/// The caller a one-shot `Reply` capability at `idx` answers. `None` for an
/// empty slot, an out-of-range index, or the wrong cap type.
pub fn reply_caller(caps: &[Option<Capability>], idx: usize) -> Option<usize> {
    match caps.get(idx) {
        Some(Some(Capability::Reply(slot))) => Some(*slot),
        _ => None,
    }
}
```

### 3.3 Receive: mint a reply cap

`recv` gains a `reply_slot` argument (`a1`): the server tells the kernel where
to put the minted reply cap. On a **Call** the kernel installs `Reply(caller)`
into the server's `caps[reply_slot]`; on a one-way **Send**, nothing is
installed (one-way recv users pass a dummy slot, ignored). The caller slot is:

- the **immediate** path (server `recv`s an already-queued Call peer): the peer
  itself.
- the **blocked** path (server woken by `ipc_call`): `ipc_call` stashed the
  caller slot in the server's `caller` field, which the server's `recv`
  continuation reads and clears (`caller` survives only as this transient — the
  durable binding is now the reply cap).

`recv`'s message return (`a0`=badge, `a1..a3`=data) is unchanged; `reply_slot`
is the new `a1` *input* (clobbered by the data output on return).

### 3.4 Reply: consume a reply cap

`reply(reply_slot, msg)`: `reply_caller(self.caps, reply_slot)` → caller slot
(else `a0 = usize::MAX`). If the caller is `AwaitingReply`, deliver `msg`, set
it `Ready`, and **consume** the cap (`caps[reply_slot] = None`). A second reply
through the same slot finds no cap → refused (one-shot). ABI: `a0`=reply_slot,
`a1`=badge, `a2..a4`=data; returns `a0`=0 or `usize::MAX`.

### 3.5 Error handling summary

| Situation | Behavior |
|-----------|----------|
| `recv` delivers a Call | Mint `Reply(caller)` into `caps[reply_slot]`; return the request. |
| `recv` delivers a one-way Send | No cap minted (`reply_slot` ignored); return the message. |
| Mint fails (rare: caller missing) | Deliver the message anyway; the absent reply cap means `reply` will refuse (caller stays blocked — same stranding as a dropped reply today). |
| `reply` with a valid one-shot cap | Wake the caller, consume the cap, `a0 = 0`. |
| `reply` to an empty/wrong slot (incl. double-reply) | `a0 = usize::MAX`; no-op. |
| `reply` to a caller no longer `AwaitingReply` | Consume the cap, `a0 = usize::MAX` (defensive; can't normally happen). |
| Server overruns its cap slots | The server is responsible for replying/freeing; overwriting a live reply slot strands that caller (a server bug). |

### 3.6 The demo

- **RTC server** (one in flight): `loop { recv(EP, R); read clock; reply(R, t) }`
  with a fixed reply slot `R` — minimal change, proves the new ABI.
- **`deferrer` server** (two in flight, out of order): receive a call into reply
  slot 1, then a call into reply slot 2 (holding both), then `reply(2, …)`
  before `reply(1, …)`. It returns `request_badge | 0x100`.
- **`dclientA`/`dclientB`**: `let r = call(DEFER_EP, 0xa1 or 0xb1); exit(r)` —
  each exits with its reply value, distinct, proving the two calls were tracked
  independently and answered out of order.
- A reserved `DEFER_EP` endpoint; the server and both clients hold its endpoint
  cap. `MAX_TASKS` rises to fit `deferrer` + `dclientA` + `dclientB`.

## 4. Testing

- **Host unit tests** (`arch/riscv64`): `cap::reply_caller` — returns the
  caller for a `Reply` cap; `None` for wrong-type/empty/out-of-range.
- **QEMU smoke test** (`tools/test-qemu.ps1`, extended; written first,
  failing): the server returns `request | 0x100`, so with requests `0xa1`/`0xb1`
  the replies are `0x1a1`/`0x1b1` = 417/433. Assert
  `sched: task 'dclientA' exited \(code 417\)` and
  `sched: task 'dclientB' exited \(code 433\)`; keep the RTC client line and
  every other milestone.

## 5. Deliverables

1. `cap.rs`: `Capability::Reply(usize)` + `reply_caller` + host tests.
2. `sched.rs`: mint-on-receive in `ipc_recv` (`reply_slot` arg); reply-through-
   cap in `ipc_reply` (`reply_slot` arg); `Task.caller` retained as the recv
   transient.
3. `kernel/src/main.rs`: `sys_recv`/`sys_reply` wrappers take a reply slot; RTC
   server/converted; `deferrer` + `dclientA`/`dclientB` + their stacks, grants,
   `DEFER_EP`; `MAX_TASKS` bump.
4. Extended QEMU smoke test + host tests, all green.
5. Short learning note `docs/learning/0021-reply-capabilities.md`.
6. Roadmap: reply capabilities marked done.
7. Glossary: *reply capability (one-shot)* only.

## 6. Open questions (for later phases)

- **Capability delegation through IPC** — lets reply caps (and other caps) be
  *forwarded* between components; the larger capability-transfer feature.
- **Reaping / generations** — if task slots are ever reclaimed, reply caps need
  a generation to detect a reused slot.
- **A higher-level RPC stub** so servers don't hand-roll reply-slot management.
