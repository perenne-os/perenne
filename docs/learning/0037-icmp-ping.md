# 0037 — ICMP echo: pinging the gateway

**One-line:** the OS sends an ICMP Echo Request to the gateway `10.0.2.2` and gets
the reply — `net: ping 10.0.2.2: reply (seq 0)` — its first IP round-trip it
*initiates by address*, reusing the adopted IP (source) and the ARP-resolved
gateway MAC (destination).

## What changed
- A `kernel_common::net::icmp` submodule: `build_echo_request` (Ethernet + IPv4 +
  ICMP type 8) and `parse_echo_reply` (type 0, matching identifier + sequence).
- `net_client` now keeps the gateway MAC it used to discard and adds a fourth
  exchange — the ping — after the ARP step.

## The idea worth keeping: a "new protocol" that's almost no new code
ICMP is IPv4 **protocol 1**, and its checksum is the **same RFC 1071
one's-complement** the IPv4 header already uses. So the whole protocol is ~30
lines on top of `ipv4`: reuse `ipv4::build_header(proto = 1)` for the wrapper and
`ipv4::checksum` for the ICMP checksum (which also verifies to 0, like the IP
header). The lesson from the layered design (note 0035) paying off: once the
primitives are pure and composable, each new protocol is mostly a header layout.

## The idea worth keeping: it chains onto what we already have
Ping needs two addresses it can't make up: a **source IP** (our DHCP-adopted
`NET_IP`, Phase 18) and a **destination MAC** (the gateway's, from the ARP step,
Phase 16). Phase 19 is the first place all three networking phases compose into one
action — `DHCP → adopt → ARP → ping` — each step feeding the next. That's what an IP
stack *is*: small resolved facts (my address, the next hop's MAC) assembled into a
packet.

## The spike outcome
The one unknown was whether QEMU's SLIRP answers an ICMP echo to the gateway in
this harness (host ICMP can need privileges; external pings are non-hermetic). The
boot smoke **was** the spike — and SLIRP replied to `10.0.2.2` directly, on the
first try, no fallback. (Had it not, the plan's documented degraded proof was
TX-only: the request transmitted + the framing host-tested, like Phase 15.)

## Proof
`net: resolved 10.0.2.2 -> 52:55:0a:00:02:02 (src 10.0.2.15)` →
`net: ping 10.0.2.2: reply (seq 0)`, with the cross-boot self-healing demo still
green.

## What's next
RTT/statistics and pinging arbitrary hosts; replying to *inbound* pings; ICMP
error types (destination-unreachable, TTL-exceeded); DNS over the UDP layer; and
encrypting traffic with the Phase 14 channel.
