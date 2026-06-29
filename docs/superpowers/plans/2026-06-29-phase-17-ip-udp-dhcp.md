# Phase 17 — Minimal IP/UDP stack (DHCP) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an IPv4 + UDP layer (host-tested) and prove it with a DHCP DISCOVER→OFFER exchange over SLIRP, so the OS learns its own IP (`10.0.2.15`) from the network — built on the Phase 16 user-space NIC.

**Architecture:** Extend Phase 16's blk model. Pure IPv4/UDP/DHCP wire logic grows in `kernel_common::net` (host-tested, TDD); the U-mode `net` driver generalizes from one-shot to a bounded server (re-post RX per exchange, exit on a sentinel badge); the kernel `net_client` (renamed from `net_resolver`) runs the ARP exchange (kept) then the DHCP exchange, both through the shared DMA page.

**Tech Stack:** Rust `no_std`, RISC-V64, QEMU `virt` + SLIRP user network, virtio-net (Phase 16), the existing capability/IPC + PLIC + `wait_irq` machinery.

## Global Constraints

- **Target/scope:** QEMU `virt` riscv64 only; no board. Unchanged QEMU flags (`-netdev user,id=net0 -device virtio-net-device,netdev=net0`).
- **U-mode codegen rule (driver only):** `net_component` lives in `#[link_section = ".user_text"]` and may NOT call kernel `.text` or use `for … in`/`Range` iterators (they may not inline in debug → a call into kernel `.text` → `InstructionPageFault`). MMIO/DMA via the existing `#[inline(always)]` helpers; manual `while` counters. The pure `net` logic runs **kernel-side** (in `net_client`), so it may use iterators/match freely.
- **Wire format:** big-endian. DHCP DISCOVER is broadcast (dst MAC `ff:..`, src IP `0.0.0.0`, dst `255.255.255.255`, UDP 68→67), with the BOOTP broadcast flag set. UDP checksum = `0` (valid for IPv4). Source IP stays the hardcoded `10.0.2.15` (DHCP *reads* the offered IP; it does not reconfigure the stack).
- **No new task / no MAX_TASKS change:** `net_client` replaces `net_resolver` in the same slot.
- **Sentinel:** `NET_DONE: usize = 0` — the driver's "exit" badge (no real frame has length 0).
- **Done-when (whole phase):** `./tools/test-qemu.ps1` shows both `net: resolved 10.0.2.2 -> 52:55:0a:00:02:02` and `net: dhcp offered 10.0.2.15`, with the cross-boot self-healing demo still passing and `cargo test` green.

## File structure

- `libs/common/src/net.rs` — add `ipv4`, `udp`, `dhcp` submodules + host tests (Tasks 1–3). Existing ARP code/tests unchanged.
- `kernel/src/main.rs` — generalize `net_component` to a bounded server + `NET_DONE` const (Task 4); rename/extend `net_resolver` → `net_client` with the DHCP exchange + update the spawn site (Task 5).
- Docs (Task 7): `docs/learning/0035-*.md`, `docs/roadmap/roadmap.md`, `docs/glossary.md`, the spec impl note.

---

### Task 1: `ipv4` submodule (checksum + header)

**Files:**
- Modify: `libs/common/src/net.rs` (add `pub mod ipv4` after the ARP functions, ~line 66; add tests in the existing `mod tests`)

**Interfaces:**
- Produces: `ipv4::checksum(&[u8]) -> u16`, `ipv4::build_header(src_ip:[u8;4], dst_ip:[u8;4], proto:u8, payload_len:usize, ident:u16, out:&mut [u8]) -> usize`, `ipv4::{IPV4_HDR_LEN, PROTO_UDP, ETHERTYPE_IPV4}`. Consumed by Tasks 2 and 5.

- [ ] **Step 1: Write the failing tests**

In `libs/common/src/net.rs`, inside `mod tests` (before its closing `}`), add:
```rust
    #[test]
    fn ipv4_checksum_canonical_example() {
        // Canonical IPv4 header (Wikipedia) WITH its real checksum 0xb861 in
        // place re-checksums to 0; with the checksum field zeroed it yields 0xb861.
        let full = [
            0x45u8, 0x00, 0x00, 0x73, 0x00, 0x00, 0x40, 0x00, 0x40, 0x11, 0xb8, 0x61,
            0xc0, 0xa8, 0x00, 0x01, 0xc0, 0xa8, 0x00, 0xc7,
        ];
        assert_eq!(ipv4::checksum(&full), 0, "valid header verifies to 0");
        let mut zeroed = full;
        zeroed[10] = 0;
        zeroed[11] = 0;
        assert_eq!(ipv4::checksum(&zeroed), 0xb861, "canonical checksum");
    }

    #[test]
    fn ipv4_build_header_verifies_and_fields() {
        let mut out = [0u8; 20];
        let n = ipv4::build_header([10, 0, 2, 15], [255, 255, 255, 255], ipv4::PROTO_UDP, 8, 0x1234, &mut out);
        assert_eq!(n, ipv4::IPV4_HDR_LEN);
        assert_eq!(ipv4::checksum(&out), 0, "built header self-verifies");
        assert_eq!(out[0], 0x45, "version 4, IHL 5");
        assert_eq!(out[9], ipv4::PROTO_UDP);
        assert_eq!(&out[12..16], &[10, 0, 2, 15]);
        assert_eq!(&out[16..20], &[255, 255, 255, 255]);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p kernel-common ipv4_`
