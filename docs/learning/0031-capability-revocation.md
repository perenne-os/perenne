# 0031 — Capability revocation: take back delegated authority

**One-line:** the kernel can now **revoke** an endpoint capability — invalidating
every copy across every component's table — so a holder's next use of it fails.
The revocation half of a real capability system, and the partner to Phase 8's
delegation.

## What changed
- A `revoke` syscall (a7 = 12): `revoke(ep_cap)` revokes the endpoint named by a
  capability the caller holds, from **every other holder** (the caller keeps its
  own — "take back what I granted, keep my key").
- Pure `cap::revoke_in_caps(caps, ep)` clears every `Endpoint(ep)` slot in one
  table and counts them; `sched::revoke_endpoint(ep, except)` runs it over every
  task's CSpace except the caller's.
- A self-contained demo: a `lease_server` hands a `tenant` an `Endpoint(LEASE_EP)`
  cap, answers one call, then revokes the endpoint. The tenant's second call
  fails.

## The idea worth keeping: transitive revocation without a derivation tree
Revocation is famously the hard part of capability systems — once you have handed
out (and the holder may have re-delegated) a capability, how do you invalidate
*all* of it? seL4 keeps a **capability derivation tree** (CDT) to revoke a cap and
all its descendants. This kernel sidesteps that for the common case with a
**transitive sweep**: revoking an endpoint scans every CSpace and clears every
`Endpoint(ep)` entry. Because a cleared slot is just `None`, the *existing*
`cap_lookup` already returns `None` and the holder's IPC already fails — **no new
check on the hot path, no new state, no CDT.** Authority to revoke is simply
holding the cap (you have authority over the endpoint).

The trade-off vs the canonical mechanism: this is O(tasks) per revoke (a sweep)
and coarse (it revokes *all* holders of the endpoint, not just the descendants of
one grant). **Epoch/generation revocation** — give each endpoint a generation,
have a cap carry the generation it was minted at, and bump the generation to
revoke — is O(1) and re-grantable, but requires every endpoint capability to
carry a generation. Deferred; the sweep teaches the concept with zero churn to
the `Capability` type.

## A deterministic ordering
The demo relies on the server running `reply` → `revoke` → `exit` before the
tenant's second call is scheduled. After `reply`, the server keeps running (reply
returns to it); it revokes (clearing the tenant's cap) and only then yields. So
when the tenant resumes from its first call and issues its second, its cap is
already gone.

## Proof
`tenant`'s first `call` succeeds; `cap: 'lease' revoked endpoint 7 from 1
holder(s)`; the tenant's second `call` returns `usize::MAX` (cap revoked); it
exits with code 13 ("used-then-revoked"). Authority was taken back.

## What's next
Epoch/generation revocation (O(1), re-grantable); a capability derivation tree
for "revoke only the descendants of this grant"; and revoking non-endpoint
capability types.
