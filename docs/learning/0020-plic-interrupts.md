# 0020 — The first device interrupt (the PLIC)

**One-line:** the virtio-rng driver now *blocks* for its device's interrupt
instead of spinning a poll loop — the kernel routes the IRQ through the PLIC
and wakes it.

## What changed
- New `arch/riscv64/src/plic.rs`: drive the PLIC (claim/complete, per-context
  enable/threshold) for hart 0's S-mode context. The PLIC is mapped into every
  address space (the interrupt fires while a user task's `satp` is active).
- `Cause::SupervisorExternal` + a handler that claims the IRQ and wakes the
  bound task. `sie.SEIE` is enabled at boot.
- New `Capability::Interrupt(irq)` + a `wait_irq(cap)` syscall: the component
  blocks (`WaitingIrq`); the handler wakes it.
- The entropy component's used-ring poll loop becomes `wait_irq` + a device ack.

## The ideas worth keeping
1. **An interrupt controller is just MMIO: claim, then complete.** The PLIC
   tells you the highest-priority pending IRQ (claim) and you acknowledge it
   (complete); per-context enable bits and a threshold decide what reaches you.
2. **A kernel/userspace interrupt split must mask on deliver, unmask on
   ack.** The virtio line is level-triggered and stays asserted until the
   *driver* (in U-mode) acks the device — asynchronously. If the kernel
   completed immediately, it would re-fire in a storm. So the kernel **claims
   but does not complete**: claiming puts the source "in service" (the PLIC
   won't re-deliver it). The driver acks the device, then `wait_irq` completes
   the previous claim — re-arming the source for the next interrupt.
3. **A capability gates "wait for my IRQ" too.** Same unforgeable-index check
   as IPC/getrandom — the component owns its device *and* its interrupt.

## The debugging lesson (cost most of the phase)
The first design *unmasked* the IRQ (set its PLIC enable bit) inside `wait_irq`
and *masked* it (cleared enable) in the handler. It hung: the device drew and
asserted **before** `wait_irq` ran, so the source's enable bit was set *after*
the line was already high — and **QEMU's PLIC only asserts `SEIP` on the
rising edge of an *enabled* source.** The PLIC showed the source pending,
enabled, and above threshold (`pend=0x100, en=0x100, thr=0, prio=1`), yet
`sip.SEIP` stayed `0` and no trap fired. The spike had only *polled* the claim
register (readable from S-mode regardless), so it never exercised SEIP
delivery. The fix: keep the source **always enabled** and mask with claim's
in-service state instead of the enable bit — so the source is never enabled
after it has already asserted.

## Why a spike first (and its limit)
The spike verified the PLIC base, register layout, S-mode context (1), and the
RNG's IRQ (8) — invaluable. But it polled rather than taking the interrupt, so
it missed the SEIP-edge behavior. Lesson: a spike that polls a controller does
not prove *delivery*.

## Proof
`irq: external IRQ 8 woke 'entropy'`, then the pool is seeded and ML-KEM keyed
as before — the entropy now arrives by interrupt, not by polling.
