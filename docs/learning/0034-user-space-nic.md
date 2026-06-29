# 0034 — The NIC becomes a user-space component

**One-line:** the virtio-net driver moves out of the kernel into an unprivileged
U-mode component (ADR 0007), joining `rng` and `blk` — so every device the OS
drives is now a capability-bounded component the kernel never touches the
registers of. Same proof as Phase 15: `net: resolved 10.0.2.2 -> 52:55:0a:00:02:02`.

## What changed
- A U-mode `net_component` (`.user_text`) owns the NIC: its MMIO + DMA frame are
  mapped RW-U into it **alone**. It does the two-queue bring-up, pre-posts an RX
  buffer, and serves one call/reply ARP exchange, blocking on the device IRQ
  (`wait_irq`) like `blk`/`rng`.
- A kernel `net_resolver` task builds the ARP request into the shared,
  identity-mapped DMA page (the host-tested `kernel_common::net`), `call`s the
  driver, parses the reply, and prints the gateway MAC.
- Removed the in-kernel `net_resolve_gateway` and `mem::map_device` (both existed
  only for Phase 15's kernel-side driver).

## The idea worth keeping: the driver does mechanics, the kernel does logic
A U-mode component lives in `.user_text` and **cannot call kernel `.text`** — so
it can't reach the host-tested ARP logic. The `blk` model resolves this: the
**driver does only raw device mechanics** (bring-up, transmit, receive) over a
shared DMA page; the **higher-level logic stays kernel-side** in the client that
fills/reads that page. So `net_resolver` keeps using the pure `kernel_common::net`
unchanged — no ARP bytes hand-written in U-mode (the alternative learning note
0033 anticipated). One frame the kernel writes, one the kernel reads; the driver
only moves bytes to and from the wire.

## Two U-mode gotchas the boot test surfaced
1. **No iterators in `.user_text`.** A `for .. in [array]` (queue setup) and a
   `for _ in 0..16` (RX wait) compile to `IntoIterator`/`Range::next` calls that a
   debug build may not inline → calls into kernel `.text` the U-mode task can't
   fetch → `InstructionPageFault`. Use an `#[inline(always)]` helper and a manual
   `while` counter, as `entropy`/`blk` already do.
2. **A one-shot interrupt driver should exit, not idle.** `wait_irq` *completes*
   the previous PLIC claim; the handler claims-but-doesn't-complete so the source
   stays in-service (masked) until the driver acks + waits again. A driver that
   does one exchange then loops on `recv` leaves its last claim in-service
   forever. So `net_component` does its exchange then `sys_exit(0)` — the same
   bounded-work pattern as the `entropy` component.

## A note on timing
Moving the NIC into the scheduler means its early ARP exchange competes in the
round-robin and shifts the self-healing demo a few ticks later, so the per-boot
smoke deadline rose 60s → 90s. The work is unchanged and still completes (blk
reads recover on the timer tick, per [note 0023](0023-minimal-filesystem.md)); the
deadline only bounds the failure case.

## Proof
`ipc: 'net' blocks on recv` → `net: resolved 10.0.2.2 -> 52:55:0a:00:02:02` →
`sched: task 'net' exited (code 0)` — the gateway MAC, learned by an unprivileged
driver, with the rest of the system (including the cross-boot KB write-back)
running on.

## What's next
A minimal IP/UDP stack (DHCP, ping) on this driver; receiving unsolicited frames
(RX as an ongoing service, not a one-shot); and encrypting traffic with the
Phase 14 channel.
