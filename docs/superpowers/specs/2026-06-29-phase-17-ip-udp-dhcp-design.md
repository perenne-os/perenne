# Phase 17 — Minimal IP/UDP stack: DHCP-learn-our-IP (design)

**Status:** approved 2026-06-29 (user authorized writing the spec and
implementing end-to-end)
**Priority served:** the networking pillar — Phase 15 opened the wire (ARP),
Phase 16 made the NIC an unprivileged component, and this adds the **first real
protocol layer**: IPv4 + UDP. The proof is a DHCP exchange — the OS *learns its
own IP from the network* instead of hardcoding it (echoing Phase 4a, which read
RAM/timebase from firmware rather than hardcoding QEMU's values).

## Implementation note (2026-06-29, during build)

Shipped exactly as designed, no deviations. The host-tested `ipv4`/`udp`/`dhcp`
submodules landed first (6 new tests green, including the canonical IPv4 checksum
vector `0xb861`); the `net` driver became a bounded server (RX re-post per
exchange, `NET_DONE` = 0 exit); `net_resolver` → `net_client` runs ARP then DHCP.
**SLIRP answered the DISCOVER directly** — no TX-only fallback needed: the boot
smoke shows `net: resolved 10.0.2.2 -> 52:55:0a:00:02:02`, `net: dhcp offered
10.0.2.15`, then `sched: task 'net' exited (code 0)`, with both cross-boot
self-healing boots still green (the 90s deadline from Phase 16 held). UDP checksum
0 and the BOOTP broadcast flag worked as expected; no MAX_TASKS change.

## The gap

The OS can put one fixed-format Ethernet frame (ARP) on the wire, but has no IP
or UDP — no way to address a host by IP, no transport ports, no datagrams. This
phase builds a minimal IPv4 + UDP layer and proves it with a single UDP exchange
over QEMU's user network (SLIRP): broadcast a **DHCPDISCOVER** and parse the
**DHCPOFFER**, reading the offered address (`10.0.2.15`).

DHCP is the right first UDP milestone: SLIRP's built-in DHCP server answers it
**locally** (hermetic — no external network, unlike DNS which SLIRP forwards to
the host resolver), it needs no ARP (broadcast), it is one request/response
exchange, and it tells a real story — the OS learns its IP.

## Architecture (extends Phase 16's blk model)

The pure wire logic grows in `kernel_common::net` (host-tested); a kernel client
drives the U-mode `net` driver, which still only moves bytes between RAM and the
wire. Three layered, independently-testable units are added to `net`, plus a
small driver generalization and a renamed/extended kernel client.

### Pure, host-tested logic (`libs/common`, `net` module)

Big-endian on the wire. Each unit builds into and parses out of a byte slice.

- **`ipv4`**
  - `checksum(header: &[u8]) -> u16` — the one's-complement IPv4 header checksum.
  - `build_header(src_ip, dst_ip, proto, payload_len, ident, frame: &mut [u8]) -> usize`
    — a 20-byte IPv4 header (version/IHL, total length, TTL 64, protocol, the
    computed header checksum). Returns 20.
  - `IPV4_HDR_LEN = 20`, `PROTO_UDP = 17`, `ETHERTYPE_IPV4 = 0x0800`.
- **`udp`**
  - `build(src_mac, dst_mac, src_ip, dst_ip, src_port, dst_port, payload, frame) -> usize`
    — assembles Ethernet + IPv4 + UDP around `payload`. UDP checksum is `0` (valid
    for IPv4 per RFC 768; SLIRP accepts it — keeps us off the UDP pseudo-header).
    Returns the total frame length.
  - `parse(frame, want_dst_port) -> Option<&[u8]>` — validates EtherType IPv4,
    protocol UDP, and UDP destination port, and returns the UDP payload slice.
    Lenient on the reply's checksums (demux by port; the framing we sent is what
    we verify, host-side).
  - `UDP_HDR_LEN = 8`.
- **`dhcp`**
  - `build_discover(xid: u32, client_mac: &[u8;6], payload: &mut [u8]) -> usize`
    — a BOOTP/DHCP DISCOVER: op=1 (BOOTREQUEST), htype/hlen Ethernet, the `xid`,
    `chaddr` = client MAC, the magic cookie `0x63825363`, and options
    (DHCP message type = DISCOVER, end). Returns the payload length.
  - `parse_offer(payload, xid) -> Option<[u8;4]>` — if `payload` is a BOOTREPLY
    with the matching `xid`, the magic cookie, and DHCP message type = OFFER (2),
    return `yiaddr` (the offered address); else `None`.
  - DHCP client/server ports `68`/`67`.

Host tests: `ipv4::checksum` against a known vector; `build_header` → checksum
verifies to zero; `udp::build` → `udp::parse` round-trip returns the payload;
`parse` rejects a non-UDP/wrong-port frame; `dhcp::build_discover` →
`parse_offer` on a synthesized OFFER returns `10.0.2.15`; `parse_offer` rejects a
wrong `xid` / wrong message type / a request (op 1).

### `net` driver (U-mode, `kernel/src/main.rs`) — one-shot → bounded server

Generalize the Phase 16 driver: it re-posts a fresh RX buffer per exchange and
serves a `recv → transmit → wait-IRQ → reply` loop, exiting on a sentinel badge.
This keeps Phase 16's "exit early so the last device IRQ claim is not parked
in-service" property while allowing more than one exchange:

