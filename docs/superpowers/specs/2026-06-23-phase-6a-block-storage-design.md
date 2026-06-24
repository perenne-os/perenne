# Kernel — Phase 6a Design: block storage (virtio-blk)

- **Date:** 2026-06-23
- **Status:** Draft — awaiting review
- **Scope of this document:** Phase 6a only — a user-space virtio-blk driver
  component that reads and writes disk sectors over a virtqueue, reusing the
  virtio transport and the PLIC interrupt path. The first sub-phase of the
  Phase 6 arc (persistent storage & the living knowledge base). Fully
  QEMU-testable (needs a `-drive` + `virtio-blk-device`).

---

## 0. Where this sits

Phase 6 (chosen 2026-06-23) makes the self-healing knowledge organism **real**
— it reads its actual knowledge base from persistent storage instead of the
compiled-in `KB-0005` stub. The arc is **6a block storage → 6b a minimal
filesystem → 6c the living knowledge base**. 6a is the device foundation: a
block driver the filesystem (6b) will call.

It reuses everything built for virtio-rng and the PLIC: the virtio-mmio
handshake + split virtqueue, the `wait_irq` capability/syscall, the
identity-mapped DMA frame, and the `find_device` probe. The difference from
virtio-rng is the **request shape** (a 3-descriptor chain).

**Spike-verified facts** (kernel-side bring-up, this QEMU):
- The block device is **DeviceID 2**; the handshake is identical to virtio-rng
  (accept only `VIRTIO_F_VERSION_1`; reset → ACK → DRIVER → features →
  FEATURES_OK → queue 0 → DRIVER_OK).
- Each I/O is a **3-descriptor chain**: desc 0 → a 16-byte header
  `{ type: u32, reserved: u32, sector: u64 }` (flags `NEXT`, next=1); desc 1 →
  the 512-byte data buffer (flags `NEXT`, plus `WRITE` if it is a *read* since
  the device fills it; next=2); desc 2 → a 1-byte status (flags `WRITE`,
  next=0). `type` = 1 write (`T_OUT`), 0 read (`T_IN`); status 0 = OK.
- DMA frame layout used (one 4 KiB frame): desc@0, avail@128, used@160,
  header@256, data@512, status@1024. The avail ring publishes the head
  descriptor (0); `avail.idx` increments per request.
- A write then a (buffer-zeroed) read returns the written pattern — writes
  persist. Probing by DeviceID is slot-independent (rng + blk can coexist).

## 1. Goal

A user-space `blk` component owns the virtio-blk device (its MMIO + a DMA frame
mapped only into it) and performs sector reads and writes over a virtqueue,
blocking on its IRQ via `wait_irq`. Because the DMA frame is identity-mapped,
the kernel reads/writes the sector data **directly in that physical page** — the
basis for the filesystem (6b) to consume the driver with no extra plumbing. 6a
proves the driver with a self-test round-trip.

**You learn (kept brief):** how a block device differs from a stream device —
a request is a *chain* of descriptors (header → data → status), and the data
descriptor's `WRITE` flag flips with the direction; and how the same virtqueue
+ `wait_irq` machinery serves a very different device.

**Done when** `./tools/test-qemu.ps1` (booting with a scratch `-drive` +
`virtio-blk-device`) observes, alongside every existing milestone:

1. **A sector round-trip** — the `blk` component writes a known pattern to
   sector 0, reads it back (after zeroing its buffer), and confirms the pattern
   was restored; the kernel logs `blk: sector round-trip ok`.

And off the bare target:

2. **Host unit tests** — `find_device`/`is_blk` (DeviceID match) and the blk
   DMA-layout offsets.

## 2. Non-goals (deferred)

- **A filesystem** — 6b. 6a is the raw block device; the only "consumer" is a
  self-test plus a kernel result line.
- **An IPC service for block I/O** — the `call`-driven "read sector N for the
  FS" interface is 6b's concern (when a kernel FS actually calls the component).
  6a's component self-tests and reports a one-way result.
- **Multi-sector / scatter-gather / async queues** — one 512-byte sector per
  request, one request at a time, queue size 8 (only one descriptor chain used).
- **Reading the device's capacity / config space** — 6a reads/writes sector 0
  directly; the FS will read geometry later if needed.
- **A kernel-side block layer** — the driver stays in user space (ADR 0007); the
  kernel reads the result from the identity-mapped DMA page.

## 3. Design

### 3.1 Components

| Component | Location | Responsibility |
|-----------|----------|----------------|
| blk constants + `find_device` | `arch/riscv64/src/virtio.rs` | `DEVICE_ID_BLK`=2, request type/flag/sector constants, blk DMA offsets; generalize `find_rng` → `find_device(bases, id)` (pure layout host-tested). |
| The `blk` driver | `kernel/src/main.rs` | A U-mode component: virtio-blk handshake + queue, a 3-descriptor read/write, `wait_irq`, the self-test, the result report. |
| Discovery + grant | `kernel/src/main.rs` `kmain` | Probe DeviceID 2, allocate a DMA frame, map MMIO + DMA into the component, grant `Endpoint`(report) + `Interrupt`(blk IRQ), set launch args, PLIC-enable the blk IRQ. |
| Result consumer | `kernel/src/main.rs` | A kernel task that `recv`s the blk result and prints `blk: sector round-trip ok`. |

### 3.2 The blk constants (`virtio.rs`)

Add (alongside the existing virtio constants):

