# Kernel — Design: a virtio-rng entropy component

- **Date:** 2026-06-20
- **Status:** Draft — awaiting review
- **Scope of this document:** a single user-space component that drives the
  QEMU **virtio-rng** device to obtain real hardware entropy, and the wiring
  that uses that entropy to seed ML-KEM — retiring Phase 3c's fixed seed.
  Fully QEMU-testable (needs `-device virtio-rng-device`).

---

## 0. Where this sits

The capability microkernel's payoff (ADR 0007): drivers live *outside* the
kernel as capability-holding user-space components. The RTC driver proved a
**simple** component (one MMIO read). This is the **complex** follow-up the
roadmap names — a real virtio device with a virtqueue and DMA — and it also
delivers the security-differentiator payoff: real entropy replacing Phase
3c's fixed ML-KEM seed (`PQC_DEMO_SEED = [0x3c; 32]`).

It builds on: per-address-space isolation (3b-ii), capability-checked IPC
(3b-iii), the RTC component's device-mapping pattern (ADR 0007), and the
launch-argument mechanism introduced by 5b (the kernel hands a task a value in
`a0` at launch; here generalized to a few registers).

## 1. Goal

A user-space `entropy` component owns the virtio-rng device — its MMIO and a
DMA region mapped only into it — drives the virtio-mmio handshake and a single
virtqueue to draw random bytes, and delivers 32 bytes of real entropy over IPC
to seed ML-KEM. The kernel only *discovers* the device and grants the
component its mapped memory; it never drives the device.

**You learn (kept brief):** what a virtio device actually is — the virtio-mmio
register handshake, a split virtqueue (descriptor table + available/used
rings) as driver↔device shared DMA memory, and why an identity-mapped guest
makes DMA addressing trivial; and how a non-trivial driver still fits the
user-space-component model (the kernel grants mapped MMIO + a DMA frame as
capabilities; the component does the rest unprivileged).

**Done when** `./tools/test-qemu.ps1` (booting with `-device
virtio-rng-device`) observes, alongside every existing milestone:

1. **Live entropy** — the `entropy` component draws two 32-byte samples from
   virtio-rng, confirms they differ (a stuck/fixed source would repeat), and
   the kernel logs `entropy: virtio-rng live (two draws differ)`.
2. **Entropy-seeded PQC** — a kernel consumer receives the 32-byte seed and
   runs the ML-KEM-768 round-trip on it, logging `pqc: ML-KEM-768 round-trip
   ok (entropy-seeded)`. The fixed-seed early `pqc_demo` is removed.

And off the bare target:

3. **Host unit tests** for the new pure helpers (device-tree virtio
   discovery; the virtqueue layout math).

## 2. Non-goals (deferred)

- **Interrupts / the PLIC** — the component **polls** the used ring; no virtio
  interrupt handling. (A PLIC + interrupt-driven IO is a separate future
  phase.)
- **A general virtio stack** — only virtio-mmio + virtio-rng, only queue 0,
  only modern (Version 2). No virtio-blk/net, no indirect/packed queues, no
  multi-queue.
- **A kernel CSPRNG / entropy pool** — the component delivers raw device
  entropy straight into one ML-KEM seed. Pooling/whitening/reseeding is later.
- **Restarting/​healing the entropy component** — it is not a 5b patient (no
  Restart capability granted); its `relaunch` info exists but no healer drives
  it.
- **Making the entropy component restart-safe against device half-states** —
  one-shot at boot; it draws, delivers, and exits.
- **Removing kernel-side ML-KEM** — ML-KEM stays in the kernel (it cannot run
  in U-mode); only its *seed* now comes from the device. Moving crypto into a
  component is a much larger, separate effort.

## 3. Design

### 3.1 Components

