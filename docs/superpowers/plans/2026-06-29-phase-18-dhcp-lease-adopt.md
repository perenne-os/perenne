# Phase 18 — Complete the DHCP lease & adopt the IP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish the DHCP handshake (DISCOVER→OFFER→REQUEST→ACK) and make the leased address the stack's source IP (a kernel `NET_IP` static, retiring the hardcoded constant), used in a post-lease gateway ARP.

**Architecture:** Pure DHCP logic grows in `kernel_common::net::dhcp` (host-tested, TDD): an `Offer { yiaddr, server_id }`, `build_request`, `parse_ack`, and a TLV `option` helper. The kernel `net_client` is rewritten to lease, adopt into `NET_IP`, then ARP the gateway from `NET_IP`, all through the unchanged Phase 16/17 bounded-server driver.

**Tech Stack:** Rust `no_std`, RISC-V64, QEMU `virt` + SLIRP, virtio-net (Phase 16), the IPv4/UDP/DHCP layer (Phase 17).

## Global Constraints

- **Target/scope:** QEMU `virt` riscv64 only; no board. Unchanged QEMU flags.
- **U-mode codegen rule:** unchanged and not touched — the `net_component` driver is unchanged this phase. All new logic is kernel-side (`net_client`) or pure (`net::dhcp`), so iterators/`match`/slices are fine.
- **Wire format:** big-endian. REQUEST is broadcast like DISCOVER (broadcast flag, src `0.0.0.0`, dst `255.255.255.255`, UDP 68→67) plus option 50 (requested IP) and option 54 (server id). UDP checksum stays 0.
- **`REQUEST_LEN = 236 + 4 + 3 + 6 + 6 + 1 = 256`.**
- **Adoption:** `static mut NET_IP: [u8;4] = [0,0,0,0]` is the sole source of the send-path source IP; set on DHCPACK. The hardcoded `[10,0,2,15]` source constant in `net_client` is removed.
- **No new task / no MAX_TASKS change.** The flow is reordered: lease first, then ARP.
- **Done-when (whole phase):** `./tools/test-qemu.ps1` shows `net: dhcp offered 10.0.2.15`, `net: dhcp leased 10.0.2.15 (ack)`, `net: adopted ip 10.0.2.15`, and `net: resolved 10.0.2.2 -> 52:55:0a:00:02:02 (src 10.0.2.15)`, with the cross-boot self-healing demo still passing and `cargo test` green.

## File structure

- `libs/common/src/net.rs` — extend `dhcp`: `Offer`, `option`, `is_reply`, `parse_offer` (now `Option<Offer>`), `build_request`, `parse_ack`, `REQUEST_LEN`; remove `msg_type_is`. Update the two existing `dhcp_*` tests; add new ones (Tasks 1–2).
- `kernel/src/main.rs` — add `static mut NET_IP`; rewrite `net_client` + a `dhcp_round` helper (Task 3).
- `tools/test-qemu.ps1` — add the three new assertion lines (Task 4).
- Docs (Task 5): `docs/learning/0036-*.md`, roadmap, glossary, spec impl note.

---

### Task 1: `dhcp` options refactor + `Offer` struct

**Files:**
- Modify: `libs/common/src/net.rs` — the `dhcp` module (the message-type consts area, the `parse_offer` fn ~lines 223–238, and `msg_type_is` ~lines 240–257); update the test `dhcp_discover_then_offer_roundtrip` (~line 372).

**Interfaces:**
- Produces: `dhcp::Offer { yiaddr: [u8;4], server_id: [u8;4] }` (derives `Clone, Copy, Debug, PartialEq, Eq`); `dhcp::parse_offer(&[u8], u32) -> Option<Offer>`; internal `option(&[u8], u8) -> Option<&[u8]>`, `is_reply(&[u8], u32, u8) -> bool`; consts `MSG_REQUEST = 3`, `MSG_ACK = 5`. Consumed by Tasks 2 and 3.

