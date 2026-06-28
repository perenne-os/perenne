# Phase 15 — virtio-net + ARP: the OS's first network exchange (design)

**Status:** approved 2026-06-28 (user authorized completing the phase end-to-end)
**Priority served:** the vision's hardware-breadth / "real OS" pillar — opens
**networking**, the project's biggest absent capability. A third user-space
device driver (ADR 0007).

## The gap

The OS drives `virtio-rng` and `virtio-blk` as unprivileged components, but has
no networking at all. Phase 15 adds a `virtio-net` driver and performs the OS's
first real network exchange: **ARP** — transmit a request for the gateway,
receive the reply (its MAC). ARP is the right first milestone: one fixed-format
frame, no IP/TCP stack, and QEMU's built-in user network (SLIRP) answers ARP for
the gateway (`10.0.2.2`), so the exchange is self-contained and testable.

## Architecture

A user-space `net` component (ADR 0007) reuses everything the rng/blk drivers
established: the modern virtio-mmio handshake, a split virtqueue in an
identity-mapped DMA frame, the kernel discovering the device and granting the
component its MMIO + DMA + `Interrupt` cap, and `wait_irq` blocking on the device
IRQ. The new device-specific parts:

- **Two virtqueues** (RX = queue 0, TX = queue 1) instead of one.
- A **12-byte `virtio_net_hdr`** prepended to each frame (modern virtio, GSO_NONE
  / zeroed for our tiny frame).
- **Pre-posting an RX buffer** so the device can deliver the reply.

The MAC is read from device config space (offset `0x100`) after negotiating
`VIRTIO_NET_F_MAC`; the guest IP is SLIRP's default `10.0.2.15`, the gateway
`10.0.2.2`.

## Components

### Pure, host-tested logic (`libs/common`, new `net` module)

- **`arp::build_request(src_mac: &[u8;6], src_ip: [u8;4], target_ip: [u8;4], frame: &mut [u8]) -> usize`**
  — write a 42-byte ARP-request Ethernet frame (broadcast destination,
  ethertype `0x0806`, ARP payload: htype 1, ptype `0x0800`, hlen 6, plen 4, oper
  1, sender MAC/IP, target MAC zero, target IP). Returns the length.
- **`arp::parse_reply(frame: &[u8], want_ip: [u8;4]) -> Option<[u8;6]>`** — if
  `frame` is an ARP **reply** (oper 2) whose sender IP is `want_ip`, return the
  sender's MAC; else `None`.
- Host tests: a built request parses back (sender fields); a synthesized reply
  yields the MAC; a non-ARP frame, wrong ethertype, wrong oper, or wrong target
  IP yields `None`.

### Kernel (`arch/riscv64`, `kernel/src/main.rs`)

Discover the virtio-net slot (`DEVICE_ID_NET = 1`) from the device tree via the
existing `find_device`; map its MMIO + a zeroed DMA frame into the `net`
component (`build_virtio_space`); grant `Interrupt(net_irq)`; route the IRQ
through the PLIC — identical to the blk/rng wiring. `virtio.rs` gains
`DEVICE_ID_NET` and the virtio-net DMA-layout offsets (two queues + an RX buffer
+ a TX buffer within the frame), spike-verified.

### `net` component (U-mode, ADR 0007)

Inline-asm virtio-net bring-up (status/feature handshake negotiating
`VERSION_1` + `MAC`; set up RX queue 0 and TX queue 1 in the DMA frame), read the
MAC from config, pre-post an RX buffer, build the ARP request (the pure `arp`
logic, whose output it copies into the TX buffer after a zeroed net header), post
+ notify TX, `wait_irq`, then walk the RX used ring and `parse_reply`. It reports
the resolved gateway MAC. (All constants ≤ what U-mode materializes inline; no
kernel `.text`/`.rodata`, per the recurring codegen rule.)

## Data flow (the proof)

`net` posts an RX buffer → transmits ARP "who-has 10.0.2.2" → SLIRP replies → the
device writes the reply into the RX buffer and raises its IRQ → the component
`parse_reply`s the sender MAC → `net: resolved 10.0.2.2 -> <MAC>` (logged by the
kernel from the component's report). The smoke asserts that line.

## Risk / spike (do first)

The one real unknown is whether QEMU's user netdev (SLIRP) reliably answers an
ARP request in the smoke harness, plus the exact virtio-net bring-up
(two-queue layout, net header, RX pre-posting). **Spike kernel-side first** (a
temporary kernel bring-up + ARP exchange, easy to iterate, direct memory access),
verify SLIRP replies with the gateway MAC, then port to the user-space component.
**Fallback** if SLIRP-ARP doesn't cooperate: prove the **TX path** only
(transmit a valid ARP frame; the device consumes it via the used ring + IRQ) —
still a real first-networking milestone, mirroring 6a's blk write proof. The
shipped scope (full exchange vs TX-only) will be recorded honestly per the spike.

## Error handling

| Situation | Behavior |
|---|---|
| No virtio-net device discovered | the `net` component isn't spawned (logged), like the optional rng/blk paths. |
| A received frame that isn't our ARP reply | ignored (`parse_reply` → `None`); wait for the next (bounded). |
| Device/queue error | logged; one exchange, no retry storm. |

## Testing

- Host: `arp::build_request`/`parse_reply` (build→parse round-trip, reply→MAC,
  and `None` for non-ARP / wrong ethertype / wrong oper / wrong target).
- Boot test: add `-netdev user,id=net0 -device virtio-net-device,netdev=net0` to
  the QEMU args; assert `net: resolved 10.0.2.2 -> <MAC>` (or the TX-consumed line
  in the fallback).

## Scope / YAGNI

One spec. virtio-net bring-up + a single ARP request/reply. **No** IP/UDP/TCP
stack, no DHCP, no routing, no sockets — just the driver and ARP (the foundation
a later stack builds on). Pairing networking with the Phase 14 encrypted channel
(encrypt traffic) is noted but out of scope.

## What this proves / what's next

The OS speaks to a network for the first time — a user-space NIC driver resolving
the gateway by ARP, interrupt-driven, capability-bounded. Deferred: a minimal
IP/UDP stack (e.g. DHCP, ping), receiving unsolicited frames, multiple NICs, and
encrypting traffic with the Phase 14 channel.