```
loop {
    badge = sys_recv(NET_EP_CAP, NET_REPLY_SLOT)
    if badge == NET_DONE { sys_reply(slot, 0); sys_exit(0) }
    // post a fresh RX buffer (each exchange consumes one)
    post rx_desc -> rx_buf, advance rx_avail.idx
    // transmit `badge` bytes from tx_buf, notify queue 1
    post tx_desc(len = badge), advance tx_avail.idx, QUEUE_NOTIFY 1
    // block on the IRQ until the RX used ring advances past the prior count
    rx_len = wait-for-rx (bounded; 0 on no reply)
    sys_reply(NET_REPLY_SLOT, rx_len)
}
```

`NET_DONE` is a reserved badge value (`0`) — no real frame has length 0, so it is
an unambiguous "you're done, exit" signal. The iterator-free constraints from
Phase 16 hold (no `for … in`/`Range` in `.user_text`).

### `net_client` (kernel, renames/extends Phase 16's `net_resolver`)

One kernel task runs all exchanges sequentially, early at boot, through the
shared identity-mapped DMA page:

1. **ARP** (kept, a regression of Phase 15/16): build into the TX buffer, call the
   driver, parse the reply → `net: resolved 10.0.2.2 -> <mac>`.
2. **DHCP**: pick an `xid`, `dhcp::build_discover` → `udp::build` (broadcast dst
   MAC `ff:..`, src IP `0.0.0.0`, dst `255.255.255.255`, UDP 68→67) into the TX
   buffer, call the driver, `udp::parse`(dst port 68) + `dhcp::parse_offer` →
   `net: dhcp offered 10.0.2.15`.
3. `call(driver, NET_DONE)` → the driver exits. Then idle.

## Data flow (the proof)

```
net_client: build ARP         → call(driver, arp_len)  → parse_reply → "net: resolved 10.0.2.2 -> .."
            build DHCPDISCOVER → call(driver, disc_len) → parse_offer → "net: dhcp offered 10.0.2.15"
            call(driver, NET_DONE) ───────────────────────────────────► driver sys_exit(0)
```

The smoke asserts both the (existing) ARP line and the new DHCP line.

## Components / changes

- **Added (`libs/common/src/net.rs`):** `ipv4`, `udp`, `dhcp` submodules + their
  host tests. The existing ARP functions and tests are unchanged.
- **Changed (`kernel/src/main.rs`):** the `net_component` driver loop (bounded
  server, RX re-post, `NET_DONE` exit); `net_resolver` → `net_client` gains the
  DHCP exchange; a `NET_DONE` const. The DMA layout, caps, and spawn wiring are
  unchanged (same TX/RX buffers; the TX buffer just holds a larger frame).
- **Unchanged:** `virtio.rs`, the cap/EP/slot consts, `build_virtio_space`,
  MAX_TASKS (no new task — `net_client` replaces `net_resolver`).

## Error handling

| Situation | Behavior |
|---|---|
| No virtio-net device | neither task spawned; `net: no virtio-net device found` (as today). |
| No DHCP OFFER within the bounded IRQ wait | driver replies `0`; client prints `net: no dhcp offer` (one exchange, no retry storm). |
| A reply that isn't our OFFER (wrong xid/port/op) | `udp::parse`/`dhcp::parse_offer` → `None` → `net: no dhcp offer`. |
| Driver sentinel | `NET_DONE` (badge 0) → driver replies and `sys_exit`s; no parked IRQ. |

## Testing

- Host: `cargo test` — the new `ipv4`/`udp`/`dhcp` tests (the bulk of the work,
  written TDD) plus the unchanged ARP tests, all green.
- Boot: `./tools/test-qemu.ps1` (unchanged QEMU `-netdev user` flags) shows
  `net: resolved 10.0.2.2 -> 52:55:0a:00:02:02` **and**
  `net: dhcp offered 10.0.2.15`, with the cross-boot self-healing write-back still
  passing. QEMU-only; no board.
- Risk / fallback: the one integration unknown is SLIRP answering DISCOVER with an
  OFFER in this harness and our framing being exact. The framing is host-tested
  and SLIRP runs a DHCP server, so the risk is low; if no OFFER arrives, the proof
  degrades to "DISCOVER transmitted and consumed by the device" (TX-only),
  mirroring Phase 15's documented fallback. Recorded honestly per outcome.

## Scope / YAGNI

One spec: an IPv4 + UDP layer and a DHCP DISCOVER→OFFER exchange. **No** full DHCP
lease (REQUEST/ACK), no DNS, no ICMP/ping, no TCP, no computed UDP checksum, no
sockets, and **no reconfiguring the stack to use the leased IP** — DHCP *reads*
`10.0.2.15`; the source IP we send with stays the hardcoded `10.0.2.15`. Those are
Phase 18+.

## What this proves / what's next

The OS speaks IP and UDP for the first time, and learns its own address from the
network. Deferred: completing the lease (REQUEST/ACK) and actually adopting the
leased IP; DNS over the same UDP layer; ICMP echo (ping); receiving unsolicited
datagrams as an ongoing service; and encrypting UDP payloads with the Phase 14
channel.
