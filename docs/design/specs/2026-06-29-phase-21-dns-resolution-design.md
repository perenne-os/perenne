# Phase 21 — DNS resolution (name → IP) over UDP (design)

**Status:** approved 2026-06-29 (user authorized writing the spec and
implementing end-to-end)
**Priority served:** the networking pillar — the stack speaks IPv4/UDP/ICMP and is
DHCP-configured; DNS is the first time it turns a *human name* into an address, the
thing every real network use begins with.

## Implementation note (2026-06-30, during build)

Shipped as designed, **real round-trip** — the spike paid off. The `dns` module
(`build_query` + `parse_response` with compression-pointer skipping) landed first
(2 host tests green); `net_client` wraps the query in `udp` to `10.0.2.3` as the
last exchange. **This harness has working host DNS**, so SLIRP's resolver forwarded
and answered the A query — no self-demo fallback needed: the boot smoke shows
`net: dns example.com -> 172.66.147.243` (a live Cloudflare address; asserted as any
IPv4 since real records vary), then `sched: task 'net' exited (code 0)`, with both
cross-boot self-healing boots green. `10.0.2.3` reached via `gw_mac` (SLIRP's
shared virtual-host MAC) as planned. No new task, no MAX_TASKS change.

## The gap

The OS can address hosts by IP (ping, DHCP, ARP), but has no way to resolve a
**name**. This phase adds a minimal DNS resolver: build an A-record query, send it
over UDP to SLIRP's DNS server, and parse the first A record from the answer.

## The testability constraint (why a spike + fallback)

Unlike ARP/DHCP/the gateway ping — which SLIRP answers **locally** — SLIRP's DNS
server `10.0.2.3` **forwards** queries to the host's resolver. So a real query only
resolves if this machine has working DNS/network (the reason Phase 17 chose DHCP
over DNS). And, as Phase 20 found, if the resolver never answers, the bounded
driver hangs waiting for a reply. The boot smoke is therefore the **spike**, and
DNS is made the **last** exchange so a no-answer stalls only the net task (the rest
of the boot, including the self-healing demo, is unaffected). Fallback: a synthetic
self-demo (Phase 20 precedent).

## Architecture (extends Phase 17's UDP layer)

Pure DNS message logic joins `kernel_common::net::dns`; `net_client` adds a DNS
exchange through the unchanged bounded-server `net_component` driver. No new task,
no driver change.

### Pure, host-tested additions (`net::dns`)

- **`build_query(name: &str, txid: u16, out: &mut [u8]) -> usize`** — a DNS
  A-record query: 12-byte header (id = `txid`, flags `0x0100` recursion-desired,
  QDCOUNT 1, AN/NS/AR 0), the QNAME as length-prefixed labels (`example.com` →
  `0x07 example 0x03 com 0x00`), QTYPE 1 (A), QCLASS 1 (IN). Returns the length.
- **`parse_response(payload: &[u8], txid: u16) -> Option<[u8;4]>`** — verify the id
  and the QR (response) flag, skip the question(s), then walk the `ANCOUNT` answers
  — correctly skipping each answer's NAME (a label sequence **or** a `0xC0…`
  compression pointer) — and return the **first A record's** (TYPE 1, RDLENGTH 4)
  4-byte IP. Handles CNAME-then-A and multiple answers; `None` on wrong id / not a
  response / no A record.
- An internal `skip_name(payload, i) -> Option<usize>` (pointer = +2; label = +1+len;
  root `0` = +1).
- Host tests: `build_query("example.com", 0x1234)` → header (id, flags `0x0100`,
  QDCOUNT 1), QNAME `7 example 3 com 0`, QTYPE 1, QCLASS 1; `parse_response` on a
  synthesized response (id match, QR set, one A answer whose NAME is a `0xC0 0x0C`
  pointer, RDATA `93.184.216.34`) → `Some([93,184,216,34])`; rejects a wrong id, a
  no-answer response (ANCOUNT 0), and a response whose only answer is non-A.

(The `dns` module runs **kernel-side** in `net_client`, so its `&str::split`,
range loops, and `match` are fine — no U-mode codegen constraint.)

### Kernel flow (`net_client`, `kernel/src/main.rs`)

After the inbound-ping self-demo, add (the last exchange before `NET_DONE`), using
the resolved `gw_mac`:

1. **DNS query**: `dns::build_query("example.com", txid)` into a payload buffer →
   `udp::build(our_mac, gw_mac, NET_IP, [10,0,2,3], src_port = 0xC000, dst_port =
   53, ident = 0, payload, tx_frame)` → call the driver. SLIRP's resolver forwards
   to the host and returns the answer.
2. `udp::parse(rxf, 0xC000)` → `dns::parse_response(payload, txid)` →
   `net: dns example.com -> a.b.c.d`.
3. `NET_DONE` → the driver exits.

The DNS server `10.0.2.3` is reached via `gw_mac` — SLIRP routes all its virtual
hosts (gateway, DNS) through one MAC, so no separate ARP for `10.0.2.3` is needed
(avoiding an extra exchange and its hang risk). `txid = 0xABCD`, `src_port =
0xC000`. ~Five driver exchanges total; **no new task, no MAX_TASKS change.**

## Data flow (the proof)

```
net_client: (DHCP + adopt + ARP + ping + inbound-ping self-demo — Phases 18-20)
            dns query "example.com" ─► udp ─► driver ─► (SLIRP forwards to host) DNS answer
            udp::parse(.., 0xC000) -> dns::parse_response(.., txid) -> A record
                                                                   "net: dns example.com -> a.b.c.d"
            NET_DONE ─► driver sys_exit(0)
```

## Risk / spike + fallback

The boot smoke spikes whether this harness resolves names. DNS is the last
exchange, so a no-answer stalls only the net task. Outcomes:
- **Resolves** → assert `net: dns example.com -> \d+\.\d+\.\d+\.\d+` (any IPv4 —
  real A records vary, so the value is not pinned).
- **No answer** → ship the synthetic self-demo: run `parse_response` on a
  synthesized answer in memory and emit `net: dns example.com -> <ip> (self-demo)`,
  removing the hanging wire query. Recorded honestly (Phase 20 precedent).

## Error handling

| Situation | Behavior |
|---|---|
| No DNS answer within the bounded wait | net task stalls on DNS only (last exchange); boot otherwise fine → self-demo fallback. |
| Response with no A record / wrong id | `parse_response → None` → `net: dns example.com: no answer`. |
| Gateway ARP failed (no `gw_mac`) | skip: `net: dns skipped (no gateway MAC)`. |

## Testing

- Host: `cargo test` — the new `dns` tests (`build_query`, `parse_response` incl.
  compression-pointer NAME) plus the unchanged net tests, all green.
- Boot: `./tools/test-qemu.ps1` shows `net: dns example.com -> <ip>` (real) or
  `… (self-demo)` (fallback) after the existing net lines, with the cross-boot
  self-healing demo still passing. QEMU-only; no board.

## Scope / YAGNI

One A-record query for one name → the first A record. **No** caching, no other
record types (AAAA/MX), no CNAME chasing beyond skipping, no retries/multiple
queries, no resolver address from DHCP options, and **no using** the resolved IP
(we resolve and print; connecting to it is later). Those are Phase 22+.

## What this proves / what's next

The OS turns a name into an address — the entry point to every real network
interaction. Deferred: caching; AAAA/other records; using the resolved IP (e.g. a
UDP/TCP exchange with the resolved host); a resolver address learned from DHCP;
and encrypting traffic with the Phase 14 channel.
