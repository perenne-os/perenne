# 0036 — Completing the DHCP lease & adopting the IP

**One-line:** the OS finishes the DHCP handshake (DISCOVER→OFFER→**REQUEST→ACK**)
and **adopts** the leased address as its real source IP — the gateway ARP now goes
out `(src 10.0.2.15)` from a leased address, not a hardcoded one.

## What changed
- `kernel_common::net::dhcp` grew the second half of the handshake: an
  `Offer { yiaddr, server_id }` (the OFFER now yields both), `build_request`
  (DHCPREQUEST with option 50 = requested IP + option 54 = server id),
  `parse_ack`, and a generic TLV `option(opts, code)` helper that replaced the
  one-off message-type scan.
- The kernel gained `static mut NET_IP: [u8;4]` (starts `0.0.0.0`), set on ACK.
  `net_client` was reordered: lease first, adopt, then ARP the gateway **from
  `NET_IP`**. The hardcoded `[10,0,2,15]` source constant is gone.

## The idea worth keeping: DORA, and why the REQUEST echoes the server id
DHCP is four messages — **D**iscover, **O**ffer, **R**equest, **A**ck. The subtle
part is the REQUEST: it carries **option 54 (server identifier)**, copied from the
OFFER. On a network with several DHCP servers you'd get several OFFERs; echoing one
server's id in the REQUEST is how you tell that server "I accept yours" and the
others "not yours." Even with SLIRP's single server, sending option 54 (and option
50, the requested address) is what makes it a well-formed REQUEST the server
ACKs — without them many servers stay silent.

## The idea worth keeping: "adopt" means one source of truth
Before, every frame's source IP was a constant. "Adopting" the lease means the
send path reads its address from **one place** — `NET_IP` — that DHCP fills in.
That's the whole point of being configured *by the network*: the value lives in
mutable state the protocol owns, not in the binary. The post-lease gateway ARP is
what gives `NET_IP` a consumer this phase (otherwise "adopt" would just be "store a
value nothing reads").

## Honest note
SLIRP always leases the same `10.0.2.15` we used to hardcode, so the address
*value* doesn't change. Adoption is proven by the **plumbing**: `NET_IP` boots
`0.0.0.0`, becomes the leased value at ACK, and is the sole source the ARP reads —
`(src 10.0.2.15)` in the log is that value flowing out of mutable state.

## A small lifetime note (kernel side)
`dhcp_round` returns the reply's UDP payload as `&'static [u8]` into the
identity-mapped DMA frame (which lives the whole run). The buffer is reused each
round, so each reply is parsed into **owned** data (`Offer` / `[u8;4]`) before the
next round overwrites it.

## Proof
`net: dhcp offered 10.0.2.15` → `net: dhcp leased 10.0.2.15 (ack)` →
`net: adopted ip 10.0.2.15` → `net: resolved 10.0.2.2 -> 52:55:0a:00:02:02
(src 10.0.2.15)` → `sched: task 'net' exited (code 0)`. SLIRP ACKed directly; no
fallback.

## What's next
Lease renewal/expiry (T1/T2 timers); adopting the netmask/router/DNS options DHCP
also offers; DNS resolution and ICMP echo (ping) over the now-configured stack;
encrypting UDP payloads with the Phase 14 channel.