| Component | Location | Responsibility |
|-----------|----------|----------------|
| virtio discovery | `arch/riscv64/src/dt.rs` | Collect the `virtio,mmio` node bases (QEMU `virt` has 8) into `MachineInfo` (fixed array + count). Pure, host-tested. |
| RNG probe | `arch/riscv64/src/virtio.rs` *(new)* | Pure helpers: identify the RNG slot by DeviceID, and compute the split-virtqueue layout (offsets/addresses within the DMA frame). Host-tested. The gated MMIO probe wrapper lives here too. |
| Memory grant | `arch/riscv64/src/mem.rs` | Map the MMIO page **and** a DMA page **RW-U, identity** into the component (a small extension of the RTC's single R-U device mapping). |
| Launch args | `arch/riscv64/src/sched.rs`, `task.rs` | Generalize 5b's `a0`-generation: a task may also receive `a1`/`a2` at launch. `set_launch_args(slot, a1, a2)` writes the forged context (and records them for restart); `user_trampoline` adds `mv a1,s4; mv a2,s5`. |
| The driver | `kernel/src/main.rs` | The `entropy` U-mode component: virtio-mmio handshake + one virtqueue, two draws, compare, IPC the seed. All inline-asm (codegen wall). |
| The consumer | `kernel/src/main.rs` | A kernel (S-mode) task: `recv` 32 bytes, call `kernel_crypto::ml_kem768_agree(seed)`, print the result. |

### 3.2 Discovery and the kernel grant

QEMU `virt` always lists 8 `virtio,mmio` nodes (e.g. `0x1000_1000` ..
`0x1000_8000`, 0x1000 apart); `-device virtio-rng-device` attaches the RNG to
one. `dt::parse` collects their `reg` bases into `MachineInfo.virtio_mmio:
[usize; 8]` + `virtio_mmio_count`. Early in `kmain` (MMU off, like reading the
DTB) the kernel reads each base's **DeviceID** register (offset `0x008`) and
remembers the base whose DeviceID is `4` (entropy). If none, it logs `entropy:
no virtio-rng device found` and skips the component (boot continues — see
§3.6).

For the component the kernel allocates one **zeroed, identity-mapped** DMA
frame (`frame::alloc_zeroed()`), and maps into the component's address space:
its U-mode stack (RW-U, as today), the **RNG MMIO page (RW-U, identity)**, and
the **DMA frame (RW-U, identity)**. Identity mapping (VA=PA) is required: the
device DMAs to the physical addresses the component writes into descriptors,
and with VA=PA the component can use the same value for both. Zeroing means
the component never needs `memset` for the rings.

The two base addresses are handed to the component as **launch arguments**:
generalizing 5b (which set `a0 = generation` via `s3`), `set_launch_args`
stores `a1 = mmio_base` and `a2 = dma_base` in the forged context (`s4`,
`s5`), and `user_trampoline` loads them (`mv a1,s4; mv a2,s5`) before `sret`.
Tasks that set no args get `0` (harmless). The args are recorded in `Relaunch`
so a future restart re-forges them identically.

### 3.3 The driver (user-space, modern virtio-mmio v2)

All register and DMA access is inline asm (no kernel `.text`/`.rodata`). The
component receives `a1 = mmio_base`, `a2 = dma_base`.

**Handshake** (virtio-mmio register offsets): verify `MagicValue` (0x000 =
"virt") and `Version` (0x004 = 2) and `DeviceID` (0x008 = 4); then drive
`Status` (0x070): `0` (reset) → `ACKNOWLEDGE(1)` → `DRIVER(2)`; read
`DeviceFeatures` (0x010, sel 0x014), accept only `VIRTIO_F_VERSION_1` (bit 32:
sel=1, bit 0), write `DriverFeatures` (0x020, sel 0x024); set `FEATURES_OK(8)`
and read `Status` back to confirm it stuck.

**Queue 0 setup:** write `QueueSel(0x030)=0`, read `QueueNumMax(0x034)`, write
`QueueNum(0x038)=QSIZE` (a small power of two, e.g. 8); write the ring
physical addresses split across low/high registers — `QueueDesc(0x080/0x084)`,
`QueueDriver(0x090/0x094)` (avail), `QueueDevice(0x0a0/0x0a4)` (used) — from
the layout in §3.4; write `QueueReady(0x044)=1`. Finally `Status |=
DRIVER_OK(4)`.

**A draw** (×2, reusing descriptor 0): write descriptor 0 = `{ addr =
buf_pa, len = 32, flags = VIRTQ_DESC_F_WRITE(2), next = 0 }`; publish index 0
in the avail ring (`avail.ring[avail.idx % QSIZE] = 0`; `fence`; `avail.idx +=
1`); write `QueueNotify(0x050)=0`; **poll** `used.idx` until it advances
(`fence` around the read). The device has written 32 random bytes into
`buf_pa`. Use two buffers (`buf0`, `buf1`) in the DMA frame.

**Compare + report:** loop-compare the two 32-byte buffers (loads + branch,
codegen-safe). Then deliver `buf0` as the seed over IPC: load the 4 words and
`send(EP, badge=word0, data=[word1,word2,word3])` — 32 bytes in one
register-only message — with the badge low bit / a data flag indicating
"draws differed" vs "identical". The component then `exit`s.

### 3.4 The virtqueue layout (pure, host-tested)

For queue size `QSIZE = 8`, within the zeroed DMA frame (offsets, all within
4 KiB, each ring at its required alignment — desc 16, avail 2, used 4):

| Region | Offset | Size |
|--------|--------|------|
| Descriptor table (`QSIZE × 16`) | 0 | 128 |
| Available ring (`4 + 2 + 2×QSIZE`) | 128 | 22 |
| Used ring (`4 + 2 + 8×QSIZE`) | 160 (16-aligned) | 70 |
| Entropy buffer 0 | 256 | 32 |
| Entropy buffer 1 | 288 | 32 |

A pure `fn vq_layout(dma_base: usize, qsize: usize) -> VqLayout` returns the
absolute addresses (`desc`, `avail`, `used`, `buf0`, `buf1`); host-tested for
the offsets/alignment. The component and the kernel agree via this one
function (the kernel needs only the frame; the component computes addresses
from `a2`).

### 3.5 Entropy → ML-KEM

The component sends 32 bytes to endpoint `ENTROPY_EP`; a **kernel consumer
task** (S-mode, full privilege — may call `kernel_crypto`) holds the
receive capability, `recv`s the 4 words, reconstructs `[u8; 32]`, and calls
`kernel_crypto::ml_kem768_agree(seed)`. On success it prints `pqc: ML-KEM-768
round-trip ok (entropy-seeded)`; it also prints `entropy: virtio-rng live (two
draws differ)` (or `entropy: WARNING two draws identical` if the flag says so).
The early fixed-seed `pqc_demo()` in `kmain` is removed; ML-KEM is now seeded
by the device.

Ordering: the `entropy` component and the `pqc_consumer` are spawned so the
consumer `recv`-blocks before the component sends (consumer in an earlier
slot, like the RTC server). `MAX_TASKS` rises from 8 to accommodate the two
new tasks alongside the existing cast.

### 3.6 Error handling summary

| Situation | Behavior |
|-----------|----------|
| No virtio-rng device (smoke run without `-device`) | Kernel logs `entropy: no virtio-rng device found`, skips the component + consumer; boot and all other milestones continue. |
| `FEATURES_OK`/`Version`/`DeviceID` check fails | Component logs via exit code (kernel formats) and exits; consumer is left waiting (system still runs via idle). |
| Used ring never advances (device error) | The component polls a bounded number of spins, then exits with an error code (no infinite hang); kernel notes it. |
| Two draws identical (suspected stuck source) | Still seeds ML-KEM (any 32 bytes work) but the consumer logs the WARNING form — the honest negative result. |
| Component touches memory it doesn't own | Contained + diagnosed exactly as any U-mode fault (5a) — and *not* a 5b patient, so just contained. |

## 4. Testing

- **Host unit tests** (`arch/riscv64`, `cargo test`):
  - `dt::parse` collects the virtio-mmio bases (extend the fixture assertion:
    the QEMU `virt` DTB lists 8, at the expected bases).
  - `virtio::vq_layout` returns the documented offsets/addresses and respects
    desc/avail/used alignment for `dma_base` values.
  - `virtio::is_rng(device_id)` / the DeviceID match helper.
- **QEMU smoke test** (`tools/test-qemu.ps1`, extended; written first,
  failing): add `-device virtio-rng-device` to the QEMU args, and assert
  `entropy: virtio-rng live (two draws differ)` and `pqc: ML-KEM-768
  round-trip ok (entropy-seeded)`; keep every existing milestone. (The old
  `pqc: ML-KEM-768 round-trip ok (shared secret agreed)` line is replaced.)
- **Planning spike (de-risk the codegen wall):** before writing the plan,
  spike the virtio-mmio handshake + a single draw as a throwaway in the
  component, run it under QEMU to confirm the U-mode inline-asm driver works
  (and the modern-v2 register offsets/feature bits are right against this QEMU
  version), then revert — the project's established spike-verify pattern (used
  for the FDT parser and the RTC).

