# 0015 — Self-healing, step two: the caged fix (Phase 5b)

**One-line:** the self-healer now *acts* — an isolated user-space component
restarts a crashed component — and the kernel is the cage that keeps that
power bounded.

## What changed
- A `healer` U-mode component blocks on a reserved crash endpoint. When the
  kernel contains and diagnoses a crash (5a), `exit_current` notifies the
  healer (reusing the IPC rendezvous), delivering a badge that is the cap
  index of the crashed component's `Restart` capability.
- A new `restart(cap_idx)` syscall: the kernel capability-checks it, enforces
  a per-task retry bound (`MAX_RESTARTS`), re-forges the task's first-run
  context (its address space, stacks, and data persist), and logs.
- A `transient` patient crashes once and recovers; the always-crashing
  `flaky` is restarted only up to the bound, then flagged for triage.

## The two ideas worth keeping
1. **Isolation + a capability cage make an acting agent safe.** The agent
   (healer) runs unprivileged in user space and can do only what a capability
   grants. The *kernel* enforces the bound and logs — so even a buggy or
   compromised healer cannot restart-loop. Agency in user space; enforcement
   in the kernel (ADR 0005's safety cage).
2. **A restart is just re-forging the first-run context.** Nothing is
   reaped or re-allocated — the slot, `satp`, stacks, and data page persist.
   To let a transient fault differ from a permanent one, the kernel hands the
   task its **launch generation** in `a0` (`user_trampoline` does `mv a0, s3`):
   generation 0 = first run, >0 = a restart. The transient patient crashes
   only on generation 0.

## A scheduling subtlety worth remembering
The healer's inbox holds one message. With both patients runnable at once,
plain round-robin could run the second patient — overwriting the first crash
notification — before the healer ran. So a crash that wakes the healer makes
`exit_current` run the healer *next*, draining each crash before another can
occur. (A single-slot inbox is fine once delivery is paired with immediate
hand-off.)

## Why act only after diagnosis (5a before 5b)
Recognize and explain a problem deterministically *before* anything gains the
authority to change the system in response — and then confine that authority.

## Proof
`transient` crashes (LoadPageFault) → diagnosed (KB-0005) → `heal: restarted
'transient' (attempt 1)` → `transient` exits 0 (recovered). `flaky` crashes
repeatedly → restarted to the bound → `heal: giving up on 'flaky' after 2
restarts (flagged for triage)`. The kernel, the RTC component, and the
heartbeat run throughout.
