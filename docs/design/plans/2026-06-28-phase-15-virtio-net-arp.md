# Phase 15 — virtio-net + ARP — Implementation Plan

> Spike-driven (the bring-up + SLIRP-ARP were the risk). The spike passed first
> try, so implementation followed it closely. Recorded here for the convention.

**Goal:** A virtio-net driver brings up the NIC and resolves the gateway by ARP —
the OS's first network exchange.

**Spec:** `docs/design/specs/2026-06-28-phase-15-virtio-net-arp-design.md`
**Learning note:** `docs/learning/0033-virtio-net-arp.md`

## What was built (in order)

1. **Pure ARP logic** — `libs/common/src/net.rs`: `arp::build_request` /
   `arp::parse_reply` (Ethernet/ARP wire format), host-tested (build→parse
   round-trip, reply→MAC, `None` for non-ARP / wrong ethertype / wrong oper /
   wrong target / truncated). Commit `feat(net): pure ARP …`.
2. **`mem::map_device`** — map a device MMIO page into the master kernel table
   (records the master root at `init` in `KERNEL_ROOT`).
3. **Spike** — a kernel-side virtio-net bring-up (modern handshake, two queues
   RX/TX in a DMA frame, pre-posted RX buffer, ARP request via the pure logic,
   notify TX, poll the RX used ring, parse the reply). Run manually with
   `-netdev user -device virtio-net-device`. **Result (first try):**
   `net: resolved 10.0.2.2 -> 52:55:0a:00:02:02` (SLIRP's gateway MAC).
4. **Ship** — promoted the spike to `net_resolve_gateway`, called from `kmain`
   when a virtio-net device is discovered (`virtio::DEVICE_ID_NET = 1`). Commit
   `feat(net): virtio-net driver + ARP gateway resolution …`.
5. **Test** — added `-netdev user,id=net0 -device virtio-net-device,netdev=net0`
   to `tools/test-qemu.ps1`; assert `net: resolved 10.0.2.2 -> 52:55:0a:00:02:02`.
6. **Docs** — learning note 0033, roadmap (Phase 15 done / Phase 16+), glossary
   (virtio-net, ARP, SLIRP), spec revision note.

## Deviation from the spec (documented)

The driver **runs in the kernel** (`net_resolve_gateway`), not as a U-mode
component — the spike already worked kernel-side and used the host-tested `arp`
logic directly (DRY). Moving the NIC driver to an unprivileged user-space
component like rng/blk (ADR 0007) is a deferred refinement, noted in the spec
and learning note. The full ARP exchange (TX + RX) shipped, not the TX-only
fallback.

## Verification

`kernel-common` host tests (3 net) + the rest of the workspace green; the
two-boot smoke PASSES (Phases 2–15) including `net: resolved 10.0.2.2 -> …`.