- [ ] **Step 1: Update the existing roundtrip test to the new return type**

In `libs/common/src/net.rs`, in `dhcp_discover_then_offer_roundtrip`, replace:
```rust
        assert_eq!(dhcp::parse_offer(&offer, xid), Some([10, 0, 2, 15]));
```
with:
```rust
        assert_eq!(dhcp::parse_offer(&offer, xid).map(|o| o.yiaddr), Some([10, 0, 2, 15]));
```

- [ ] **Step 2: Run the test to verify it fails to compile**

Run: `cargo test -p kernel-common dhcp_discover_then_offer_roundtrip`
Expected: FAIL — `parse_offer` still returns `Option<[u8;4]>` which has no `.map(|o| o.yiaddr)` field `yiaddr` (type error), confirming the signature must change.

- [ ] **Step 3: Add message-type consts**

In the `dhcp` module, after `const MSG_OFFER: u8 = 2;`, add:
```rust
    const MSG_REQUEST: u8 = 3;
    const MSG_ACK: u8 = 5;
```

- [ ] **Step 4: Add the `Offer` struct**

In the `dhcp` module, after the `DISCOVER_LEN` const, add:
```rust
    /// What a DHCPOFFER tells us: the offered address and the server identifier
    /// (option 54) that a REQUEST must echo so the right server commits the lease.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Offer {
        pub yiaddr: [u8; 4],
        pub server_id: [u8; 4],
    }
```

- [ ] **Step 5: Replace `parse_offer` and `msg_type_is` with the option-based version**

In the `dhcp` module, replace the whole `parse_offer` function and the whole
`msg_type_is` function (the current lines 223–257) with:
```rust
    /// If `payload` is a DHCPOFFER (BOOTREPLY, our `xid`, magic cookie, message
    /// type OFFER), return the offered address and server id. `None` otherwise.
    pub fn parse_offer(payload: &[u8], xid: u32) -> Option<Offer> {
        if !is_reply(payload, xid, MSG_OFFER) {
            return None;
        }
        let mut yiaddr = [0u8; 4];
        yiaddr.copy_from_slice(&payload[16..20]);
        let mut server_id = [0u8; 4];
        if let Some(s) = option(&payload[240..], 54) {
            if s.len() >= 4 {
                server_id.copy_from_slice(&s[..4]);
            }
        }
        Some(Offer { yiaddr, server_id })
    }

    /// Common BOOTREPLY guard: length, op = reply, our `xid`, magic cookie, and
    /// DHCP message type (option 53) == `msg_type`.
    fn is_reply(payload: &[u8], xid: u32, msg_type: u8) -> bool {
        payload.len() >= 240
            && payload[0] == OP_REPLY
            && payload[4..8] == xid.to_be_bytes()
            && payload[236..240] == MAGIC
            && option(&payload[240..], 53).and_then(|v| v.first()).copied() == Some(msg_type)
    }

    /// Walk the TLV option area for `code`, returning its value bytes. `0` = pad,
    /// `255` = end; every other option is `code, len, value…`.
    fn option(opts: &[u8], code: u8) -> Option<&[u8]> {
        let mut i = 0;
        while i < opts.len() {
            match opts[i] {
                0 => i += 1,
                255 => return None,
                c => {
                    if i + 1 >= opts.len() {
                        return None;
                    }
                    let len = opts[i + 1] as usize;
                    if i + 2 + len > opts.len() {
                        return None;
                    }
                    if c == code {
                        return Some(&opts[i + 2..i + 2 + len]);
                    }
                    i += 2 + len;
                }
            }
        }
        None
    }
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p kernel-common dhcp_`
Expected: PASS — `dhcp_discover_then_offer_roundtrip` and `dhcp_parse_offer_rejects_mismatches` both green (the `.is_none()` assertions are unaffected by the return-type change).

- [ ] **Step 7: Commit**

```bash
git add libs/common/src/net.rs
git commit -m "refactor(net): dhcp Offer struct + TLV option helper (parse_offer returns yiaddr + server_id)"
```

