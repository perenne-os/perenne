# Phase 16 — Move the NIC driver to a user-space component (design)

**Status:** approved 2026-06-28 (user authorized writing the spec and
implementing end-to-end)
**Priority served:** the microkernel soul (ADR 0007 — features and drivers live
*outside* the kernel as capability-holding components). Pays back the deviation
Phase 15 flagged: its virtio-net driver shipped **in the kernel** for pragmatism.
This phase relocates it to U-mode, joining `rng` and `blk` as the **third**
unprivileged virtio driver, before any IP/UDP stack grows on top.

## The gap

Phase 15 opened networking, but `net_resolve_gateway` runs in the kernel and
touches the NIC's MMIO registers directly (`mem::map_device(mmio)`). That breaks
the project's central guarantee — a driver is an *unprivileged* component bounded
by the capabilities it was granted, and the kernel never touches the device. The
`rng` and `blk` drivers already prove the pattern; the NIC is the outlier.

## Architecture (the `blk` model, applied to net)

U-mode components live in `.user_text` and **cannot call kernel `.text`** (no `U`
bit on kernel code), so the U-mode driver cannot reach the host-tested
`kernel_common::net` ARP logic. The established `blk` split resolves this: the
**U-mode driver does only raw device mechanics**; the **higher-level logic stays
in a kernel client** that shares the driver's DMA page. Two pieces replace the
in-kernel `net_resolve_gateway`:

### `net_component` (U-mode driver, ADR 0007)

`.user_text` + inline asm only. At launch it reads `a1 = mmio`, `a2 = dma` (the
`set_launch_args` convention), runs the modern two-queue virtio-net bring-up
(RX = queue 0, TX = queue 1; negotiate `VERSION_1`), and pre-posts one RX buffer.
Then it serves a **call/reply endpoint** loop:

1. `badge = sys_recv(NET_EP, NET_REPLY_SLOT)` — the badge is the TX frame length
   (including the 12-byte `virtio_net_hdr`).
2. Publish the TX descriptor (`len = badge`), notify queue 1.
3. **Block on the device IRQ** (`sys_wait_irq(NET_IRQ_CAP)`) until the **RX** used
   ring index advances. A TX-completion IRQ may wake it first; it acks
   `INTERRUPT_STATUS` and re-waits. Bounded by an internal wait cap so a genuine
   no-reply replies `0`.
4. `sys_reply(NET_REPLY_SLOT, rx_len)` — the received frame length (`0` = none).

It holds exactly two capabilities: `Endpoint(NET_EP)` and `Interrupt(net_irq)`.
Its MMIO + DMA pages are mapped RW-U into it **and nowhere else** — its device
"capability." It is the only task that touches the NIC registers; the kernel does
not.

### `net_resolver` (kernel client task)

Holds `Endpoint(NET_EP)`. It:

1. Builds the ARP request into the shared DMA TX buffer (after a zeroed 12-byte
   `virtio_net_hdr`) using the **host-tested `kernel_common::net::build_request`**.
2. `call_message(NET_EP, 12 + arp_len)` — hands the driver the total TX length and
   blocks until it replies with the RX length.
3. Parses the RX buffer (after its 12-byte header) with
   `kernel_common::net::parse_reply`, printing
   `net: resolved 10.0.2.2 -> 52:55:0a:00:02:02` (or `net: no ARP reply` on `0`).

All ARP logic stays kernel-side and pure — exactly as the `fs` client stayed
kernel-side over the `blk` driver.

### Shared DMA frame

A kernel-allocated, identity-mapped page (`NET_DMA_PA`, a kernel global like
`BLK_DMA_PA`). The resolver reaches it through the master identity map, the driver
through its RW-U identity map — **same physical frame, no copy**. The layout
reuses the Phase 15 spike offsets within one 4 KiB frame:

| Region | Offset |
|---|---|
| RX desc / avail / used | `0x000` / `0x080` / `0x100` |
| TX desc / avail / used | `0x200` / `0x280` / `0x300` |
| RX buffer | `0x400` |
| TX buffer | `0xC00` |

Frames are 12-byte `virtio_net_hdr` + ARP; the host-tested `net` logic operates on
the slice *after* the header (the resolver writes/reads at `buf + 12`).

## Data flow (the proof)

