# Phase 18 — Complete the DHCP lease & adopt the leased IP (design)

**Status:** approved 2026-06-29 (user authorized writing the spec and
implementing end-to-end)
**Priority served:** the networking pillar — Phase 17 read a DHCP OFFER but
discarded it (source IP stayed hardcoded). This finishes the handshake
(REQUEST/ACK) and makes the leased address the stack's **real** source IP, so the
OS is configured *by the network* rather than by a constant.

## The gap

Phase 17's `net_client` broadcasts a DISCOVER, parses the OFFER's address, prints
it, and throws it away — it never sends a REQUEST, never receives an ACK, and the
frames it builds still use a hardcoded `[10, 0, 2, 15]` source IP. The lease is
incomplete and the address is not adopted.

## Architecture (extends Phase 17)

Pure DHCP logic grows in `kernel_common::net::dhcp` (host-tested); the kernel
`net_client` runs a reordered exchange sequence through the unchanged Phase 16/17
bounded-server driver. The leased IP lives in one kernel `static`, the single
source of truth for the send path.

### Pure, host-tested additions (`net::dhcp`)

- **`pub struct Offer { pub yiaddr: [u8; 4], pub server_id: [u8; 4] }`** — the
  OFFER carries both the offered address and the **server identifier** (option
  54), which the REQUEST must echo so the right server commits the lease.
- **`parse_offer(payload, xid) -> Option<Offer>`** (signature change from Phase
  17's `Option<[u8;4]>`) — validate a BOOTREPLY/OFFER for our `xid`; return
  `yiaddr` + the option-54 server id.
- **`build_request(xid, client_mac: &[u8;6], requested_ip: [u8;4], server_id: [u8;4], out: &mut [u8]) -> usize`**
  — a DHCPREQUEST: broadcast like DISCOVER (broadcast flag) but message-type
  REQUEST (3) plus option 50 (requested IP = `yiaddr`) and option 54 (server id).
  Length `REQUEST_LEN = 236 + 4 + 3 + 6 + 6 + 1 = 256`.
- **`parse_ack(payload, xid) -> Option<[u8;4]>`** — confirm a DHCPACK
  (message-type 5, our `xid`); return the confirmed `yiaddr`.
- A small internal TLV helper **`option(opts, code) -> Option<&[u8]>`**
  (generalizes Phase 17's `msg_type_is`; used for option 53 and 54), plus an
  internal `is_reply(payload, xid, msg_type)` guard shared by `parse_offer`/`parse_ack`.
  `MSG_REQUEST = 3`, `MSG_ACK = 5` join the existing message-type consts.

Host tests: REQUEST build → re-parse (op/cookie/xid, option 53 = REQUEST, option
50 = requested IP, option 54 = server id); `parse_offer` on a synthesized OFFER
returns `{yiaddr, server_id}`; `parse_ack` on a synthesized ACK returns the
address; both reject a wrong `xid` and a wrong message type.

### Kernel adoption + reordered flow (`net_client`, `kernel/src/main.rs`)

A new `static mut NET_IP: [u8; 4] = [0, 0, 0, 0]` — unconfigured at boot, the
sole source of the stack's source IP. `net_client` is rewritten to:

1. **DISCOVER → OFFER** → `net: dhcp offered 10.0.2.15` (keep an `Offer`:
   `yiaddr` + `server_id`).
2. **REQUEST → ACK** (requested IP = `offer.yiaddr`, server id = `offer.server_id`)
   → `net: dhcp leased 10.0.2.15 (ack)`.
3. **Adopt**: `NET_IP = ack_ip` → `net: adopted ip 10.0.2.15`.
4. **ARP the gateway**, sender IP read from `NET_IP` →
   `net: resolved 10.0.2.2 -> 52:55:0a:00:02:02 (src 10.0.2.15)` — the adopted IP
   flowing into a real frame.
5. `NET_DONE` → the driver exits.

Three driver exchanges (DISCOVER, REQUEST, ARP); the bounded server already serves
N. **No new task, no MAX_TASKS change.** The hardcoded `[10, 0, 2, 15]` source
constant is removed — the ARP's source comes only from `NET_IP`.

## Data flow (the proof)

```
net_client: DISCOVER ─► driver ─► OFFER   {yiaddr, server_id}   "net: dhcp offered 10.0.2.15"
            REQUEST  ─► driver ─► ACK      yiaddr (confirmed)    "net: dhcp leased 10.0.2.15 (ack)"
            NET_IP = yiaddr                                       "net: adopted ip 10.0.2.15"
            ARP(src = NET_IP) ─► driver ─► reply (gw MAC)         "net: resolved 10.0.2.2 -> .. (src 10.0.2.15)"
            NET_DONE ─► driver sys_exit(0)
```

## Error handling

| Situation | Behavior |
|---|---|
| No OFFER within the bounded IRQ wait | `net: no dhcp offer`; skip REQUEST/adopt; still ARP from `NET_IP` (`0.0.0.0`) so the regression line appears (SLIRP answers ARP regardless of sender IP). |
| OFFER but no ACK | `net: no dhcp ack`; skip adopt; ARP from `0.0.0.0`. |
| ACK address differs from the OFFER | adopt the **ACK**'s `yiaddr` (the server's final word). |

## Testing / "Done when"

- Host: `cargo test` — the new `dhcp` tests (REQUEST/ACK, `Offer`) and the updated
  Phase 17 `parse_offer` test, all green.
- Boot: `./tools/test-qemu.ps1` shows, in order, `net: dhcp offered 10.0.2.15`,
  `net: dhcp leased 10.0.2.15 (ack)`, `net: adopted ip 10.0.2.15`, and
  `net: resolved 10.0.2.2 -> 52:55:0a:00:02:02 (src 10.0.2.15)`, with the cross-boot
  self-healing demo still passing. The existing `net: resolved 10.0.2.2 -> …`
  assertion still matches (it is a substring of the new line). QEMU-only; no board.
- Risk: low — REQUEST/ACK is the same UDP/BOOTP framing as DISCOVER/OFFER (already
  proven against SLIRP in Phase 17), plus two options. The one unknown is whether
  SLIRP sends a DHCPACK for the REQUEST; SLIRP implements the full DORA handshake,
  so the risk is small. Fallback: if no ACK arrives, adopt the OFFER's address
  instead and record the deviation honestly.

## Scope / YAGNI

Just lease completion (REQUEST/ACK) + adoption into `NET_IP` + one post-lease
frame (the gateway ARP) that uses it. **No** lease renewal/timers/expiry (T1/T2),
no netmask/router/DNS configuration from DHCP options, no DECLINE/NAK handling
beyond "no ack → skip adopt", no DNS/ICMP. Those are Phase 19+.

## Honest note

SLIRP leases exactly the `10.0.2.15` the stack previously hardcoded, so the
address *value* does not change. Adoption is proven by the **plumbing**: `NET_IP`
starts `0.0.0.0`, becomes the leased value at ACK, and is the sole source the
post-lease ARP reads — the hardcoded constant is gone.

## What this proves / what's next

The OS completes a real DHCP lease and is configured by the network. Deferred:
lease renewal/expiry; adopting the netmask/router/DNS options; DNS resolution and
ICMP echo over the now-configured stack; encrypting UDP payloads with the Phase 14
channel.
