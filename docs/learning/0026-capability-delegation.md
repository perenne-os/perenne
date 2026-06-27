# 0026 — Capability delegation through IPC

**One-line:** a component can now hand a capability it holds to another
component at runtime — `grant` copies a cap from the sender's table into a
recv-blocked peer's table, kernel-mediated and unforgeable. The authority graph
stops being frozen at boot and becomes dynamic.

## What changed
- A new **`grant` syscall** (a7 = 11): `grant(ep_cap, src_cap_slot, badge)`
  delegates (copies) the capability in the sender's `src_cap_slot` to a peer
  recv-blocked on `ep_cap`. **Copy semantics** — the sender keeps its cap, so a
  broker can hand the same service to many clients.
- Pure `cap::cap_at(caps, idx)` reads the whole `Capability` to copy — and is
  the **unforgeability guard**: it returns `None` for an empty/out-of-range
  slot, so a component can only delegate a cap it actually holds.
- The receiver names where the delegated cap lands using the **same `recv`
  reply-slot** it already passes (`a1`) — one slot, one meaning: "the slot the
  kernel installs an incoming capability into this recv" (a reply cap for a
  `Call`, or a delegated cap for a `grant`).
- `Task.pending_grant` carries the cap in transit; `install_cap` is the
  slot-bounded write (a generalization of `mint_reply_cap`).

## The idea worth keeping
**Delegation is the kernel's existing reply-cap step, made available to
components.** The kernel already installed a capability into a receiver's table
mid-IPC — that is exactly what minting a one-shot `Reply` cap does when a server
receives a `Call`. `grant` generalizes that one move: instead of the *kernel*
minting a cap, a *component* nominates one of its own caps to be copied in.
Unforgeability is preserved because the kernel reads the cap out of the sender's
own table (it can't name what it wasn't given) and copies the real `Capability`
value (the receiver can't fabricate). So dynamic delegation needed almost no new
trust machinery — just a guarded read and a bounded write, riding the rendezvous
that was already there.

## The demo
`broker` holds the RTC endpoint cap and a grant-channel cap; `needy` holds the
grant channel but **no** RTC cap. `broker` first attempts a bad grant (an empty
source slot → `cap: 'broker' grant rejected (no capability in slot)`, the guard
firing), then delegates the RTC endpoint to `needy` (`cap: 'broker' delegated
Endpoint(0) to 'needy'`). `needy` then `call`s the RTC server on the
now-delegated cap and exits with the live clock — it reached the server **only**
because authority was delegated to it at runtime.

## Why it matters
This is the foundational capability operation a real capability system needs,
and what makes ADR 0007 (extensibility via capability-holding components) real:
authority can flow between components without the kernel pre-wiring every
relationship at boot. A vendor/community component can be handed exactly the
authority it needs, by another component, at runtime.

## What's next
Deferred: **move/transfer** semantics (conserve authority, one holder),
**rights attenuation** (delegate with reduced rights — needs rights bits we
don't have yet), **revocation**, and **reply-cap forwarding** (a server handing
its one-shot reply cap to a worker — now mechanically possible on this `grant`).
