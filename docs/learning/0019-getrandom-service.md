# 0019 — getrandom: capability-gated entropy for U-mode

**One-line:** U-mode components can now draw from the kernel entropy pool
through a capability-gated `getrandom` syscall — because the CSPRNG can't run
unprivileged, a syscall (not a server) is the right interface.

## What changed
- The entropy pool moved from the kernel binary into the `arch` crate (with the
  scheduler/frame-allocator singletons) so the syscall layer can reach it.
- New `Capability::Randomness` + `has_randomness`; a `getrandom` syscall
  (`a7=9`) serviced in `sched`: it checks the caller holds the capability, then
  fills `a1..a4` with 32 fresh pool bytes (`a0=0`), else refuses (`a0=MAX`).
- A `rnguser` component proves the gating: refused without a capability, served
  with one.

## The ideas worth keeping
1. **A kernel-owned generator is exposed as a syscall, not a server.** The
   CSPRNG uses `core`/`.rodata` and can't run in U-mode, so no user-space "rng
   server" could hold it — it would only forward to this syscall. (Linux's
   `getrandom(2)` is a syscall for the same reason.)
2. **A capability can gate a non-IPC syscall.** Until now capabilities gated
   IPC (an endpoint index) and restart (a target). `getrandom` shows the same
   unforgeable-index check on an ordinary syscall: the caller names a
   `Randomness` capability it was granted; holders are served, others refused.
3. **Where state lives follows who consumes it.** Once a *syscall* consumes the
   pool, the pool belongs in `arch` (the syscall layer), not the kernel binary
   — even though that means `arch` now depends on `kernel_crypto`.

## A scheduling lesson (why the demo proves *gating*, not liveness)
`rnguser` first tried to also prove the bytes were *live* by checking two draws
differ. But the IPC interleave runs `rnguser` before the `pqc` task seeds the
pool (slot order doesn't imply run order once tasks block on each other), so it
saw two unseeded zero-draws and the check failed. The fix: let `rnguser` prove
the *capability gating* (its real job) and leave pool-liveness to the `pqc`
demo, which already asserts "draws differ" from a seeded pool. Don't couple one
component's success to another's scheduling.

## Proof
`rng: request rejected (no capability)` then `rng: served 32 bytes to
'rnguser'`, and `rnguser` exits 0 — capability-gated entropy reaching user
space. (The pool's live, distinct output is shown by note
[0018](0018-kernel-entropy-pool.md)'s `pqc` demo.)