```
pub const DEVICE_ID_BLK: u32 = 2;
pub const VIRTQ_DESC_F_NEXT: u16 = 1;   // (VIRTQ_DESC_F_WRITE = 2 already exists)
pub const BLK_T_IN: u32 = 0;            // read (device writes the data buffer)
pub const BLK_T_OUT: u32 = 1;           // write (device reads the data buffer)
pub const BLK_SECTOR_SIZE: usize = 512;
// blk request buffers within the DMA frame (after the rings):
pub const BLK_HDR_OFF: usize = 256;     // 16-byte header
pub const BLK_DATA_OFF: usize = 512;    // 512-byte sector data
pub const BLK_STATUS_OFF: usize = 1024; // 1-byte status
```

Generalize the probe so both devices share it:

```
pub unsafe fn find_device(bases: &[usize], device_id: u32) -> Option<usize> { … }
```

`kmain`'s existing RNG probe becomes `find_device(bases, DEVICE_ID_RNG)`.

### 3.3 The `blk` component (the new part: a 3-descriptor request)

Receives `a1 = mmio`, `a2 = dma` at launch (like the entropy component). It
reuses the inline-asm MMIO/DMA helpers. After the standard handshake + queue-0
setup, a request (`write: bool`, `avail_idx`) builds the chain in the DMA
frame:

```
header @ BLK_HDR_OFF:  type = if write { BLK_T_OUT } else { BLK_T_IN }; reserved = 0; sector = 0
desc 0: addr = hdr,    len = 16,  flags = NEXT,                         next = 1
desc 1: addr = data,   len = 512, flags = NEXT | (if read { WRITE }),   next = 2
desc 2: addr = status, len = 1,   flags = WRITE,                        next = 0
avail.ring[avail_idx % QSIZE] = 0; fence; avail.idx = avail_idx + 1; fence
notify queue 0; wait_irq(IRQ_CAP); ack the device (InterruptStatus → InterruptACK)
status byte (0 = OK)
```

**Self-test:** fill the data buffer with `byte[i] = i & 0xff` (a loop of
inline-asm byte stores — codegen-safe, no `.rodata`); issue a **write** of
sector 0; zero the data buffer; issue a **read** of sector 0; verify the buffer
holds the pattern again (loop compare). Report success/failure with a one-way
`send` (badge 1 = ok / 0 = fail) to the result consumer, then `exit`. Two I/Os,
two interrupts — both via `wait_irq`, exactly like the entropy component's draw
loop.

### 3.4 Discovery, mapping, and the result consumer

`kmain`, mirroring the entropy/virtio-rng wiring:
- `find_device(bases, DEVICE_ID_BLK)` (early, MMU off) → blk base.
- If present: allocate a zeroed DMA frame; `build_virtio_space(stack, mmio,
  dma)`; grant `Endpoint(BLK_EP)` (cap slot 0, for the report) and
  `Interrupt(blk_irq)` (cap slot 1, for `wait_irq`); `set_launch_args(mmio,
  dma)`; `plic::init` + `set_priority(blk_irq, 1)` + `enable(blk_irq)` +
  `sie_enable_external` (the PLIC setup is idempotent if rng already did it).
- Spawn the result consumer (a kernel task) before the component, holding
  `Endpoint(BLK_EP)`; it `recv`s the result and prints `blk: sector round-trip
  ok` (or a failure line).

### 3.5 Error handling summary

| Situation | Behavior |
|-----------|----------|
| No virtio-blk device (smoke without `-drive`) | Component not spawned; `kmain` logs `blk: no virtio-blk device found`; boot continues. |
| A request's status byte ≠ 0 (device I/O error) | The component reports failure (badge 0); the consumer prints `blk: sector round-trip FAILED`. |
| The read-back pattern mismatches | Same — reported as failure. |
| Device read-only (`-drive` is `readonly`) | The write fails (status ≠ 0) → reported failure; the smoke uses a writable scratch image. |
| Component touches memory it doesn't own | Contained + diagnosed as any U-mode fault (5a). |

## 4. Testing

- **Host unit tests** (`arch/riscv64`): `is_blk(2)` true / others false (or
  `find_device` device-id matching); the blk DMA offsets are ordered and within
  the 4 KiB frame (`BLK_HDR_OFF` after the used ring; `BLK_DATA_OFF +
  BLK_SECTOR_SIZE ≤ BLK_STATUS_OFF`; `BLK_STATUS_OFF + 1 ≤ 4096`).
- **QEMU smoke test** (`tools/test-qemu.ps1`, extended; written first,
  failing): create a scratch raw disk image, add `-drive
  file=<img>,if=none,format=raw,id=blk0 -device virtio-blk-device,drive=blk0`,
  and assert `blk: sector round-trip ok`; keep every other milestone (rng and
  blk coexist — each component finds its device by DeviceID).
- **Planning spike (done):** verified the 3-descriptor chain, header/status
  format, DMA offsets, and write-persistence against QEMU's virtio-blk.

## 5. Deliverables

1. `virtio.rs`: the blk constants + `find_device` (generalized) + host tests.
2. `kernel/src/main.rs`: the `blk` component (handshake + 3-desc I/O + self-test
   + report), `sys` helpers as needed, the discovery/mapping/grant/PLIC wiring,
   and the result consumer; `MAX_TASKS` bump if needed.
3. `tools/test-qemu.ps1`: the scratch disk + virtio-blk QEMU args + the
   assertion.
4. Host tests + smoke, all green.
5. Short learning note `docs/learning/0022-block-storage.md`.
6. Roadmap: Phase 6a marked done.
7. Glossary: only genuinely new terms (block device / sector, virtio-blk).

## 6. Open questions (for 6b / later)

- **The block-I/O IPC service** — a `call`-driven "read/write sector N"
  interface the kernel filesystem invokes (and a kernel-side `call`), reading
  the result from the identity-mapped DMA page. (6b.)
- **A block cache** and multi-sector requests once the FS needs throughput.
- **Device capacity / geometry** from the virtio-blk config space.