---

### Task 2: `build_request` + `parse_ack`

**Files:**
- Modify: `libs/common/src/net.rs` — add to the `dhcp` module; add tests in `mod tests`.

**Interfaces:**
- Consumes: `Offer`, `option`, `is_reply`, `MSG_REQUEST`, `MSG_ACK` (Task 1).
- Produces: `dhcp::REQUEST_LEN: usize` (256); `dhcp::build_request(xid:u32, client_mac:&[u8;6], requested_ip:[u8;4], server_id:[u8;4], out:&mut [u8]) -> usize`; `dhcp::parse_ack(payload:&[u8], xid:u32) -> Option<[u8;4]>`. Consumed by Task 3.

- [ ] **Step 1: Write the failing tests**

In `mod tests`, add:
```rust
    #[test]
    fn dhcp_request_build_then_reparse() {
        let mac = [0x52u8, 0x54, 0x00, 0x12, 0x34, 0x56];
        let xid = 0x1234_5678u32;
        let mut req = [0u8; dhcp::REQUEST_LEN];
        let n = dhcp::build_request(xid, &mac, [10, 0, 2, 15], [10, 0, 2, 2], &mut req);
        assert_eq!(n, dhcp::REQUEST_LEN);
        assert_eq!(req[0], 1, "BOOTREQUEST");
        assert_eq!(&req[4..8], &xid.to_be_bytes());
        assert_eq!(&req[236..240], &[0x63, 0x82, 0x53, 0x63], "magic cookie");
        // Options: 53=REQUEST(3), 50=requested IP, 54=server id, end.
        assert_eq!(&req[240..243], &[53, 1, 3], "msg type = REQUEST");
        assert_eq!(&req[243..249], &[50, 4, 10, 0, 2, 15], "requested IP");
        assert_eq!(&req[249..255], &[54, 4, 10, 0, 2, 2], "server id");
        assert_eq!(req[255], 255, "end");
    }

    #[test]
    fn dhcp_parse_ack_returns_address() {
        let mac = [0x52u8, 0x54, 0x00, 0x12, 0x34, 0x56];
        let xid = 0x1234_5678u32;
        // Start from a REQUEST, turn it into the ACK the server would send.
        let mut ack = [0u8; dhcp::REQUEST_LEN];
        dhcp::build_request(xid, &mac, [10, 0, 2, 15], [10, 0, 2, 2], &mut ack);
        ack[0] = 2; // BOOTREPLY
        ack[16..20].copy_from_slice(&[10, 0, 2, 15]); // yiaddr
        ack[242] = 5; // option 53 value = ACK
        assert_eq!(dhcp::parse_ack(&ack, xid), Some([10, 0, 2, 15]));
        // Rejections.
        assert!(dhcp::parse_ack(&ack, 0x9999_9999).is_none(), "wrong xid");
        let mut not_ack = ack;
        not_ack[242] = 2; // OFFER, not ACK
        assert!(dhcp::parse_ack(&not_ack, xid).is_none(), "not an ack");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p kernel-common 'dhcp_request_build_then_reparse|dhcp_parse_ack'`
Expected: FAIL — `build_request` / `parse_ack` / `REQUEST_LEN` not found.

- [ ] **Step 3: Implement `REQUEST_LEN`, `build_request`, `parse_ack`**

