# 0039 — DNS resolution: turning a name into an address

**One-line:** the OS resolves `example.com` to an IPv4 address by querying SLIRP's
DNS server over UDP — `net: dns example.com -> 172.66.147.243` — its first time
turning a human name into an address.

## What changed
- A `kernel_common::net::dns` module: `build_query(name, txid)` (a DNS A-record
  query) and `parse_response(payload, txid)` (the first A record's IP). Pure,
  host-tested.
- `net_client` adds a DNS exchange — wrap the query in UDP to `10.0.2.3:53` and
  parse the answer.

## The idea worth keeping: DNS is just a UDP payload with a label-encoded name
The query is a 12-byte header + a **question**, and the question's name is encoded
as **length-prefixed labels** (`example.com` → `7 example 3 com 0`) rather than
dotted text. Because Phase 17 already made UDP a clean layer, "add DNS" was just a
new payload shape — `udp::build` carries the query, `udp::parse` returns the
reply's payload, and `dns` only deals with the DNS bytes inside.

## The one parsing subtlety: name compression
A DNS answer repeats the queried name, but to save space it usually stores a
**compression pointer** — a 2-byte `0xC0 <offset>` that points back to the name
earlier in the packet — instead of the labels again. So you can't assume the
answer starts at a fixed offset; you need a generic `skip_name` that handles both
a label sequence and a pointer. Getting that right (and tested against a real
pointer) is most of the parser.

## The hermeticity difference
Everything before this — ARP, DHCP, the gateway ping — SLIRP answers **locally**,
so it works with no outside network. DNS is the first thing SLIRP **forwards** to
the host's resolver, so it depends on this machine having working DNS. The spike
(the boot smoke) confirmed it does: the query reached a real resolver and came back
with a live Cloudflare address. Had it not, an unanswered query would hang the
bounded driver (Phase 20's lesson), and the fallback was a synthetic self-demo
(prove the parser on a fabricated answer). The A record is real and **changes over
time**, so the test asserts *an* IPv4 (`\d+\.\d+\.\d+\.\d+`), not a fixed value.

## Proof
`net: dns example.com -> 172.66.147.243` after the DHCP/ARP/ping lines, then
`sched: task 'net' exited (code 0)`, with the cross-boot self-healing demo still
green. Real round-trip; no fallback.

## What's next
Use the resolved IP for a real exchange (ping/connect to it); DNS caching; AAAA
and other record types; a resolver address learned from DHCP options; and
encrypting traffic with the Phase 14 channel.
