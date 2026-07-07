# Phase 22 — Ping the DNS-resolved IP (design)

**Status:** approved 2026-07-07
**Priority served:** the networking pillar. The OS can resolve domain names to IPs (Phase 21) and ping the gateway (Phase 19). Pinging the DNS-resolved IP integrates these two capabilities to demonstrate routing and interaction with arbitrary external hosts.

## The gap

The network client resolves `example.com` to an IPv4 address, prints it, and then terminates the network driver. We do not verify that the resolved IP is reachable or that our IP-routing logic correctly handles traffic to external (non-local) IP addresses through the gateway MAC.

## Architecture

This phase requires no changes to the network wire format parser (`kernel_common::net`) or the user-space driver component (`net_component`). It only modifies the kernel-side network orchestrator client (`net_client` in `kernel/src/main.rs`).

### Routing Choice

When sending a packet to an IP address outside our subnet (e.g. `example.com`'s public IP), the routing rule is:
1. **Ethernet Destination MAC:** The gateway's MAC address (`gw_mac`), which we resolved in the ARP step.
2. **IPv4 Destination IP:** The resolved external IP.

This is the standard next-hop routing behavior. It validates that our `build_echo_request` implementation behaves correctly when the destination IP does not match the destination MAC's subnet address directly.

### Integration in `net_client`

1. **Resolve `example.com`**: Execute DNS query/response.
2. **Ping**: If DNS succeeds and returns a valid IP (`[u8; 4]`):
   - Build an ICMP Echo Request frame targeting that IP.
   - Use a unique ping identifier (`0x5555`).
   - Call the network driver to transmit.
   - Wait for the Echo Reply.
3. **Report**: Print either:
   - `net: ping example.com (a.b.c.d): reply (seq 0)` on success.
   - `net: ping example.com (a.b.c.d): no reply` on timeout.
4. **Shutdown**: Send the `NET_DONE` command to shut down the driver.

## Testability & Robustness

Pinging an external host requires the host network to route ICMP Echo Requests from the QEMU SLIRP backend to the internet and back. In offline, firewalled, or sandboxed environments, the request will be sent but no reply will return. 

To ensure the automated test suite remains robust and green across all dev environments:
- The QEMU test runner pattern matching will accept either `reply (seq 0)` or `no reply`.
- E.g. the pattern will be: `net: ping example.com \(\d+\.\d+\.\d+\.\d+\): (reply \(seq 0\)|no reply)`

## Proof

Expected console output during boot:
```
net: dns example.com -> 172.66.147.243
net: ping example.com (172.66.147.243): reply (seq 0)
```
(or `no reply` in offline/restricted environments).