In the `dhcp` module, after `build_discover` (before `parse_offer`), add:
```rust
    /// REQUEST payload: fixed area + cookie(4) + opt 53 (3) + opt 50 (6) +
    /// opt 54 (6) + end (1).
    pub const REQUEST_LEN: usize = BOOTP_FIXED + 4 + 3 + 6 + 6 + 1;

    /// Build a DHCPREQUEST into `out` (>= REQUEST_LEN): broadcast like DISCOVER,
    /// message type REQUEST, with option 50 (requested IP = the offer's `yiaddr`)
    /// and option 54 (server id from the offer). Returns the payload length.
    pub fn build_request(
        xid: u32,
        client_mac: &[u8; 6],
        requested_ip: [u8; 4],
        server_id: [u8; 4],
        out: &mut [u8],
    ) -> usize {
        let p = &mut out[..REQUEST_LEN];
        for b in p.iter_mut() {
            *b = 0;
        }
        p[0] = OP_REQUEST;
        p[1] = 1; // htype = Ethernet
        p[2] = 6; // hlen
        p[4..8].copy_from_slice(&xid.to_be_bytes());
        p[10..12].copy_from_slice(&0x8000u16.to_be_bytes()); // flags: broadcast
        p[28..34].copy_from_slice(client_mac); // chaddr
        p[236..240].copy_from_slice(&MAGIC);
        let mut o = 240;
        p[o] = 53; // DHCP message type
        p[o + 1] = 1;
        p[o + 2] = MSG_REQUEST;
        o += 3;
        p[o] = 50; // requested IP address
        p[o + 1] = 4;
        p[o + 2..o + 6].copy_from_slice(&requested_ip);
        o += 6;
        p[o] = 54; // server identifier
        p[o + 1] = 4;
        p[o + 2..o + 6].copy_from_slice(&server_id);
        o += 6;
        p[o] = 255; // end
        REQUEST_LEN
    }

    /// If `payload` is a DHCPACK (BOOTREPLY, our `xid`, magic cookie, message type
    /// ACK), return the confirmed address (`yiaddr`). `None` otherwise.
    pub fn parse_ack(payload: &[u8], xid: u32) -> Option<[u8; 4]> {
        if !is_reply(payload, xid, MSG_ACK) {
            return None;
        }
        let mut ip = [0u8; 4];
        ip.copy_from_slice(&payload[16..20]);
        Some(ip)
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p kernel-common dhcp_`
Expected: PASS — all `dhcp_*` tests (the two new + the two from Task 1).

- [ ] **Step 5: Commit**

```bash
git add libs/common/src/net.rs
git commit -m "feat(net): dhcp build_request + parse_ack (complete the DORA handshake, host-tested)"
```

---

### Task 3: Kernel adoption — `NET_IP` + reordered `net_client`

**Files:**
- Modify: `kernel/src/main.rs` — add `static mut NET_IP` near `NET_DMA_PA`; rewrite the `net_client` function (the Phase 17 version, its doc comment through its closing `}`); add a `dhcp_round` helper.

**Interfaces:**
- Consumes: `NET_DMA_PA`, `NET_EP_CAP`, `NET_DONE` (existing); `sched::call_message`; `kernel_common::net::{build_request, parse_reply, ARP_FRAME_LEN, udp, dhcp}` incl. Tasks 1–2.
- Produces: `static mut NET_IP: [u8;4]`; the reordered `net_client`.

- [ ] **Step 1: Add the `NET_IP` static**

In `kernel/src/main.rs`, immediately after the `NET_DMA_PA` static (`static mut NET_DMA_PA: usize = 0;`), add:
```rust
    /// The stack's configured source IPv4 address. `0.0.0.0` (unconfigured) until
    /// a DHCP lease is adopted (Phase 18); the sole source of the send path's
    /// source IP, replacing the former hardcoded constant.
    static mut NET_IP: [u8; 4] = [0, 0, 0, 0];
```

- [ ] **Step 2: Replace `net_client` with the lease+adopt version**

