# 0033 — virtio-net + ARP: the OS's first network exchange

**One-line:** the OS talks to a network for the first time — a virtio-net driver
brings up the NIC and **ARP-resolves the gateway** (`10.0.2.2 -> 52:55:0a:00:02:02`),
transmitting a request and parsing the reply, over QEMU's user network.

## What changed
- A `virtio-net` driver: the modern virtio-mmio handshake with **two**
  virtqueues (RX = queue 0, TX = queue 1) — the first multi-queue device — and a
  12-byte `virtio_net_hdr` prepended to each frame.
- Pure, host-tested `kernel_common::net`: `arp::build_request` /
  `arp::parse_reply` (the Ethernet/ARP wire format). The driver builds the
  request and parses the reply through it.
- `mem::map_device` — map a device's MMIO page into the master table so a
  kernel-side driver can reach it.

## The idea worth keeping: ARP is the right first network milestone
You can't test "networking" without something to talk to. ARP is the smallest
real exchange: one fixed-format 42-byte frame, no IP/TCP stack, and QEMU's
built-in user network (**SLIRP**) answers ARP for the gateway. So the proof is
self-contained: send "who-has 10.0.2.2", receive "is-at <MAC>". The two-queue
shape is the other lesson — unlike rng/blk (one queue), a NIC needs a **receive**
queue with a **pre-posted buffer** so the device has somewhere to write the reply
*before* it arrives, plus a transmit queue.

## How it was de-risked
The unknowns were the two-queue bring-up and whether SLIRP replies to ARP — both
fastest to iterate kernel-side. A **spike** (a kernel bring-up + ARP, direct
memory, `println` debugging) passed on the first run, validating both. The
host-tested `arp` module meant the frame format was already correct; the spike
only had to get the device dance right.

## A documented pivot
The driver **runs in the kernel** for now (a boot-time `net_resolve_gateway`),
not as a U-mode component. The rng/blk/rtc drivers are unprivileged user-space
components (ADR 0007), and a NIC should be too — but the spike already worked
kernel-side and used the pure `arp` logic directly (DRY), so shipping it there
delivered the networking milestone cleanly. **Moving the NIC driver to a
user-space component is the natural next step** (it would do the same bring-up in
U-mode inline asm, like the entropy/blk drivers, and hand-write the ARP bytes
since it can't call the `arp` module).

## Proof
`net: resolved 10.0.2.2 -> 52:55:0a:00:02:02` — the gateway's MAC, learned by ARP
over the live virtio-net device.

## What's next
Move the driver to a user-space component (ADR 0007); a minimal IP/UDP stack
(DHCP, ping); receiving unsolicited frames (RX as an ongoing service, not a
one-shot); and pairing networking with the Phase 14 encrypted channel to encrypt
traffic.