Expected: FAIL — `cannot find ... ipv4` / unresolved module.

- [ ] **Step 3: Implement the `ipv4` submodule**

In `libs/common/src/net.rs`, after the `parse_reply` function (after line 66, before `#[cfg(test)]`), add:
```rust
/// IPv4 — the smallest header needed to carry UDP. Big-endian on the wire.
pub mod ipv4 {
    pub const IPV4_HDR_LEN: usize = 20;
    pub const PROTO_UDP: u8 = 17;
    pub const ETHERTYPE_IPV4: u16 = 0x0800;

    /// One's-complement checksum (RFC 1071) over `bytes`, as the IPv4 header
    /// uses. A header whose checksum field already holds the result verifies to 0.
    pub fn checksum(bytes: &[u8]) -> u16 {
        let mut sum: u32 = 0;
        let mut i = 0;
        while i + 1 < bytes.len() {
            sum += u16::from_be_bytes([bytes[i], bytes[i + 1]]) as u32;
            i += 2;
        }
        if i < bytes.len() {
            sum += (bytes[i] as u32) << 8; // odd trailing byte, high-padded
        }
        while sum >> 16 != 0 {
            sum = (sum & 0xffff) + (sum >> 16);
        }
        !(sum as u16)
    }

    /// Write a 20-byte IPv4 header carrying `payload_len` bytes of `proto` into
    /// `out` (TTL 64, no fragmentation, header checksum computed). Returns 20.
    pub fn build_header(
        src_ip: [u8; 4],
        dst_ip: [u8; 4],
        proto: u8,
        payload_len: usize,
        ident: u16,
        out: &mut [u8],
    ) -> usize {
        let h = &mut out[..IPV4_HDR_LEN];
        h[0] = 0x45; // version 4, IHL 5 (20 bytes)
        h[1] = 0; // DSCP/ECN
        let total = (IPV4_HDR_LEN + payload_len) as u16;
        h[2..4].copy_from_slice(&total.to_be_bytes());
        h[4..6].copy_from_slice(&ident.to_be_bytes());
        h[6..8].copy_from_slice(&0u16.to_be_bytes()); // flags + fragment offset
        h[8] = 64; // TTL
        h[9] = proto;
        h[10..12].copy_from_slice(&0u16.to_be_bytes()); // checksum: zero, then fill
        h[12..16].copy_from_slice(&src_ip);
        h[16..20].copy_from_slice(&dst_ip);
        let csum = checksum(h);
        h[10..12].copy_from_slice(&csum.to_be_bytes());
        IPV4_HDR_LEN
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p kernel-common ipv4_`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add libs/common/src/net.rs
git commit -m "feat(net): IPv4 header + RFC1071 checksum (pure, host-tested)"
```

---

### Task 2: `udp` submodule (build + parse)

**Files:**
- Modify: `libs/common/src/net.rs` (add `pub mod udp` after `pub mod ipv4`; add tests in `mod tests`)

**Interfaces:**
- Consumes: `ipv4::*` (Task 1); the private `be16` and `pub const ETH_HDR_LEN` in `net` (a child module reaches the parent's items via `super::`).
- Produces: `udp::build(src_mac:&[u8;6], dst_mac:&[u8;6], src_ip:[u8;4], dst_ip:[u8;4], src_port:u16, dst_port:u16, ident:u16, payload:&[u8], frame:&mut [u8]) -> usize`, `udp::parse(frame:&[u8], want_dst_port:u16) -> Option<&[u8]>`, `udp::UDP_HDR_LEN`. Consumed by Task 5.

- [ ] **Step 1: Write the failing tests**

In `mod tests`, add:
```rust
    #[test]
    fn udp_build_then_parse_roundtrip() {
        let src_mac = [0x52u8, 0x54, 0x00, 0x12, 0x34, 0x56];
        let dst_mac = [0xffu8; 6];
        let payload = [0xde_u8, 0xad, 0xbe, 0xef];
        let mut frame = [0u8; 128];
        let n = udp::build(&src_mac, &dst_mac, [0, 0, 0, 0], [255, 255, 255, 255], 68, 67, 0x1234, &payload, &mut frame);
        // IPv4 header (bytes 14..34) self-verifies.
        assert_eq!(ipv4::checksum(&frame[14..34]), 0);
        // Wrong port -> None; right port -> the payload back.
        assert!(udp::parse(&frame[..n], 53).is_none());
        assert_eq!(udp::parse(&frame[..n], 67), Some(&payload[..]));
    }

    #[test]
    fn udp_parse_rejects_non_udp() {
        let mut frame = [0u8; 64];
        frame[12..14].copy_from_slice(&0x0806u16.to_be_bytes()); // ARP ethertype
        assert!(udp::parse(&frame, 67).is_none());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p kernel-common udp_`
Expected: FAIL — unresolved module `udp`.

- [ ] **Step 3: Implement the `udp` submodule**

In `libs/common/src/net.rs`, after the `ipv4` module, add:
```rust
/// UDP over IPv4 over Ethernet. Builds a full frame; parses incoming datagrams
/// by destination port. UDP checksum is 0 on send (valid for IPv4).
pub mod udp {
    use super::ipv4;
    pub const UDP_HDR_LEN: usize = 8;

    /// Assemble Ethernet + IPv4 + UDP around `payload` into `frame`. Returns the
    /// total frame length (14 + 20 + 8 + payload).
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        src_mac: &[u8; 6],
        dst_mac: &[u8; 6],
        src_ip: [u8; 4],
        dst_ip: [u8; 4],
        src_port: u16,
        dst_port: u16,
        ident: u16,
        payload: &[u8],
        frame: &mut [u8],
    ) -> usize {
        // Ethernet header.
        frame[0..6].copy_from_slice(dst_mac);
        frame[6..12].copy_from_slice(src_mac);
        frame[12..14].copy_from_slice(&ipv4::ETHERTYPE_IPV4.to_be_bytes());
        // IPv4 header (covers the UDP header + payload).
        let udp_len = UDP_HDR_LEN + payload.len();
        let ip = super::ETH_HDR_LEN;
        ipv4::build_header(src_ip, dst_ip, ipv4::PROTO_UDP, udp_len, ident, &mut frame[ip..ip + ipv4::IPV4_HDR_LEN]);
        // UDP header + payload.
        let u = ip + ipv4::IPV4_HDR_LEN;
        frame[u..u + 2].copy_from_slice(&src_port.to_be_bytes());
        frame[u + 2..u + 4].copy_from_slice(&dst_port.to_be_bytes());
        frame[u + 4..u + 6].copy_from_slice(&(udp_len as u16).to_be_bytes());
        frame[u + 6..u + 8].copy_from_slice(&0u16.to_be_bytes()); // checksum 0 (optional for IPv4)
        frame[u + 8..u + 8 + payload.len()].copy_from_slice(payload);
        u + 8 + payload.len()
    }

    /// If `frame` is an IPv4/UDP datagram addressed to `want_dst_port`, return its
    /// UDP payload. Lenient on checksums (we demux by port). `None` otherwise.
    pub fn parse(frame: &[u8], want_dst_port: u16) -> Option<&[u8]> {
        let eth = super::ETH_HDR_LEN;
        if frame.len() < eth + ipv4::IPV4_HDR_LEN + UDP_HDR_LEN {
            return None;
        }
        if super::be16(&frame[12..14]) != ipv4::ETHERTYPE_IPV4 {
            return None;
        }
        let ihl = (frame[eth] & 0x0f) as usize * 4;
        if ihl < ipv4::IPV4_HDR_LEN || frame.len() < eth + ihl + UDP_HDR_LEN {
            return None;
        }
        if frame[eth + 9] != ipv4::PROTO_UDP {
            return None;
        }
        let u = eth + ihl;
        if super::be16(&frame[u + 2..u + 4]) != want_dst_port {
            return None;
        }
        let ulen = super::be16(&frame[u + 4..u + 6]) as usize;
        if ulen < UDP_HDR_LEN || u + ulen > frame.len() {
            return None;
        }
        Some(&frame[u + UDP_HDR_LEN..u + ulen])
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p kernel-common udp_`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add libs/common/src/net.rs
git commit -m "feat(net): UDP-over-IPv4 build/parse (pure, host-tested)"
```

---

### Task 3: `dhcp` submodule (build_discover + parse_offer)

**Files:**
- Modify: `libs/common/src/net.rs` (add `pub mod dhcp` after `pub mod udp`; add tests in `mod tests`)

**Interfaces:**
- Produces: `dhcp::build_discover(xid:u32, client_mac:&[u8;6], out:&mut [u8]) -> usize`, `dhcp::parse_offer(payload:&[u8], xid:u32) -> Option<[u8;4]>`, `dhcp::{CLIENT_PORT, SERVER_PORT, DISCOVER_LEN}`. Consumed by Task 5.

- [ ] **Step 1: Write the failing tests**

In `mod tests`, add:
```rust
    #[test]
    fn dhcp_discover_then_offer_roundtrip() {
        let mac = [0x52u8, 0x54, 0x00, 0x12, 0x34, 0x56];
        let xid = 0x1234_5678u32;
        let mut disc = [0u8; dhcp::DISCOVER_LEN];
        let n = dhcp::build_discover(xid, &mac, &mut disc);
        assert_eq!(n, dhcp::DISCOVER_LEN);
        assert_eq!(disc[0], 1, "BOOTREQUEST");
        assert_eq!(&disc[236..240], &[0x63, 0x82, 0x53, 0x63], "magic cookie");
        assert_eq!(&disc[28..34], &mac, "chaddr");
        // Synthesize the OFFER the server would send back.
        let mut offer = disc;
        offer[0] = 2; // BOOTREPLY
        offer[16..20].copy_from_slice(&[10, 0, 2, 15]); // yiaddr
        offer[242] = 2; // option 53 value = OFFER
        assert_eq!(dhcp::parse_offer(&offer, xid), Some([10, 0, 2, 15]));
    }

    #[test]
    fn dhcp_parse_offer_rejects_mismatches() {
        let mac = [0x52u8, 0x54, 0x00, 0x12, 0x34, 0x56];
        let xid = 0xaabb_ccddu32;
        let mut offer = [0u8; dhcp::DISCOVER_LEN];
        dhcp::build_discover(xid, &mac, &mut offer);
        offer[0] = 2;
        offer[16..20].copy_from_slice(&[10, 0, 2, 15]);
        offer[242] = 2;
        assert!(dhcp::parse_offer(&offer, 0x9999_9999).is_none(), "wrong xid");
        let mut req = offer;
        req[0] = 1; // BOOTREQUEST, not reply
        assert!(dhcp::parse_offer(&req, xid).is_none(), "not a reply");
        let mut not_offer = offer;
        not_offer[242] = 1; // msg type DISCOVER, not OFFER
        assert!(dhcp::parse_offer(&not_offer, xid).is_none(), "not an offer");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p kernel-common dhcp_`
Expected: FAIL — unresolved module `dhcp`.

- [ ] **Step 3: Implement the `dhcp` submodule**

In `libs/common/src/net.rs`, after the `udp` module, add:
```rust
/// Minimal DHCP (over BOOTP): build a DISCOVER, parse an OFFER's offered address.
pub mod dhcp {
    pub const CLIENT_PORT: u16 = 68;
    pub const SERVER_PORT: u16 = 67;
    const MAGIC: [u8; 4] = [0x63, 0x82, 0x53, 0x63];
    const OP_REQUEST: u8 = 1;
    const OP_REPLY: u8 = 2;
    const MSG_DISCOVER: u8 = 1;
    const MSG_OFFER: u8 = 2;
    /// BOOTP fixed area (op..file) before the magic cookie.
    const BOOTP_FIXED: usize = 236;
    /// DISCOVER payload: fixed area + cookie(4) + option 53 (3) + end (1).
    pub const DISCOVER_LEN: usize = BOOTP_FIXED + 4 + 3 + 1;

    /// Build a DHCPDISCOVER BOOTP payload into `out` (>= DISCOVER_LEN). The
    /// broadcast flag is set so the OFFER is broadcast back (we have no IP yet).
    /// Returns the payload length.
    pub fn build_discover(xid: u32, client_mac: &[u8; 6], out: &mut [u8]) -> usize {
        let p = &mut out[..DISCOVER_LEN];
        for b in p.iter_mut() {
            *b = 0;
        }
        p[0] = OP_REQUEST;
        p[1] = 1; // htype = Ethernet
        p[2] = 6; // hlen
        p[4..8].copy_from_slice(&xid.to_be_bytes());
        p[10..12].copy_from_slice(&0x8000u16.to_be_bytes()); // flags: broadcast
        p[28..34].copy_from_slice(client_mac); // chaddr (first 6 of 16)
        p[236..240].copy_from_slice(&MAGIC);
        p[240] = 53; // option: DHCP message type
        p[241] = 1;
        p[242] = MSG_DISCOVER;
        p[243] = 255; // end
        DISCOVER_LEN
    }

    /// If `payload` is a DHCPOFFER (BOOTREPLY, our `xid`, magic cookie, message
    /// type OFFER), return the offered address (`yiaddr`). `None` otherwise.
    pub fn parse_offer(payload: &[u8], xid: u32) -> Option<[u8; 4]> {
        if payload.len() < 240 {
            return None;
        }
        if payload[0] != OP_REPLY || payload[4..8] != xid.to_be_bytes() || payload[236..240] != MAGIC {
            return None;
        }
        if !msg_type_is(&payload[240..], MSG_OFFER) {
            return None;
        }
        let mut ip = [0u8; 4];
        ip.copy_from_slice(&payload[16..20]); // yiaddr
        Some(ip)
    }

    /// Walk the TLV option area for option 53 (DHCP message type) == `want`.
    fn msg_type_is(opts: &[u8], want: u8) -> bool {
        let mut i = 0;
        while i < opts.len() {
            match opts[i] {
                0 => i += 1,        // pad
                255 => return false, // end
                53 => return i + 2 < opts.len() && opts[i + 1] >= 1 && opts[i + 2] == want,
                _ => {
                    if i + 1 >= opts.len() {
                        return false;
                    }
                    i += 2 + opts[i + 1] as usize;
                }
            }
        }
        false
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p kernel-common dhcp_`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add libs/common/src/net.rs
git commit -m "feat(net): DHCP DISCOVER build + OFFER parse (pure, host-tested)"
```

---

### Task 4: Generalize the `net` driver to a bounded server

**Files:**
- Modify: `kernel/src/main.rs` — add `NET_DONE` const near the net consts (~line 458); rewrite the `net_component` serve section (the post-bring-up part, ~lines 1606–1652).

**Interfaces:**
- Consumes: `NET_EP_CAP`, `NET_IRQ_CAP`, `NET_REPLY_SLOT` (existing); `sys_recv`, `sys_reply`, `sys_wait_irq`, `sys_exit`, `dma_w*`, `dma_r16`, `dma_r32`, `mmio_*`, `virtio::*` (existing).
- Produces: `NET_DONE: usize = 0`; a `net_component` that serves N exchanges (badge = TX length) then exits on `NET_DONE`. Consumed by Task 5.

- [ ] **Step 1: Add the `NET_DONE` const**

In `kernel/src/main.rs`, after `const NET_REPLY_SLOT: usize = 2;` (~line 458), add:
```rust
    /// Sentinel badge that tells the net driver to exit (no real frame has
    /// length 0). The kernel `net_client` sends it after its last exchange.
    const NET_DONE: usize = 0;
```

- [ ] **Step 2: Replace the driver's single-exchange body with a serve loop**

In `net_component`, replace everything from the `// --- pre-post one RX buffer (device-writable) ---` comment through the `sys_exit(0)` line and its closing (the current lines 1606–1652, i.e. the block starting `// --- pre-post one RX buffer` and ending `            sys_exit(0)\n        }\n    }`) with:
```rust
            // Serve exchanges until told to exit. Each exchange: re-post a fresh
            // RX buffer (the device consumes one per reply), transmit the caller's
            // frame, block on the IRQ until the RX used ring advances, reply the
            // received length. A bounded server, not a recv-idle loop: it exits on
            // NET_DONE so its last device IRQ claim isn't left parked in-service.
            let mut seq: u16 = 0;
            loop {
                // badge = total TX length (12-byte virtio_net_hdr + frame), or
                // NET_DONE to stop.
                let badge = sys_recv(NET_EP_CAP, NET_REPLY_SLOT);
                if badge == NET_DONE {
                    sys_reply(NET_REPLY_SLOT, 0);
                    sys_exit(0)
                }
                let slot = (seq % virtio::VQ_SIZE as u16) as usize;

                // post a fresh RX buffer (device writes into rx_buf)
                dma_w64(rx_desc, rx_buf as u64);
                dma_w32(rx_desc + 8, 2048);
                dma_w16(rx_desc + 12, virtio::VIRTQ_DESC_F_WRITE);
                dma_w16(rx_desc + 14, 0);
                dma_w16(rx_avail + 4 + slot * 2, 0); // ring[slot] -> desc 0
                dma_fence();
                dma_w16(rx_avail + 2, seq + 1);
                dma_fence();

                // publish the TX descriptor (device reads tx_buf), notify queue 1
                dma_w64(tx_desc, tx_buf as u64);
                dma_w32(tx_desc + 8, badge as u32);
                dma_w16(tx_desc + 12, 0);
                dma_w16(tx_desc + 14, 0);
                dma_w16(tx_avail + 4 + slot * 2, 0);
                dma_fence();
                dma_w16(tx_avail + 2, seq + 1);
                dma_fence();
                mmio_w(mmio, virtio::QUEUE_NOTIFY, 1);

                // Block on the IRQ until the RX used ring reaches seq+1. A
                // TX-completion IRQ may wake us first; ack and re-wait. Bounded so
                // a genuine no-reply replies 0. Manual `while`, not `for 0..16`
                // (Range iterators may not inline in U-mode).
                let mut rx_len: usize = 0;
                let mut attempts: u32 = 0;
                while attempts < 16 {
                    sys_wait_irq(NET_IRQ_CAP);
                    let is = mmio_r(mmio, virtio::INTERRUPT_STATUS);
                    if is != 0 {
                        mmio_w(mmio, virtio::INTERRUPT_ACK, is);
                    }
                    if dma_r16(rx_used + 2) == seq + 1 {
                        // used-ring element `slot`: id(u32) then len(u32).
                        rx_len = dma_r32(rx_used + 8 + slot * 8) as usize;
                        break;
                    }
                    attempts += 1;
                }
                seq = seq.wrapping_add(1);
                sys_reply(NET_REPLY_SLOT, rx_len);
            }
        }
    }
```

(Note: the bring-up above this block — the STATUS handshake, the two
`virtio_queue_setup` calls, and the `DRIVER_OK` write — is unchanged. Only the
single pre-posted RX buffer + the one-shot exchange are replaced by the loop.)

- [ ] **Step 3: Build the kernel**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: builds (a `NET_DONE`/`net_component` dead-code or unused warning is fine until Task 5 wires the client; no errors).

- [ ] **Step 4: Commit**

```bash
git add kernel/src/main.rs
git commit -m "feat(net): net driver becomes a bounded server (RX re-post per exchange, NET_DONE exit)"
```

---

### Task 5: Extend the kernel client (`net_resolver` → `net_client`) with the DHCP exchange

**Files:**
- Modify: `kernel/src/main.rs` — rename `net_resolver` → `net_client` and add the DHCP exchange (~lines 1655–1697); update the spawn site (~lines around the `netres` spawn in `kmain`).

**Interfaces:**
- Consumes: `NET_DMA_PA`, `NET_EP_CAP`, `NET_DONE` (Tasks 1–4); `sched::call_message`; `kernel_common::net::{build_request, parse_reply, ARP_FRAME_LEN, dhcp, udp}`.
- Produces: `extern "C" fn net_client() -> !`.

- [ ] **Step 1: Replace `net_resolver` with `net_client`**

In `kernel/src/main.rs`, replace the entire `net_resolver` function (its doc comment through its closing `}`, ~lines 1655–1697) with:
```rust
    /// The kernel client for the user-space `net` driver (Phase 16/17). It runs
    /// two exchanges through the shared identity-mapped DMA page: (1) ARP-resolve
    /// the gateway (Phase 15/16 regression), then (2) a DHCP DISCOVER→OFFER to
    /// learn our IP from the network (Phase 17). All wire framing is the
    /// host-tested `kernel_common::net`; the driver only moves bytes. It then
    /// tells the driver to exit (NET_DONE) and idles.
    extern "C" fn net_client() -> ! {
        use kernel_common::net;
        // SAFETY: NET_DMA_PA is the kernel-allocated, identity-mapped DMA frame;
        // single hart owns it. The driver shares the same physical frame RW-U.
        unsafe {
            let dma = NET_DMA_PA;
            let tx_frame = dma + 0xC00 + 12; // after the 12-byte virtio_net_hdr
            let rx_frame = dma + 0x400 + 12;
            let src_mac = [0x52u8, 0x54, 0x00, 0x12, 0x34, 0x56];

            // --- exchange 1: ARP resolve the gateway ---
            let txf = core::slice::from_raw_parts_mut(tx_frame as *mut u8, net::ARP_FRAME_LEN);
            let arp_len = net::build_request(&src_mac, [10, 0, 2, 15], [10, 0, 2, 2], txf);
            let rx_len = sched::call_message(NET_EP_CAP, 12 + arp_len);
            let mut resolved = false;
            if rx_len != 0 {
                let rxf = core::slice::from_raw_parts(rx_frame as *const u8, net::ARP_FRAME_LEN);
                if let Some(mac) = net::parse_reply(rxf, [10, 0, 2, 2]) {
                    println!(
                        "net: resolved 10.0.2.2 -> {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
                    );
                    resolved = true;
                }
            }
            if !resolved {
                println!("net: no ARP reply");
            }

            // --- exchange 2: DHCP DISCOVER -> OFFER (learn our IP) ---
            let xid = 0x1234_5678u32;
            let mut disc = [0u8; net::dhcp::DISCOVER_LEN];
            net::dhcp::build_discover(xid, &src_mac, &mut disc);
            let frame_len = {
                let txf = core::slice::from_raw_parts_mut(tx_frame as *mut u8, 512);
                net::udp::build(
                    &src_mac, &[0xffu8; 6], [0, 0, 0, 0], [255, 255, 255, 255],
                    net::dhcp::CLIENT_PORT, net::dhcp::SERVER_PORT, xid as u16, &disc, txf,
                )
            };
            let rx_len = sched::call_message(NET_EP_CAP, 12 + frame_len);
            let mut offered = false;
            if rx_len != 0 {
                let flen = rx_len.saturating_sub(12).min(2036);
                let rxf = core::slice::from_raw_parts(rx_frame as *const u8, flen);
                if let Some(payload) = net::udp::parse(rxf, net::dhcp::CLIENT_PORT) {
                    if let Some(ip) = net::dhcp::parse_offer(payload, xid) {
                        println!("net: dhcp offered {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
                        offered = true;
                    }
                }
            }
            if !offered {
                println!("net: no dhcp offer");
            }

            // --- tell the driver to exit ---
            let _ = sched::call_message(NET_EP_CAP, NET_DONE);
        }
        // Done: idle like the other one-shot kernel tasks.
        loop {
            sched::yield_now();
            // SAFETY: wait for the next interrupt between yields.
            unsafe { core::arch::asm!("wfi") };
        }
    }
```

- [ ] **Step 2: Update the spawn site**

In `kmain`, find the net spawn (the `let netres = sched::spawn("netres", net_resolver, …)` line in the `if let Some(net) = net_base { … }` block) and change it to:
```rust
            // The client is the net driver's caller: grant it the service cap.
            let netcli = sched::spawn("netcli", net_client,
                core::ptr::addr_of!(KS_NETRES) as usize + TASK_STACK);
            sched::grant_cap(netcli, NET_EP_CAP, Capability::Endpoint(NET_EP));
```
(The `KS_NETRES` stack static is reused as-is — only the task label and function change.)

- [ ] **Step 3: Build the kernel**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: builds clean — no `net_resolver`/`NET_DONE`/`net_component` dead-code warnings (all wired).

- [ ] **Step 4: Commit**

```bash
git add kernel/src/main.rs
git commit -m "feat(net): net_client runs ARP then DHCP DISCOVER->OFFER, learns our IP"
```

---

### Task 6: Verify — host tests + boot smoke

**Files:** none (verification only). May modify `tools/test-qemu.ps1` only to add the new assertion line.

- [ ] **Step 1: Host tests**

Run: `cargo test`
Expected: PASS — all `kernel_common::net` tests including the new `ipv4_*`, `udp_*`, `dhcp_*` and the unchanged ARP tests. No regressions.

- [ ] **Step 2: Add the DHCP assertion to the smoke test**

In `tools/test-qemu.ps1`, in the `$mustMatch1` array, after the line
`"net: resolved 10.0.2.2 -> 52:55:0a:00:02:02",` add:
```powershell
    "net: dhcp offered 10.0.2.15",
```

- [ ] **Step 3: Boot smoke**

Run: `./tools/test-qemu.ps1`
Expected: PASS — the serial log shows both
```
net: resolved 10.0.2.2 -> 52:55:0a:00:02:02
net: dhcp offered 10.0.2.15
```
and the cross-boot self-healing demo still passes (both boots green).

- [ ] **Step 4: If the DHCP line is missing, debug before proceeding**

Do NOT loosen the assertion. Likely culprits:
- `net: no dhcp offer` → the OFFER didn't parse. Check the driver's second exchange completed (the RX re-post + `rx_used == seq+1` at `slot = 1`, len at `rx_used + 8 + 8`); confirm `udp::parse(rxf, 68)` (the OFFER's dst port is the client port 68); confirm `xid` matches. Add a temporary `println!` of `rx_len` in `net_client` if needed.
- Hang at the DHCP exchange → the driver's `seq`/`slot` bookkeeping. Verify the first exchange set `rx_used.idx == 1` and the second waits for `== 2`.
- No OFFER at all from SLIRP → confirm the broadcast flag (`0x8000`) is set and the frame is well-formed (src IP `0.0.0.0`, dst `255.255.255.255`, dst MAC `ff:..`). Per the spec, the documented fallback is to prove the DISCOVER is transmitted/consumed; only fall back if SLIRP genuinely doesn't answer.

- [ ] **Step 5: Commit the test change**

```bash
git add tools/test-qemu.ps1
git commit -m "test: assert the DHCP OFFER (net: dhcp offered 10.0.2.15)"
```

---

### Task 7: Documentation

**Files:**
- Create: `docs/learning/0035-ip-udp-dhcp.md`
- Modify: `docs/roadmap/roadmap.md` (turn "Phase 17+ — Breadth" into a completed Phase 17 entry + a fresh Phase 18+ tail)
- Modify: `docs/glossary.md` (add IPv4 / UDP / DHCP entries)
- Modify: `docs/superpowers/specs/2026-06-29-phase-17-ip-udp-dhcp-design.md` (add an impl note)

- [ ] **Step 1: Write learning note 0035**

Create `docs/learning/0035-ip-udp-dhcp.md` — short (summary, not tutorial; per the project convention). Cover: the layered build/parse design (Ethernet→IPv4→UDP→DHCP), the IPv4 header checksum (RFC 1071 one's-complement, and that a correct header re-checksums to 0), why UDP checksum 0 is valid for IPv4, why DHCP is the right first UDP milestone (SLIRP answers it locally — hermetic — and the OS *learns its own IP*, echoing Phase 4a reading RAM from firmware), the broadcast flag (no IP yet → ask the server to broadcast the OFFER), and the driver generalization (one-shot → bounded server, RX re-post per exchange, `NET_DONE` exit so the last IRQ claim isn't parked). Note the deferral: we read the offered IP but don't yet REQUEST/ACK or reconfigure the stack to use it.

- [ ] **Step 2: Update the roadmap**

Replace the `## Phase 17+ — Breadth` heading with a completed
`## Phase 17 — Minimal IP/UDP stack: DHCP-learn-our-IP  *(done — 2026-06-29)*`
entry (goal / you-learn / done-when, matching the Phase 16 style and the shipped
behavior — both the ARP and `net: dhcp offered 10.0.2.15` proofs, the IPv4/UDP/DHCP
host-tested layer, the bounded-server driver), and add a fresh
`## Phase 18+ — Breadth` tail carrying the remaining long tail (complete the DHCP
lease + adopt the IP; DNS over the same UDP layer; ICMP echo/ping; RX as an
ongoing service; encrypt UDP payloads with the Phase 14 channel; epoch revocation
+ CDT; per-component crash ledgers; growable records; board boot 4c; more
devices/HAL).

- [ ] **Step 3: Update the glossary**

In `docs/glossary.md`, after the existing `ARP` / `SLIRP` entries, add concise
entries for **IPv4** (the 20-byte header + RFC 1071 checksum), **UDP**
(connectionless datagrams over IPv4, addressed by port; checksum optional in
IPv4), and **DHCP** (how a host learns its IP — DISCOVER/OFFER over UDP 68/67;
Phase 17 reads the offered `10.0.2.15` from SLIRP's built-in server).

- [ ] **Step 4: Add the spec implementation note**

In the spec, add a short `## Implementation note (2026-06-29, during build)`
recording what shipped (the IPv4/UDP/DHCP host-tested layer, the bounded-server
driver + `NET_DONE`, the ARP exchange kept, UDP checksum 0, and whether SLIRP
answered DISCOVER directly or the TX-only fallback was needed).

- [ ] **Step 5: Verify references**

Run: `pwsh tools/check-references.ps1`
Expected: PASS. Fix any dangling reference before committing.

- [ ] **Step 6: Commit**

```bash
git add docs/
git commit -m "docs: Phase 17 IP/UDP/DHCP — learning note 0035, roadmap, glossary; spec impl note"
```

---

## Self-review notes

- **Spec coverage:** `ipv4` (Task 1), `udp` (Task 2), `dhcp` (Task 3), bounded-server driver + `NET_DONE` (Task 4), `net_client` ARP+DHCP exchange + spawn rename (Task 5), host+boot verification incl. the new assertion (Task 6), docs (Task 7). UDP-checksum-0, broadcast flag, source-IP-stays-hardcoded, no MAX_TASKS change — all encoded.
- **Type/name consistency:** `ipv4::{checksum, build_header, IPV4_HDR_LEN, PROTO_UDP, ETHERTYPE_IPV4}`, `udp::{build, parse, UDP_HDR_LEN}`, `dhcp::{build_discover, parse_offer, CLIENT_PORT, SERVER_PORT, DISCOVER_LEN}`, `NET_DONE`, `net_client`, `net_component` used identically across tasks. The driver/client call/reply contract: client `call_message(NET_EP_CAP, 12+frame_len)` then `call_message(NET_EP_CAP, NET_DONE)` ↔ driver `sys_recv` badge = TX length or `NET_DONE`. DMA offsets (RX 0x400, TX 0xC00, both frames at +12) consistent with Phase 16.
- **TDD vs integration:** Tasks 1–3 are strict red-green host tests (the bulk of the new logic). Tasks 4–5 are device/kernel integration proven by the boot smoke (Task 6), consistent with how every device phase in this repo is verified.
- **U-mode safety:** the driver (Task 4) uses only `while` loops and `#[inline(always)]` helpers — no iterators in `.user_text`. The pure `net` logic (Tasks 1–3) runs kernel-side in `net_client`, so its `iter_mut`/`match`/slices are fine.
```
