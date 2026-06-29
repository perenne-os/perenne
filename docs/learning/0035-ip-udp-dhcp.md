# 0035 — Minimal IP/UDP stack: learning our IP by DHCP

**One-line:** the OS speaks IPv4 + UDP for the first time and **learns its own IP
from the network** — it broadcasts a DHCPDISCOVER and reads the OFFER's address
(`net: dhcp offered 10.0.2.15`), over the Phase 16 user-space NIC.

## What changed
- Pure, host-tested `kernel_common::net` grew three layered submodules:
  `ipv4` (the 20-byte header + RFC 1071 checksum), `udp` (build/parse a datagram
  over IPv4, addressed by port), and `dhcp` (build a DISCOVER, parse an OFFER's
  `yiaddr`). The ARP code is unchanged.
- The U-mode `net` driver generalized from a one-shot to a **bounded server**: it
  re-posts a fresh RX buffer per exchange, serves `recv → transmit → wait-IRQ →
  reply` in a loop, and exits on a sentinel badge (`NET_DONE`).
- The kernel `net_resolver` became `net_client`: it runs the ARP exchange (kept)
  then the DHCP exchange, both through the shared DMA page.

## The idea worth keeping: layers are just nested build/parse over one buffer
A packet is headers wrapped around a payload: `Ethernet[ IPv4[ UDP[ DHCP ] ] ]`.
Each layer is a pure function that writes its header and delegates the inside to
the next (`udp::build` calls `ipv4::build_header`; the DHCP payload is opaque to
UDP). Parsing is the mirror: peel Ethernet → check IPv4 → demux UDP by port →
hand the payload to DHCP. Keeping every layer a pure slice-in/slice-out function
made the whole stack host-testable with no device — the only thing the boot test
adds is "does SLIRP actually answer."

## Two details that matter
- **The IPv4 header checksum** is the one's-complement sum of the header's 16-bit
  words (RFC 1071). The neat property: a header whose checksum field already holds
  the result **re-checksums to 0** — so the same function both computes and
  verifies. (UDP's checksum is optional over IPv4, so we send `0`.)
- **DHCP's broadcast flag.** We have no IP yet, so we can't be addressed by one —
  we set the BOOTP broadcast flag so the server **broadcasts** the OFFER back, and
  send from `0.0.0.0` to `255.255.255.255` (dst MAC `ff:..`). DHCP is the right
  first UDP milestone because SLIRP answers it **locally** (hermetic — no external
  network, unlike DNS which it forwards to the host resolver), and it tells a real
  story: the OS learns its address, echoing Phase 4a reading RAM from firmware.

## Why the driver became a bounded server
ARP + DHCP is two exchanges, but a NIC driver that loops on `recv` forever leaves
its last device-IRQ claim parked in-service (`wait_irq` is what *completes* a
claim — see [note 0034](0034-user-space-nic.md)). So the driver serves a bounded
run and `sys_exit`s on `NET_DONE` (badge 0 — no real frame is 0 bytes), re-posting
one RX buffer per exchange and tracking the used ring by a `seq` counter.

## Proof
`net: resolved 10.0.2.2 -> 52:55:0a:00:02:02` then
`net: dhcp offered 10.0.2.15` then `sched: task 'net' exited (code 0)` — ARP and a
real UDP round-trip from one unprivileged driver, with the cross-boot self-healing
demo still passing.

## What's next
Complete the lease (REQUEST/ACK) and actually **adopt** the offered IP (today we
read it but still send from the hardcoded `10.0.2.15`); DNS over the same UDP
layer; ICMP echo (ping); receiving unsolicited datagrams as an ongoing service;
and encrypting UDP payloads with the Phase 14 channel.