```
boot: find virtio-net (DEVICE_ID_NET=1)
   → spawn net_component  (mmio + dma mapped RW-U; NET_EP + Interrupt caps; launch args)
   → spawn net_resolver   (NET_EP cap)
   → PLIC: route net_irq, set priority, enable; sie external  (idempotent)

resolver: build_request → DMA TX buf[12..]
   → call(NET_EP, 12+arp_len) ─────────────► driver: publish TX desc; notify q1
                                                      wait_irq(net_irq)  ◄── RX completion
                                                      reply(rx_len)
   ◄──────────────────────────────────────── parse_reply(DMA RX buf[12..])
   → println!("net: resolved 10.0.2.2 -> 52:55:0a:00:02:02")
```

The smoke asserts that `net: resolved …` line — now produced by a U-mode driver
the kernel never touches the registers of.

## Components / changes

### Removed

- `net_resolve_gateway` and its `mem::map_device(mmio)` call from
  `kernel/src/main.rs` (the driver now owns the MMIO via `build_virtio_space`).
- `mem::map_device` if it becomes unused after the removal (it was added solely
  for the Phase 15 kernel-side spike).

### Added (`kernel/src/main.rs`)

- `net_component` (`#[link_section = ".user_text"]`) — bring-up + serve loop, in
  inline asm, reusing the existing `virtio.rs` constants and the `dma_w*` / `mmio_*`
  helpers already in `.user_text` (verify each helper it calls is `.user_text`;
  reuse the blk driver's helpers, which are).
- `net_resolver` (kernel `extern "C" fn`) — build/call/parse/print.
- Statics: a U-stack for the driver, K-stacks for driver + resolver, the
  `NET_DMA_PA` global, and `NET_EP` / `NET_CAP` / `NET_IRQ_CAP` / `NET_REPLY_SLOT`
  consts.
- A `kmain` block behind `if let Some(net) = net_base { … } else { println!("net:
  no virtio-net device found") }`, mirroring the `blk` block: spawn the driver with
  `build_virtio_space(u, (net, net+0x1000), (dma_pa, dma_pa+0x1000))`, grant
  `Endpoint(NET_EP)` + `Interrupt(net_irq)`, `set_launch_args(net, dma_pa)`, route
  the IRQ through the PLIC (`init`/`set_priority`/`enable`, idempotent), enable
  `sie` external; then spawn `net_resolver` and grant it `Endpoint(NET_EP)`. Spawn
  the driver before the resolver so the driver `recv`-blocks first.

### Unchanged

- `kernel_common::net` (already pure / host-tested) — reused as-is.
- `virtio.rs` constants (`DEVICE_ID_NET`, the queue/status/descriptor consts) —
  reused as-is.
- `MAX_TASKS 25 → 27` (one driver + one resolver).

## Error handling

| Situation | Behavior |
|---|---|
| No virtio-net device discovered | neither task is spawned; `net: no virtio-net device found` (like the optional rng/blk paths). |
| TX-completion IRQ wakes the driver first | ack `INTERRUPT_STATUS`, re-`wait_irq` until the RX used index advances. |
| Genuine no ARP reply (internal wait cap reached) | driver replies `rx_len = 0`; resolver prints `net: no ARP reply`. |
| A received frame that isn't our ARP reply | `parse_reply → None`; resolver prints `net: no ARP reply` (one exchange, no retry storm). |
| PLIC already initialised by rng/blk | `init` / `sie_enable_external` are idempotent, as in the existing blocks. |

## Testing / "Done when"

- Host: `cargo test` — `kernel_common::net` unit tests stay green (no logic
  change, but the contract is now exercised across the kernel/U-mode boundary).
- Boot: `./tools/test-qemu.ps1` (unchanged `-netdev user,id=net0 -device
  virtio-net-device,netdev=net0` flags) shows
  `net: resolved 10.0.2.2 -> 52:55:0a:00:02:02` — **produced by the U-mode driver**,
  evidenced by a `net` task in the scheduler and the resolver calling it. Same
  QEMU-only scope; no board.

## Scope / YAGNI

One spec: relocate the existing Phase 15 ARP exchange to a U-mode driver +
kernel resolver, byte-for-byte the same wire behavior. **No** new networking
capability — no IP/UDP/ICMP stack, no DHCP, no ping, no encrypting traffic with
the Phase 14 channel, no multiple in-flight frames, no generalized NIC send/recv
API. Those are Phase 17+.

## What this proves / what's next

The NIC joins `rng` and `blk` as an unprivileged, capability-bounded driver the
kernel never touches — the microkernel promise (ADR 0007) now holds for *every*
device the OS drives. With the NIC properly outside the kernel, the natural next
step is a minimal IP/UDP stack (DHCP, ping) built on this driver, then encrypting
that traffic with the Phase 14 channel.