## 5. Deliverables

1. `dt.rs`: virtio-mmio base collection in `MachineInfo`; host tests.
2. `virtio.rs` (new): `is_rng`/DeviceID match, `vq_layout` (pure, host-tested),
   and the gated MMIO probe wrapper; module declared in `lib.rs`.
3. `mem.rs`: map MMIO + DMA pages RW-U (identity) into the component.
4. `task.rs`/`sched.rs`: launch-args generalization (`a1`/`a2` via `s4`/`s5`),
   `set_launch_args`, `Relaunch` carrying the args; `user_trampoline` update.
5. Early-boot RNG probe in `kmain`; the DMA frame alloc + component address
   space.
6. `kernel/src/main.rs`: the `entropy` driver component and the `pqc_consumer`
   kernel task; cast/`MAX_TASKS` update; remove the fixed-seed `pqc_demo`.
7. Extended QEMU smoke test (`-device virtio-rng-device`) + host tests, green.
8. Short learning note `docs/learning/0016-virtio-rng-entropy.md`.
9. Roadmap: the entropy component marked done; the 3c fixed-seed limitation
   retired in the notes.
10. Glossary: only genuinely new terms (virtio, virtqueue, DMA, MMIO if not
    present).

## 6. Open questions (for later phases)

- **A kernel entropy pool / CSPRNG** seeded by this source, reseeded over
  time, exposed as a capability-gated service to U-mode.
- **Interrupt-driven IO** (a PLIC driver) so devices need not be polled.
- **Moving ML-KEM itself into a user-space crypto component** (needs a U-mode
  runtime: heap/`memcpy`/`.rodata` for components — a substantial enabler).
- **A general virtio transport** reused by future devices (blk/net).
- **Restart-safety** so the self-healer (5b) could recover the entropy
  component after a device half-state.