Replace the entire `net_client` function (its doc comment through its closing `}`)
with:
```rust
    /// Build a broadcast DHCP UDP frame around `payload` into the shared TX buffer,
    /// call the net driver to transmit + receive, and return the reply's UDP
    /// payload (the next caller reuses the buffer, so parse the result before the
    /// next round). `None` on no reply / not UDP-to-the-client-port.
    unsafe fn dhcp_round(tx_frame: usize, rx_frame: usize, src_mac: &[u8; 6], payload: &[u8]) -> Option<&'static [u8]> {
        use kernel_common::net;
        let txf = core::slice::from_raw_parts_mut(tx_frame as *mut u8, 512);
        let frame_len = net::udp::build(
            src_mac, &[0xffu8; 6], [0, 0, 0, 0], [255, 255, 255, 255],
            net::dhcp::CLIENT_PORT, net::dhcp::SERVER_PORT, 0, payload, txf,
        );
        let rx_len = sched::call_message(NET_EP_CAP, 12 + frame_len);
        if rx_len == 0 {
            return None;
        }
        let flen = rx_len.saturating_sub(12).min(2036);
        let rxf = core::slice::from_raw_parts(rx_frame as *const u8, flen);
        net::udp::parse(rxf, net::dhcp::CLIENT_PORT)
    }

    /// The kernel client for the user-space `net` driver (Phase 18). It completes
    /// a full DHCP lease (DISCOVER → OFFER → REQUEST → ACK) over the shared DMA
    /// page, ADOPTS the leased address into `NET_IP` (retiring the hardcoded
    /// source constant), then ARP-resolves the gateway using the adopted IP — the
    /// leased address flowing into a real frame. All framing is the host-tested
    /// `kernel_common::net`; the driver only moves bytes. It then tells the driver
    /// to exit (NET_DONE) and idles.
    extern "C" fn net_client() -> ! {
        use kernel_common::net;
        // SAFETY: NET_DMA_PA is the kernel-allocated, identity-mapped DMA frame;
        // single hart owns it. The driver shares the same physical frame RW-U.
        unsafe {
            let dma = NET_DMA_PA;
            let tx_frame = dma + 0xC00 + 12; // after the 12-byte virtio_net_hdr
            let rx_frame = dma + 0x400 + 12;
            let src_mac = [0x52u8, 0x54, 0x00, 0x12, 0x34, 0x56];
            let xid = 0x1234_5678u32;

            // --- DISCOVER -> OFFER ---
            let mut disc = [0u8; net::dhcp::DISCOVER_LEN];
            net::dhcp::build_discover(xid, &src_mac, &mut disc);
            let offer = dhcp_round(tx_frame, rx_frame, &src_mac, &disc)
                .and_then(|p| net::dhcp::parse_offer(p, xid));
            match offer {
                Some(o) => println!("net: dhcp offered {}.{}.{}.{}", o.yiaddr[0], o.yiaddr[1], o.yiaddr[2], o.yiaddr[3]),
                None => println!("net: no dhcp offer"),
            }

            // --- REQUEST -> ACK, then adopt ---
            if let Some(o) = offer {
                let mut req = [0u8; net::dhcp::REQUEST_LEN];
                net::dhcp::build_request(xid, &src_mac, o.yiaddr, o.server_id, &mut req);
                let ack = dhcp_round(tx_frame, rx_frame, &src_mac, &req)
                    .and_then(|p| net::dhcp::parse_ack(p, xid));
                match ack {
                    Some(ip) => {
                        println!("net: dhcp leased {}.{}.{}.{} (ack)", ip[0], ip[1], ip[2], ip[3]);
                        NET_IP = ip; // adopt
                        println!("net: adopted ip {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
                    }
                    None => println!("net: no dhcp ack"),
                }
            }

            // --- ARP the gateway from the adopted source IP ---
            let txf = core::slice::from_raw_parts_mut(tx_frame as *mut u8, net::ARP_FRAME_LEN);
            let arp_len = net::build_request(&src_mac, NET_IP, [10, 0, 2, 2], txf);
            let rx_len = sched::call_message(NET_EP_CAP, 12 + arp_len);
            let mut resolved = false;
            if rx_len != 0 {
                let rxf = core::slice::from_raw_parts(rx_frame as *const u8, net::ARP_FRAME_LEN);
                if let Some(mac) = net::parse_reply(rxf, [10, 0, 2, 2]) {
                    println!(
                        "net: resolved 10.0.2.2 -> {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} (src {}.{}.{}.{})",
                        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5],
                        NET_IP[0], NET_IP[1], NET_IP[2], NET_IP[3]
                    );
                    resolved = true;
                }
            }
            if !resolved {
                println!("net: no ARP reply");
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

- [ ] **Step 3: Build the kernel**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: builds clean. (The spawn site already calls `net_client` from Phase 17 — unchanged. No `NET_IP` unused warning, since `net_client` reads/writes it.)

- [ ] **Step 4: Commit**

```bash
git add kernel/src/main.rs
git commit -m "feat(net): complete the DHCP lease + adopt the IP into NET_IP, ARP the gateway from it"
```

---

### Task 4: Verify — host tests + boot smoke

**Files:**
- Modify: `tools/test-qemu.ps1` (add the three new assertion lines to `$mustMatch1`).

- [ ] **Step 1: Host tests**

Run: `cargo test`
Expected: PASS — all `kernel_common::net` tests including the new/updated `dhcp_*`.

- [ ] **Step 2: Add the lease/adopt assertions to the smoke test**

In `tools/test-qemu.ps1`, in `$mustMatch1`, replace the existing line
`"net: dhcp offered 10.0.2.15",` with:
```powershell
    "net: dhcp offered 10.0.2.15",
    "net: dhcp leased 10.0.2.15 \(ack\)",
    "net: adopted ip 10.0.2.15",
    "net: resolved 10.0.2.2 -> 52:55:0a:00:02:02 \(src 10.0.2.15\)",
