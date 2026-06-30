# Phase 20 — Reply to an inbound ping (the OS as a ping target) (design)

**Status:** approved 2026-06-29 (user authorized writing the spec and
implementing end-to-end)
**Priority served:** the networking pillar — Phase 19 *initiated* a ping; this is
the receive-and-respond half: the OS **answers** an ICMP Echo Request addressed to
it, the first time it acts as a network *server* at the IP layer.

## The gap

The OS can compose and send an ICMP echo and parse the reply (Phase 19), but it
cannot *respond* to a packet sent to it — it has no path that receives an inbound
request and emits a reply. This phase adds the responder and demonstrates it.

## The testability constraint (why a self-ping)

QEMU's `-netdev user` (SLIRP) NATs the guest and does **not** forward inbound ICMP,
so no external host can ping the guest in this harness (the same kind of limit as
Phase 9's keystroke injection). The inbound request is therefore sourced by the
guest's **own self-ping**: the OS sends an echo request to its own IP
(`10.0.2.15`) via the gateway MAC; if SLIRP routes a packet bound for the guest's
own address back to the guest, the OS receives a real inbound echo **request** off
the wire and replies. Whether SLIRP loops it is the one unknown — **spiked** via
the boot smoke, with a synthetic fallback (below).

## Architecture (extends Phase 19, no driver/lifecycle change)

Pure responder logic joins `kernel_common::net::icmp`; `net_client` adds a
self-ping loopback exchange after the gateway ping. The bounded-server
`net_component` driver is unchanged — it already does transmit-then-receive per
exchange, so a one-shot inbound reply needs **no persistent listener** and no new
task.

### Pure, host-tested additions (`net::icmp`)

- **`is_echo_request(frame: &[u8], our_ip: [u8;4]) -> bool`** — true iff `frame`
  is an IPv4/ICMP Echo Request (type 8) whose destination IP is `our_ip`.
- **`build_echo_reply(request: &[u8], out: &mut [u8]) -> Option<usize>`** — given a
  received echo request, build the reply into `out`: swap Ethernet src/dst MAC,
  swap IPv4 src/dst, set ICMP type 0 (Echo Reply), keep code/identifier/sequence
  /payload, recompute the IPv4 header checksum **and** the ICMP checksum. Returns
  the reply length (`None` if `request` is too short / not an ICMP echo). Reuses
  `ipv4::checksum`.
- Host tests: build a request (Phase 19's `build_echo_request`) with **distinct**
  peer and our addresses, then `build_echo_reply` → the reply's dst MAC = the
  request's src MAC, its src MAC = the request's dst MAC, the IPv4 src/dst are
  swapped, ICMP type is 0, both checksums verify to 0, and the payload is echoed;
  `is_echo_request` is true for a request addressed to `our_ip`, and false for a
  reply (type 0), a request to a different IP, and a non-ICMP frame.

### Kernel flow (`net_client`, `kernel/src/main.rs`)

After the Phase 19 gateway ping, using the resolved `gw_mac`:

1. **Self-ping**: `build_echo_request(our_mac, gw_mac, NET_IP, NET_IP, ident, seq,
   b"loopback")` into the TX buffer → call the driver (transmit + receive). If
   SLIRP loops the packet back, the driver returns the **inbound echo request**.
2. If the returned frame `is_echo_request(.., NET_IP)`: `build_echo_reply` into the
   TX buffer → call the driver to transmit it → `net: replied to inbound ping
   (seq 0)`.
3. `NET_DONE` → the driver exits.

The RX and TX DMA buffers do not overlap (`0x400` vs `0xC00`), so the reply is
built from the RX frame into the TX frame without aliasing. `ident = 0x4321`,
`seq = 0`, `payload = b"loopback"`. ~Six driver exchanges total; **no new task, no
MAX_TASKS change.**

## Data flow (the proof)

```
net_client: (DHCP + adopt + ARP + gateway ping — Phase 18/19)
            echo-request(src=NET_IP, dst=NET_IP, dst_mac=gw_mac)
                         ─► driver ─► (SLIRP loops it) inbound echo REQUEST to us
            is_echo_request(.., NET_IP) == true
            build_echo_reply(..) ─► driver ─► (reply transmitted)   "net: replied to inbound ping (seq 0)"
            NET_DONE ─► driver sys_exit(0)
```

## Risk / spike + fallback

The boot smoke is the spike for "does SLIRP loop a self-addressed packet back to
the guest." If it does → assert `net: replied to inbound ping (seq 0)`. If it does
**not** (the self-ping returns nothing or a non-request) → the **synthetic
self-demo** fallback: fabricate an inbound echo-request frame in memory (e.g. from
the gateway to us), run the real `is_echo_request` + `build_echo_reply`, and emit
`net: replied to inbound ping (self-demo)` — the responder is real, only the
trigger is synthetic (Phase 9 precedent). The shipped proof is recorded honestly.

## Error handling

| Situation | Behavior |
|---|---|
| No looped-back frame within the bounded IRQ wait | `net: no inbound ping` (then the fallback self-demo). |
| Returned frame isn't an echo request to us | `is_echo_request → false` → `net: no inbound ping`. |
| Gateway ARP failed (no `gw_mac`) | skip: `net: inbound ping skipped (no gateway MAC)`. |

## Testing

- Host: `cargo test` — new `icmp` responder tests (`is_echo_request`,
  `build_echo_reply`) plus the unchanged net tests, all green.
- Boot: `./tools/test-qemu.ps1` shows `net: replied to inbound ping (seq 0)` (or
  the self-demo fallback line) after the existing DHCP/ARP/ping lines, with the
  cross-boot self-healing demo still passing. QEMU-only; no board.

## Scope / YAGNI

One inbound request → one reply, demonstrated via loopback. **No** persistent
always-on ping responder (the driver still exits after its bounded run — a
forever-listening service is deferred, with its timing/lifecycle concerns), no
responding to other ICMP types, no RTT/statistics, no IPv6. Those are Phase 21+.

## What this proves / what's next

The OS answers a packet sent to it — it is a network endpoint, not just a client.
Deferred: a persistent ping responder (the driver as a long-lived RX service);
responding to external pings (needs a non-SLIRP backend); ICMP error replies; DNS
over the UDP layer; and encrypting traffic with the Phase 14 channel.
