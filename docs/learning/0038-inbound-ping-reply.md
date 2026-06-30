# 0038 — Replying to an inbound ping

**One-line:** the OS can now *answer* a ping — given an ICMP Echo Request
addressed to it, it builds a correct Echo Reply (`net: replied to inbound ping
(self-demo, seq 0)`). The receive-and-respond mirror of Phase 19.

## What changed
- `kernel_common::net::icmp` gained the responder: `is_echo_request(frame,
  our_ip)` and `build_echo_reply(request, out)` — the reply is the request with
  the Ethernet MACs swapped, the IPv4 src/dst swapped, the ICMP type flipped to 0,
  and both checksums recomputed (reusing `ipv4::checksum`). Pure, host-tested.
- `net_client` runs the responder on an inbound echo request and emits the reply.

## The idea worth keeping: a reply is a request, reflected
Building the reply is almost entirely **swaps**: dst/src MAC, dst/src IP, and the
ICMP type 8→0. Everything else — identifier, sequence, payload — is copied
verbatim (that's how `ping` on the other end matches the reply to its request and
echoes the data). The only computed parts are the two checksums, both the same RFC
1071 routine. So "respond to a ping" is ~30 lines that mostly move bytes around.

## The honest part: you can't ping a SLIRP guest from outside
The plan was a real round-trip: **self-ping** our own IP and let SLIRP loop it back
as an inbound request. The spike (the boot smoke) said no on two counts:
1. **SLIRP forwards no inbound ICMP and doesn't loop a self-addressed packet back**
   — a packet to `10.0.2.15` (our own address) is not returned to us.
2. Worse, sending it **hangs the bounded driver**: the driver's contract is
   "transmit, then wait for a reply," and a self-ping that's never answered leaves
   it blocked in `wait_irq` forever (there is no fire-and-forget TX, and no
   timeout on the wait).

So a genuine inbound ping isn't achievable in this harness — the same shape of
limit as Phase 9 (real keystrokes weren't injectable). Following that precedent,
the **responder is proven on a synthesized inbound request**: the kernel fabricates
the echo request a gateway *would* send us and runs the real `is_echo_request` +
`build_echo_reply`. The responder logic is real and host-tested; only the trigger
is fabricated. A real external ping needs a non-SLIRP backend (tap / a second
QEMU), deferred.

## Why this stayed small
No persistent listener, no driver change: the responder is pure logic invoked from
`net_client`. Making the OS a *forever-listening* ping target (the driver as a
long-lived RX service, not a bounded one-shot) is the real productionization —
deferred, with its lifecycle/timing concerns.

## Proof
`net: replied to inbound ping (self-demo, seq 0)` after the DHCP/ARP/ping lines,
with the cross-boot self-healing demo still green. (Plus the host tests:
`build_echo_reply` swaps addresses, flips the type, and both checksums verify.)

## What's next
A persistent ping responder (long-lived RX service); replying to *external* pings
via a non-SLIRP backend; ICMP error replies (destination-unreachable,
TTL-exceeded); DNS over the UDP layer; and encrypting traffic with the Phase 14
channel.