```
(The pre-existing `"net: resolved 10.0.2.2 -> 52:55:0a:00:02:02",` line stays — it
still matches as a substring of the new ` (src …)` line.)

- [ ] **Step 3: Boot smoke**

Run: `./tools/test-qemu.ps1`
Expected: PASS — the serial log shows, in order:
```
net: dhcp offered 10.0.2.15
net: dhcp leased 10.0.2.15 (ack)
net: adopted ip 10.0.2.15
net: resolved 10.0.2.2 -> 52:55:0a:00:02:02 (src 10.0.2.15)
```
and both cross-boot self-healing boots stay green.

- [ ] **Step 4: If a line is missing, debug before proceeding**

Do NOT loosen the assertions.
- `net: no dhcp ack` → the REQUEST/ACK round failed. Confirm SLIRP returns an ACK: the REQUEST must carry option 54 (server id) = the OFFER's server id; check `parse_offer` extracted a non-zero `server_id` (SLIRP's is `10.0.2.2`). Temporarily print `o.server_id` in `net_client`.
- `net: no dhcp offer` after Phase 17 worked → the `dhcp_round` helper or the `Offer`-returning `parse_offer`; confirm the OFFER still parses (msg type 2 via the new `option`/`is_reply`).
- `src 0.0.0.0` in the ARP line → adoption didn't run (no ACK) or `NET_IP` wasn't set; verify the ACK branch assigns `NET_IP = ip` before the ARP.
- Per the spec fallback: only if SLIRP genuinely sends no ACK, adopt the OFFER's address instead and record the deviation honestly.

- [ ] **Step 5: Commit the test change**

```bash
git add tools/test-qemu.ps1
git commit -m "test: assert the DHCP lease completion + IP adoption (REQUEST/ACK + src)"
```

---

### Task 5: Documentation

**Files:**
- Create: `docs/learning/0036-dhcp-lease-adopt.md`
- Modify: `docs/roadmap/roadmap.md` (turn "Phase 18+ — Breadth" into a completed Phase 18 entry + a fresh Phase 19+ tail)
- Modify: `docs/glossary.md` (extend the DHCP entry: lease completion + adoption)
- Modify: `docs/superpowers/specs/2026-06-29-phase-18-dhcp-lease-adopt-design.md` (impl note)

- [ ] **Step 1: Write learning note 0036**

Create `docs/learning/0036-dhcp-lease-adopt.md` — short (summary, not tutorial).
Cover: the DORA handshake (DISCOVER/OFFER/REQUEST/ACK) and why the REQUEST must
echo the **server identifier** (option 54) — it selects which server's offer you
accept; the generic TLV `option` helper replacing the one-off message-type scan;
that "adopting" the IP means routing the send path through one `NET_IP` static
instead of a constant (so the OS is configured *by the network*), and that the
post-lease gateway ARP from `NET_IP` is what gives the adopted IP a consumer;
and the honest note that SLIRP leases the same `10.0.2.15` we hardcoded, so
adoption is proven by the plumbing (`0.0.0.0` → leased), not a different value.
Note the deferral: no renewal/expiry timers, no netmask/router/DNS options.

- [ ] **Step 2: Update the roadmap**

Replace the `## Phase 18+ — Breadth` heading with a completed
`## Phase 18 — Complete the DHCP lease & adopt the IP  *(done — 2026-06-29)*`
entry (goal / you-learn / done-when, matching the Phase 17 style and the shipped
proof lines), and add a fresh `## Phase 19+ — Breadth` tail carrying the remaining
long tail (lease renewal/expiry; adopt netmask/router/DNS options; DNS resolution
and ICMP echo over the configured stack; RX as an ongoing service; encrypt UDP
payloads with the Phase 14 channel; epoch revocation + CDT; per-component crash
ledgers; growable records; board boot 4c; more devices/HAL).

