# Phase 13 — Capability revocation: take back delegated authority (design)

**Status:** approved 2026-06-27 (user authorized completing the phase end-to-end)
**Priority served:** #2 (minimal-TCB trust posture) and #3 (capability-based
extensibility, ADR 0007). Completes the delegate → revoke pair.

## The gap

Phase 8 let a component *delegate* a capability it holds; nothing can **take it
back**. Revocation — invalidating authority you handed out — is the famously hard
part of capability systems, and it is central to a real trust posture: you cannot
claim minimal, controllable authority if granted authority is permanent. Phase 13
adds it: the kernel revokes an endpoint capability, and every holder's next use of
it fails.

## The idea (transitive sweep, no derivation tree)

Capabilities here are entries in per-task capability tables (CSpaces); an endpoint
is symbolic (its "wait queue" is the set of tasks blocked on its id). Revocation
is a **transitive sweep-and-clear**: revoking an endpoint scans every component's
cap table and clears every `Endpoint(ep)` entry, invalidating *all* copies at
once — without a capability derivation tree (the CDT that makes fine-grained
seL4-style revocation complex). It reuses what exists: a cleared slot is `None`,
so the current `cap_lookup` returns `None` and the holder's IPC fails with **no
new check on the hot path**.

**Authorization:** holding an endpoint capability authorizes revoking that
endpoint. A `revoke(ep_cap)` syscall sweeps every *other* holder and keeps the
caller's own cap — "take back what I granted, keep my key."

Epoch/generation revocation (O(1) instead of O(tasks), and re-grantable) is the
noted alternative; the sweep is chosen for **zero churn** to the `Capability`
enum and every existing grant site, while still teaching transitive revocation.

## Scope (YAGNI)

- Transitive **per-endpoint** revocation, authorized by holding the cap,
  caller-preserving (the caller keeps its own cap).
- A dedicated, isolated demo (a `lease_server` + `tenant` on a new `LEASE_EP`) so
  revocation does not disturb the RTC / delegation / self-healing proofs.
- Deferred: epoch/generation revocation (O(1), re-grantable), a derivation tree
  for "revoke only the descendants of this grant," and revoking non-endpoint cap
  types.

## Architecture & components

### Pure, host-tested logic (`arch/riscv64/src/cap.rs`)

- **`revoke_in_caps(caps: &mut [Option<Capability>], ep: EndpointId) -> usize`**
  — clear every slot holding `Capability::Endpoint(ep)` in one table; return the
  count cleared. Leaves other endpoint ids and other capability types untouched.

### Kernel rendezvous (`arch/riscv64/src/sched.rs`)

- **`revoke_endpoint(ep: EndpointId, except: usize) -> usize`** — for every task
  except `except` (the caller), run `revoke_in_caps` on its cap table; sum the
  counts.
- **`ipc_revoke(frame)`** — the `revoke` syscall: `a0` = the slot of an
  `Endpoint` cap. `cap_lookup` authorizes (the caller must hold the endpoint);
  then `revoke_endpoint(ep, current)`, log
  `cap: revoked <ep> from N holder(s)`, and return the count in `a0`. `usize::MAX`
  if the caller lacks the capability (the authorization guard).

### Syscall plumbing (`arch/riscv64/src/syscall.rs`, `kernel/src/main.rs`)

- `Syscall::Revoke` (a7 = 12); a `sys_revoke(ep_cap) -> usize` U-mode wrapper.

### Demo cast (`kernel/src/main.rs`)

A self-contained lease/tenant pair on a dedicated `LEASE_EP` (distinct from EP0
and the Phase 8 grant channel), so the existing proofs are untouched:

- **`lease_server`** holds `Endpoint(LEASE_EP)` (for recv + the revoke
  authority). It `recv`s the tenant's call, `reply`s, then **revokes**
  `LEASE_EP` (clearing the tenant's cap, keeping its own), then idles.
- **`tenant`** is granted `Endpoint(LEASE_EP)`. It `call`s twice: the first
  succeeds (reply received); the second returns `usize::MAX` because its cap was
  revoked in between. It exits with a marker code proving "call 1 ok, call 2
  revoked."

`MAX_TASKS` 20 → 22 for the two demo tasks (add two `None` to the initializer).

## Data flow (the proof)

`tenant` `call`s (1) → `lease_server` recv + `reply` → (server still running)
`revoke(LEASE_EP)` clears the tenant's cap → server blocks/idles → scheduler runs
`tenant` → `call` (2) → `cap_lookup` is `None` → `usize::MAX` → `tenant` exits
with the marker. The ordering is deterministic: after `reply` the server keeps
running and executes `revoke` before it yields, so the tenant's cap is gone
before its second call is scheduled.

Markers: `cap: revoked LEASE_EP from 1 holder(s)` and
`sched: task 'tenant' exited (code 13)` (13 = "used-then-revoked").

## Error handling

| Situation | Behavior |
|---|---|
| `revoke` without holding the endpoint cap | `usize::MAX`, no sweep (authorization guard). |
| A revoked holder's later `send`/`call`/`recv` | The existing bad-cap result (`usize::MAX`); no new path, no panic. |
| The caller's own cap | Preserved (sweep skips `except`), so a server can revoke tenants and keep serving. |
| Revoking an endpoint nobody else holds | Count 0; harmless. |

## Testing

**Host unit tests (`cap.rs`):** `revoke_in_caps` clears matching `Endpoint(ep)`
caps and counts them; leaves a different endpoint id and non-endpoint caps
(Restart/Reply/Randomness/Interrupt) intact; returns 0 when the endpoint is
absent.

**Boot test (existing two-boot harness, boot 1):** assert
`cap: revoked LEASE_EP from 1 holder(s)` and
`sched: task 'tenant' exited (code 13)` — the holder used the cap once, then its
revoked cap was rejected. Boot 2 is unaffected (the demo runs each boot
identically; the assertion lives in boot 1).

## What this proves / what's next

The kernel can take back delegated authority, transitively, invalidating all
copies — the revocation half of a real capability system, and a concrete trust-
posture guarantee (granted authority is controllable, not permanent). Deferred:
epoch/generation revocation (O(1), re-grantable), a capability derivation tree
for descendant-only revocation, and revoking non-endpoint capabilities.
