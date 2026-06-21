# 0016 — A virtio device in user space: the entropy component

**One-line:** a real virtio-rng driver runs as an unprivileged component and
feeds genuine hardware entropy into ML-KEM — retiring Phase 3c's fixed seed.

## What changed
- The kernel discovers the RNG (probes the virtio-mmio slots for DeviceID 4),
  allocates a zeroed identity-mapped DMA frame, maps the MMIO + DMA pages RW-U
  into the `entropy` component, and hands it their bases in launch registers
  (`a1`/`a2`, generalizing 5b's `a0`).
- The component does the whole modern virtio-mmio v2 bring-up itself, in
  inline asm: status/feature handshake, one split virtqueue, notify, poll the
  used ring. It draws 32 random bytes twice and sends each over IPC.
- A kernel `pqc` consumer task receives the two draws, confirms they differ (a
  live source), and runs ML-KEM-768 seeded by real entropy.

## The ideas worth keeping
1. **A virtqueue is just shared DMA memory + four rules.** A descriptor table
   (buffers), an available ring (driver → device), a used ring (device →
   driver), and a notify register; fences order the ring writes. Polling the
   used ring avoids needing interrupts.
2. **Identity mapping makes DMA trivial.** The device DMAs to *physical*
   addresses; with VA=PA the component writes the same value into a descriptor
   that it uses as a pointer. The kernel maps the DMA frame identity, RW-U,
   into the component only — the device is the component's capability.
3. **A complex driver still fits user space.** The kernel grants mapped MMIO +
   a DMA frame; the component does everything else unprivileged. The cost is
   the U-mode codegen wall — every access is inline asm, and constants come
   from `virtio::*` (folded to immediates, never a `.rodata` read).

## Two debugging lessons (both cost a smoke iteration)
- **QEMU defaults to *legacy* virtio-mmio.** The device reported Version 1 and
  ignored the modern queue registers (the used ring never advanced) until we
  added `-global virtio-mmio.force-legacy=false` for Version 2.
- **ML-KEM needs a big stack.** The consumer first hung running the round-trip
  on a 16 KiB task stack — the same overflow Phase 3c hit on the 64 KiB boot
  stack. Its kernel task gets a dedicated 512 KiB stack.

## Why a spike first
virtio is unforgiving (wrong register/version/feature → silent no-op). A
throwaway kernel-side bring-up under QEMU verified the protocol, the offsets,
the legacy-vs-modern default, and that we actually get varying bytes — before
any of it was committed.

## Proof
`entropy: virtio-rng live (two draws differ)` then `pqc: ML-KEM-768 round-trip
ok (entropy-seeded)` — real device entropy, varying every boot, now keys the
post-quantum round-trip.
