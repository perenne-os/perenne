# Phase 8 — Capability delegation through IPC (design)

**Status:** approved 2026-06-27 (user authorized completing the phase end-to-end)
**Priority served:** #1 (security / capabilities / minimal TCB) and ADR 0007
(extensibility via capability-holding components). Makes the authority graph
**dynamic**.

## The gap

Today every capability is granted statically at boot by the kernel
(`sched::grant_cap`). No component can hand authority to another at runtime — the
authority graph is frozen at boot. A real capability system delegates: a
component passes a capability it holds to another, kernel-mediated, so authority
can flow between components without the kernel pre-wiring every relationship.
This is the foundational capability operation the project memory has repeatedly
flagged as "the biggest next capability step," and what turns ADR 0007 from a
static wiring diagram into a living system.

## The idea (a precedent already in the kernel)

The kernel **already** installs a capability into a receiver's table during IPC:
that is exactly what `mint_reply_cap` does when a server `recv`s a `Call`.
Delegation generalizes that one step — instead of the *kernel* minting a cap, a
*component* nominates one of its **own** cap slots to be **copied** into a peer's
table. Unforgeability is preserved end to end: the sender can only delegate a cap
it actually holds (the kernel reads it from the sender's table), and the kernel
copies the real `Capability` value, so the receiver cannot fabricate anything.

## Scope (YAGNI)

- **Copy (grant) semantics**, not move: the sender keeps its cap; the receiver
  gets a duplicate — so a broker can delegate the same service to many clients.
  (Move/transfer noted as a future option.)
- **A dedicated `grant` syscall** (a7 = 11), leaving `send`/`call`/`reply`
  untouched — the same focused-syscall pattern as `restart`/`getrandom`/
  `wait_irq`. Lower risk, clear boundary, easy to test and review.
- Delegation rides the **existing send/recv rendezvous** (both the
  receiver-waiting and no-receiver-yet paths).
- **No rights attenuation, no revocation** (we have no rights bits yet) — the
  cap is copied as-is. Deferred.

## Architecture & components

### Pure, host-tested logic (`arch/riscv64/src/cap.rs`)

- **`cap_at(caps, idx) -> Option<Capability>`** — read the whole `Capability`
  at `idx` (the value the kernel will copy), or `None` for an empty slot or an
  out-of-range index. The generic read counterpart to the type-specific lookups
  (`cap_lookup`, `restart_target`, …). This is the unforgeability guard: a
  delegate of an absent slot returns `None` → the grant is rejected.

### Kernel rendezvous (`arch/riscv64/src/sched.rs`)

- **`install_cap(s, task, slot, cap)`** — install `cap` into `task`'s cap table
  at `slot`, a no-op if `slot >= CAP_SLOTS` (generalizes the slot-bounded write
  `mint_reply_cap` already does).
- **`ipc_grant(frame)`** — service the `grant` syscall. ABI:
  `a0` = endpoint cap index to send over, `a1` = the sender's **source** cap
  slot to delegate, `a2` = badge. Steps:
  1. `cap_lookup(a0)` authorizes the send (the sender must hold the endpoint) —
     else `a0 = usize::MAX`, logged `cap: '<name>' grant rejected (no endpoint
     capability)`.
  2. `cap_at(sender.caps, a1)` reads the cap to delegate — `None` → reject,
     logged `cap: '<name>' grant rejected (no capability in slot)`. This is the
     guard that a component can only delegate what it holds.
  3. If a peer is `recv`-blocked on the endpoint: `install_cap` the copied cap
     into that peer's table at the slot it named in `recv` (`a1` of its recv),
     deliver the badge message, wake it, log
     `cap: '<sender>' delegated <Cap> to '<receiver>'`. (Receiver-waiting path.)
  4. Else: stash the cap on `Task.pending_grant: Option<Capability>` alongside
     the message and block as a `Send`; a later `recv` installs it on pickup —
     symmetric to how a `Call` stashes `caller` for `mint_reply_cap`.
     (No-receiver-yet path.)
- **`ipc_recv`** gains one symmetric step: when it picks up a blocked sender that
  carries a `pending_grant`, it installs that cap into the receiver's named slot
  (the same slot it would put a reply cap). One slot, one meaning: "the slot the
  kernel installs an incoming capability into this recv."

### Syscall plumbing (`arch/riscv64/src/syscall.rs`, `kernel/src/main.rs`)

- `decode_syscall` maps a7 = 11 → `Syscall::Grant`; `dispatch` routes to
  `sched::ipc_grant`. A `sys_grant(ep, src_slot, badge)` U-mode wrapper.

### Demo cast (`kernel/src/main.rs`)

Replaces the static-cap `rtc_client` with the delegation flow (needy *is* an RTC
client — one that obtained its cap dynamically):
- `rtc_server` (unchanged) recv-blocks on `RTC_EP`.
- `needy` holds the grant-endpoint cap but **no** RTC cap. It `recv`s on the
  grant endpoint (naming a destination slot), then `call`s the RTC server on the
  now-delegated cap and exits with the live time.
- `broker` holds `Endpoint(RTC_EP)` + the grant-endpoint cap. It first attempts a
  bad grant (an empty slot → rejected, proving the guard), then `grant`s the RTC
  endpoint cap over the grant endpoint to `needy`.

Boot order (spawn `rtc_server`, then `needy`, then `broker`) makes both servers
recv-block before their senders act → the receiver-waiting path, matching the
existing rtc/blk ordering.

## Data flow (the proof)

`broker` grant(bad slot) → `cap: 'broker' grant rejected (no capability in slot)`
(the unforgeability guard fires). Then `broker` grant(RTC slot) → kernel copies
`Endpoint(RTC_EP)` into `needy`'s table → `cap: 'broker' delegated Endpoint to
'needy'`. `needy` `call`s RTC on the delegated cap → `rtc_server` replies the
clock → `sched: task 'needy' exited (code <live-clock ns>)`. `needy` held no RTC
capability statically; it reached the RTC server **only** because authority was
delegated to it at runtime — the dynamic authority graph, kernel-mediated and
unforgeable.

## Error handling

| Situation | Behavior |
|---|---|
| Grant over an endpoint the sender lacks | `a0 = usize::MAX`, logged; no transfer. |
| Grant of an empty/out-of-range source slot | rejected (`cap_at → None`), logged; the unforgeability guard. |
| Receiver named an out-of-range install slot | `install_cap` no-op (a receiver bug, not fatal) — message still delivered. |
| No receiver yet | sender blocks (Send) with the cap stashed; delivered when a receiver arrives. |

## Testing

**Host unit tests (`cap.rs`):** `cap_at` returns the full cap for a populated
slot (each variant), `None` for empty / out-of-range — the guard.

**Boot test (`tools/test-qemu.ps1`):** assert
`cap: 'broker' grant rejected (no capability in slot)`,
`cap: 'broker' delegated Endpoint to 'needy'`, and
`sched: task 'needy' exited (code \d{15,})` (the live clock obtained via the
delegated cap). Replace the old `'client'` exit assertion with `'needy'`.

## What this proves / what's next

Authority can flow between components at runtime, kernel-mediated and
unforgeable — the dynamic capability graph ADR 0007's extensibility needs.
Deferred: move/transfer semantics, rights attenuation, capability revocation,
and reply-cap forwarding (a server handing its reply cap to a worker — now
mechanically possible on this `grant`).
