# 0022 — Block storage (virtio-blk)

**One-line:** the OS can read and write disk sectors — a user-space virtio-blk
driver, the foundation for a filesystem and a real, on-disk knowledge base.

## What changed
- A `blk` U-mode component drives the QEMU virtio-blk device (its MMIO + a DMA
  frame mapped only into it), reusing the virtio handshake, the split
  virtqueue, and `wait_irq` from the entropy/PLIC work.
- `find_rng` generalized to `find_device(bases, id)` — both the rng (id 4) and
  the block device (id 2) are found by DeviceID, so they coexist on any slots.
- The component self-tests a sector round-trip (write a pattern, read it back,
  verify) and reports; a kernel consumer prints `blk: sector round-trip ok`.

## The idea worth keeping
**A block request is a *chain* of descriptors, not one buffer.** Where the rng
used a single device-writable descriptor, a block I/O is three linked
descriptors: a header `{type, reserved, sector}` the device reads, a 512-byte
data buffer, and a 1-byte status the device writes. The data descriptor's
`WRITE` flag flips with direction — set for a read (the device fills it), clear
for a write (the device reads it). The same virtqueue notify/`wait_irq`
machinery carries it.

## Why this is step one of a real knowledge base
The self-healer's knowledge base is still a compiled-in stub (`KB-0005`).
Phase 6 makes it real: 6a is the disk, 6b a filesystem over it, 6c the healer
reading its actual `knowledge-base/` from disk. Because the DMA frame is
identity-mapped, the kernel filesystem (6b) will read sector data straight from
that physical page — no extra plumbing.

## Proof
`blk: sector round-trip ok` — a pattern written to sector 0 and read back
intact, interrupt-driven, through an unprivileged driver.
