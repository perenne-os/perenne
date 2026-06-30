# Phase 19 — ICMP echo (ping) the gateway (design)

**Status:** approved 2026-06-29 (user authorized writing the spec and
implementing end-to-end)
**Priority served:** the networking pillar — Phases 17/18 built IPv4/UDP and a
DHCP-configured address. This adds the OS's first round-trip it **initiates by IP
address**: an ICMP echo to the gateway, reusing the adopted IP (source) and the
ARP-resolved gateway MAC (destination).

## The gap

The stack can build IPv4/UDP and is configured with a leased address, but it has
never sent an IP packet *it composed to reach a host by address* and gotten a
reply. ICMP echo (ping) is the smallest such round-trip: one self-contained
request/reply, no transport ports, reusing everything already built.

## Architecture (extends Phases 17/18)

Pure ICMP logic joins `kernel_common::net`; `net_client` adds a fourth exchange
after the ARP step, capturing the gateway MAC it currently discards. The
bounded-server `net_component` driver is unchanged (it already serves N
exchanges).

### Pure, host-tested additions (`net::icmp`)

ICMP is IPv4 protocol 1, and its checksum is the **same** RFC 1071
one's-complement already in `ipv4` — so this reuses `ipv4::build_header(proto = 1)`
and `ipv4::checksum` directly (no new checksum code).

- **`PROTO_ICMP: u8 = 1`**, `ICMP_ECHO_REQUEST: u8 = 8`, `ICMP_ECHO_REPLY: u8 = 0`.
- **`build_echo_request(src_mac: &[u8;6], dst_mac: &[u8;6], src_ip: [u8;4], dst_ip: [u8;4], ident: u16, seq: u16, payload: &[u8], frame: &mut [u8]) -> usize`**
  — assemble Ethernet + IPv4 (proto 1) + ICMP (type 8, code 0, checksum over the
  ICMP message, identifier, sequence, then `payload`). Returns the frame length.
- **`parse_echo_reply(frame: &[u8], ident: u16, seq: u16) -> bool`** — true iff
  `frame` is an IPv4/ICMP **Echo Reply** (type 0) whose identifier and sequence
  match.

Host tests: a built request, flipped to a reply (ICMP type 0), parses `true`; the
ICMP message checksums to 0 (the same verify-to-zero property as the IPv4 header);
rejects a wrong ICMP type (still 8), a wrong identifier, a wrong sequence, and a
non-ICMP protocol.

### Kernel flow (`net_client`, `kernel/src/main.rs`)

The Phase 18 ARP step prints the gateway MAC and discards it; Phase 19 captures it
into `gw_mac: Option<[u8;6]>` and adds a ping step:

1. DHCP DISCOVER→OFFER→REQUEST→ACK → adopt `NET_IP` *(Phase 18, unchanged)*.
2. ARP the gateway → `net: resolved 10.0.2.2 -> … (src 10.0.2.15)`; keep `gw_mac`.
3. **PING**: build an echo request
   (`our_mac` → `gw_mac`, `NET_IP` → `10.0.2.2`, `ident = 0x1234`, `seq = 0`,
   `payload = b"kernelOS"`) into the shared TX buffer, call the driver, then
   `parse_echo_reply` → `net: ping 10.0.2.2: reply (seq 0)`.
4. `NET_DONE` → the driver exits.

Four driver exchanges (DISCOVER, REQUEST, ARP, PING); the bounded server already
serves N. **No new task, no MAX_TASKS change.**

## Data flow (the proof)

```
net_client: (DHCP lease + adopt — Phase 18)
            ARP 10.0.2.2 ─► driver ─► reply (gw_mac)        "net: resolved 10.0.2.2 -> .. (src 10.0.2.15)"
            echo-request(src=NET_IP, dst=10.0.2.2, dst_mac=gw_mac)
                         ─► driver ─► echo-reply             "net: ping 10.0.2.2: reply (seq 0)"
            NET_DONE ─► driver sys_exit(0)
```

## Risk / spike

The one integration unknown is whether SLIRP answers an ICMP echo to the gateway
in this harness. The ICMP framing is host-tested, so the boot smoke **is** the
spike (like Phase 15). **Fallback** if no reply arrives: prove the Echo Request is
transmitted and consumed by the device (TX-only — the driver's reply length is 0
but the request went out), reported as `net: ping 10.0.2.2: no reply` plus the
host-tested framing standing on its own — a documented degraded proof mirroring
Phase 15's TX-only fallback. The shipped scope (full round-trip vs TX-only) is
recorded honestly per the outcome.

## Error handling

| Situation | Behavior |
|---|---|
| No echo reply within the bounded IRQ wait | `net: ping 10.0.2.2: no reply` (one ping, no retry storm). |
| Gateway ARP failed (no `gw_mac`) | skip the ping: `net: ping skipped (no gateway MAC)`. |
| A reply that isn't our echo (wrong id/seq/type) | `parse_echo_reply → false` → `no reply`. |

## Testing

- Host: `cargo test` — the new `icmp` tests (build→parse-as-reply round-trip,
  checksum-to-zero, and the rejections) plus the unchanged net tests, all green.
- Boot: `./tools/test-qemu.ps1` shows `net: ping 10.0.2.2: reply (seq 0)` (or the
  TX-only fallback line) alongside the existing DHCP/ARP lines, with the cross-boot
  self-healing demo still passing. QEMU-only; no board.

## Scope / YAGNI

One echo request → one reply, to the gateway. **No** repeated pings, statistics,
or RTT timing; no ICMP error types (destination-unreachable, TTL-exceeded); no
fragmentation; no IPv6; no responding to *inbound* pings. Those are Phase 20+.

## What this proves / what's next

The OS composes an IP packet to reach a host by address and gets a reply — the
foundation every higher protocol (and any real diagnostic) stands on. Deferred:
RTT/statistics, pinging arbitrary/external hosts, ICMP error handling, replying to
inbound pings, DNS over the UDP layer, and encrypting traffic with the Phase 14
channel.