- [ ] **Step 3: Update the glossary**

In `docs/glossary.md`, extend the **DHCP** entry to note Phase 18 completes the
four-message lease (DISCOVER/OFFER/REQUEST/ACK — the REQUEST echoing the server
identifier) and **adopts** the leased address as the stack's source IP (a kernel
`NET_IP`, retiring the hardcoded constant).

- [ ] **Step 4: Add the spec implementation note**

In the spec, add a short `## Implementation note (2026-06-29, during build)`
recording what shipped (the `Offer` struct + `option`/`is_reply` refactor,
`build_request`/`parse_ack`, the `NET_IP` adoption + reordered flow) and whether
SLIRP returned a DHCPACK directly or the OFFER-address fallback was used.

- [ ] **Step 5: Verify references**

Run: `pwsh tools/check-references.ps1`
Expected: PASS. Fix any dangling reference before committing.

- [ ] **Step 6: Commit**

```bash
git add docs/
git commit -m "docs: Phase 18 DHCP lease + adopt — learning note 0036, roadmap, glossary; spec impl note"
```

---

## Self-review notes

- **Spec coverage:** `Offer`/`option`/`is_reply`/`parse_offer` (Task 1), `build_request`/`parse_ack`/`REQUEST_LEN` (Task 2), `NET_IP` adoption + reordered `net_client` + retiring the hardcoded source (Task 3), host+boot verification with the new assertions (Task 4), docs (Task 5). The reordered flow, no-MAX_TASKS-change, broadcast REQUEST, and UDP-checksum-0 constraints are all encoded.
- **Type/name consistency:** `dhcp::{Offer{yiaddr,server_id}, parse_offer→Option<Offer>, build_request, parse_ack, REQUEST_LEN, CLIENT_PORT, SERVER_PORT, DISCOVER_LEN}`, `MSG_REQUEST=3`/`MSG_ACK=5`, `NET_IP`, `dhcp_round`, `net_client` consistent across tasks. `dhcp_round` returns `Option<&'static [u8]>` consumed by `and_then(parse_offer/parse_ack)` before the buffer is reused. The ARP source is `NET_IP` (not a constant). Proof strings match the spec exactly (`leased … (ack)`, `adopted ip …`, `resolved … (src …)`).
- **TDD vs integration:** Tasks 1–2 are strict red-green host tests; Task 3 is kernel integration proven by the boot smoke (Task 4) — consistent with the repo.
- **Lifetime note:** `dhcp_round`'s `&'static` is sound because `NET_DMA_PA` is a kernel-owned, identity-mapped frame that lives for the whole run; each reply is parsed into owned data (`Offer`/`[u8;4]`) before the next round overwrites the buffer.
```
