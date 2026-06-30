# Phase 20 — Reply to an inbound ping Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove the OS answers an ICMP Echo Request addressed to it — a self-ping loops through SLIRP back to the guest, which replies with a correct Echo Reply.

**Architecture:** Pure responder logic (`is_echo_request` + `build_echo_reply`) joins `kernel_common::net::icmp`, reusing `ipv4::checksum`. `net_client` adds a self-ping loopback exchange after the gateway ping, through the unchanged bounded-server driver (no persistent listener, no new task). The boot smoke is the spike for whether SLIRP loops the self-ping back; a synthetic self-demo is the documented fallback.

**Tech Stack:** Rust `no_std`, RISC-V64, QEMU `virt` + SLIRP, the IPv4/ICMP layer (Phases 17/19), virtio-net (Phase 16).

## Global Constraints

- **Target/scope:** QEMU `virt` riscv64 only; no board. Unchanged QEMU flags.
- **Reuse, don't reinvent:** the responder reuses `ipv4::{build_header, checksum, IPV4_HDR_LEN}` and the Phase 19 `icmp` consts. No change to `ipv4`/`udp`/`dhcp` or the Phase 19 `build_echo_request`/`parse_echo_reply`.
- **Wire format:** big-endian. The reply swaps Ethernet src/dst MAC and IPv4 src/dst, sets ICMP type 0, keeps code/ident/seq/payload, recomputes both checksums. Self-ping: `ident = 0x4321`, `seq = 0`, `payload = b"loopback"`, src IP = dst IP = `NET_IP`, dst MAC = `gw_mac`.
- **No driver/lifecycle change:** the `net_component` driver is untouched (bounded server, exits on `NET_DONE`); no persistent listener; no new task; no MAX_TASKS change. ~Six driver exchanges total.
- **RX/TX buffers don't overlap** (`rx_frame` at `0x400+12`, `tx_frame` at `0xC00+12`), so the reply is built from the RX frame into the TX frame without aliasing.
- **Spike + fallback:** the boot smoke reveals whether SLIRP loops the self-ping back. If yes → assert `net: replied to inbound ping (seq 0)`. If no → ship the synthetic self-demo (`net: replied to inbound ping (self-demo)`, the responder real, the trigger fabricated) and record the deviation, mirroring Phase 9.
- **Done-when (whole phase):** `./tools/test-qemu.ps1` shows `net: replied to inbound ping (seq 0)` (or the self-demo fallback) after the existing DHCP/ARP/ping lines, with the cross-boot self-healing demo still passing and `cargo test` green.

## File structure

- `libs/common/src/net.rs` — add `is_echo_request` + `build_echo_reply` to the `icmp` module + host tests (Task 1).
- `kernel/src/main.rs` — add the self-ping loopback exchange to `net_client` (Task 2).
- `tools/test-qemu.ps1` — add the inbound-ping assertion (Task 3).
- Docs (Task 4): `docs/learning/0038-*.md`, roadmap, glossary, spec impl note.

---

### Task 1: `icmp` responder — `is_echo_request` + `build_echo_reply`

**Files:**
- Modify: `libs/common/src/net.rs` — add two functions to the `icmp` module (after `parse_echo_reply`); add tests in `mod tests`.

**Interfaces:**
- Consumes: `ipv4::{build_header, checksum, IPV4_HDR_LEN, ETHERTYPE_IPV4}`, the `icmp` consts (`PROTO_ICMP`, `ICMP_ECHO_REQUEST`, `ICMP_ECHO_REPLY`, `ICMP_HDR_LEN`), `super::{be16, ETH_HDR_LEN}` — all existing.
- Produces: `icmp::is_echo_request(frame: &[u8], our_ip: [u8;4]) -> bool`; `icmp::build_echo_reply(request: &[u8], out: &mut [u8]) -> Option<usize>`. Consumed by Task 2.

- [ ] **Step 1: Write the failing tests**

In `libs/common/src/net.rs`, in `mod tests`, add:
```rust
    #[test]
    fn icmp_build_echo_reply_swaps_and_echoes() {
        let our_mac = [0x52u8, 0x54, 0x00, 0x12, 0x34, 0x56];
        let peer_mac = [0x52u8, 0x55, 0x0a, 0x00, 0x02, 0x02];
        let our_ip = [10u8, 0, 2, 15];
        let peer_ip = [10u8, 0, 2, 2];
        let payload = b"loopback";
        // A request the peer sent us: peer_mac/peer_ip -> our_mac/our_ip.
        let mut req = [0u8; 128];
        let n = icmp::build_echo_request(&peer_mac, &our_mac, peer_ip, our_ip, 0x4321, 7, payload, &mut req);
        assert!(icmp::is_echo_request(&req[..n], our_ip), "request addressed to us");
        assert!(!icmp::is_echo_request(&req[..n], [9, 9, 9, 9]), "not addressed to 9.9.9.9");
        // Build the reply.
        let mut reply = [0u8; 128];
        let m = icmp::build_echo_reply(&req[..n], &mut reply).unwrap();
        assert_eq!(m, n, "same length (payload echoed)");
        // Ethernet swapped: reply dst = request src (peer), reply src = request dst (us).
        assert_eq!(&reply[0..6], &peer_mac, "reply dst mac = requester");
        assert_eq!(&reply[6..12], &our_mac, "reply src mac = us");
        // IPv4 swapped (src at 26..30, dst at 30..34) and checksums verify.
        assert_eq!(&reply[26..30], &our_ip, "reply src ip = us");
        assert_eq!(&reply[30..34], &peer_ip, "reply dst ip = requester");
        assert_eq!(ipv4::checksum(&reply[14..34]), 0, "ipv4 header verifies");
        // ICMP: type 0, payload echoed, checksum verifies.
        assert_eq!(reply[34], icmp::ICMP_ECHO_REPLY, "echo reply");
        assert_eq!(&reply[42..42 + payload.len()], payload, "payload echoed");
        assert_eq!(ipv4::checksum(&reply[34..m]), 0, "icmp checksum verifies");
        // The reply is NOT an echo request.
        assert!(!icmp::is_echo_request(&reply[..m], peer_ip));
    }

    #[test]
    fn icmp_build_echo_reply_rejects_non_request() {
        // A non-ICMP frame yields None.
        let mut frame = [0u8; 64];
        frame[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
        frame[14] = 0x45;
        frame[14 + 9] = 17; // UDP
        let mut out = [0u8; 64];
        assert!(icmp::build_echo_reply(&frame, &mut out).is_none());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p kernel-common icmp_build_echo_reply`
Expected: FAIL — `is_echo_request` / `build_echo_reply` not found.

- [ ] **Step 3: Implement the responder**

In `libs/common/src/net.rs`, in the `icmp` module, after `parse_echo_reply`
(before the module's closing `}`), add:
```rust
    /// True iff `frame` is an IPv4/ICMP Echo Request (type 8) whose destination
    /// IP is `our_ip`.
    pub fn is_echo_request(frame: &[u8], our_ip: [u8; 4]) -> bool {
        let eth = super::ETH_HDR_LEN;
        if frame.len() < eth + ipv4::IPV4_HDR_LEN + ICMP_HDR_LEN {
            return false;
        }
        if super::be16(&frame[12..14]) != ipv4::ETHERTYPE_IPV4 {
            return false;
        }
        let ihl = (frame[eth] & 0x0f) as usize * 4;
        if ihl < ipv4::IPV4_HDR_LEN || frame.len() < eth + ihl + ICMP_HDR_LEN {
            return false;
        }
        if frame[eth + 9] != PROTO_ICMP || frame[eth + 16..eth + 20] != our_ip {
            return false;
        }
        frame[eth + ihl] == ICMP_ECHO_REQUEST
    }

    /// Given a received ICMP Echo Request `request`, build the Echo Reply into
    /// `out`: swap Ethernet src/dst MAC, swap IPv4 src/dst, set ICMP type 0, keep
    /// code/identifier/sequence/payload, recompute both checksums. Returns the
    /// reply length, or `None` if `request` is too short / not an ICMP echo.
    pub fn build_echo_reply(request: &[u8], out: &mut [u8]) -> Option<usize> {
        let eth = super::ETH_HDR_LEN;
        if request.len() < eth + ipv4::IPV4_HDR_LEN + ICMP_HDR_LEN {
            return None;
        }
        if super::be16(&request[12..14]) != ipv4::ETHERTYPE_IPV4 || request[eth + 9] != PROTO_ICMP {
            return None;
        }
        let ihl = (request[eth] & 0x0f) as usize * 4;
        if ihl < ipv4::IPV4_HDR_LEN || request.len() < eth + ihl + ICMP_HDR_LEN {
            return None;
        }
        let total = request.len();
        // Ethernet: dst = request's src, src = request's dst.
        out[0..6].copy_from_slice(&request[6..12]);
        out[6..12].copy_from_slice(&request[0..6]);
        out[12..14].copy_from_slice(&ipv4::ETHERTYPE_IPV4.to_be_bytes());
        // IPv4: rebuild with src/dst swapped (covers the same ICMP length).
        let mut src_ip = [0u8; 4];
        src_ip.copy_from_slice(&request[eth + 16..eth + 20]); // request's dst -> our src
        let mut dst_ip = [0u8; 4];
        dst_ip.copy_from_slice(&request[eth + 12..eth + 16]); // request's src -> our dst
        let icmp_len = total - eth - ihl;
        ipv4::build_header(src_ip, dst_ip, PROTO_ICMP, icmp_len, 0, &mut out[eth..eth + ipv4::IPV4_HDR_LEN]);
        // ICMP: copy the message, flip type to reply, recompute checksum.
        let c = eth + ipv4::IPV4_HDR_LEN;
        let rc = eth + ihl;
        out[c..c + icmp_len].copy_from_slice(&request[rc..rc + icmp_len]);
        out[c] = ICMP_ECHO_REPLY;
        out[c + 1] = 0; // code
        out[c + 2..c + 4].copy_from_slice(&0u16.to_be_bytes());
        let csum = ipv4::checksum(&out[c..c + icmp_len]);
        out[c + 2..c + 4].copy_from_slice(&csum.to_be_bytes());
        Some(c + icmp_len)
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p kernel-common icmp_`
Expected: PASS — the two new tests plus the Phase 19 `icmp_*` tests.

- [ ] **Step 5: Commit**

```bash
git add libs/common/src/net.rs
git commit -m "feat(net): ICMP echo responder — is_echo_request + build_echo_reply (host-tested)"
```

---

### Task 2: Self-ping loopback in `net_client`

**Files:**
- Modify: `kernel/src/main.rs` — in `net_client`, insert the self-ping block after the gateway-ping block (between the `match gw_mac { … }` ping block and the `// --- tell the driver to exit ---` comment).

**Interfaces:**
- Consumes: `icmp::{build_echo_request, is_echo_request, build_echo_reply}` (Task 1 + Phase 19); `NET_IP`, `NET_EP_CAP`, `gw_mac` (in scope), `sched::call_message`.
- Produces: the self-ping exchange + its log line.

- [ ] **Step 1: Insert the self-ping loopback block**

In `kernel/src/main.rs`, in `net_client`, find the `// --- tell the driver to exit ---`
comment (with its `let _ = sched::call_message(NET_EP_CAP, NET_DONE);`) and insert
immediately BEFORE it:
```rust
            // --- inbound ping: self-ping our own IP; SLIRP loops it back as an
            // inbound echo REQUEST, which we answer with an echo REPLY ---
            match gw_mac {
                Some(mac) => {
                    let ident = 0x4321u16;
                    let seq = 0u16;
                    // Send an echo request addressed to ourselves (via the gateway
                    // MAC, so SLIRP receives and, if it routes self-addressed
                    // packets back, returns it to us as an inbound request).
                    let req_len = {
                        let txf = core::slice::from_raw_parts_mut(tx_frame as *mut u8, 512);
                        net::icmp::build_echo_request(&src_mac, &mac, NET_IP, NET_IP, ident, seq, b"loopback", txf)
                    };
                    let rx_len = sched::call_message(NET_EP_CAP, 12 + req_len);
                    let mut handled = false;
                    if rx_len != 0 {
                        let flen = rx_len.saturating_sub(12).min(2036);
                        let rxf = core::slice::from_raw_parts(rx_frame as *const u8, flen);
                        if net::icmp::is_echo_request(rxf, NET_IP) {
                            let txf = core::slice::from_raw_parts_mut(tx_frame as *mut u8, 512);
                            if let Some(reply_len) = net::icmp::build_echo_reply(rxf, txf) {
                                let _ = sched::call_message(NET_EP_CAP, 12 + reply_len);
                                println!("net: replied to inbound ping (seq {})", seq);
                                handled = true;
                            }
                        }
                    }
                    if !handled {
                        // SLIRP did not loop the self-ping back. Prove the responder
                        // on a synthesized inbound request instead (the responder is
                        // real; only the trigger is fabricated — see the spec).
                        let mut fake = [0u8; 64];
                        let fake_len =
                            net::icmp::build_echo_request(&mac, &src_mac, [10, 0, 2, 2], NET_IP, ident, seq, b"selfdemo", &mut fake);
                        let mut reply = [0u8; 64];
                        if net::icmp::is_echo_request(&fake[..fake_len], NET_IP)
                            && net::icmp::build_echo_reply(&fake[..fake_len], &mut reply).is_some()
                        {
                            println!("net: replied to inbound ping (self-demo)");
                        } else {
                            println!("net: no inbound ping");
                        }
                    }
                }
                None => println!("net: inbound ping skipped (no gateway MAC)"),
            }

```
(Keep the existing `// --- tell the driver to exit ---` block right after it.)

- [ ] **Step 2: Build the kernel**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: builds clean (no warnings — `is_echo_request`/`build_echo_reply` used).

- [ ] **Step 3: Commit**

```bash
git add kernel/src/main.rs
git commit -m "feat(net): net_client self-pings its own IP and replies to the looped-back inbound request"
```

---

### Task 3: Verify — host tests + boot smoke (the spike)

**Files:**
- Modify: `tools/test-qemu.ps1` (add the inbound-ping assertion, matching the observed outcome).

- [ ] **Step 1: Host tests**

Run: `cargo test`
Expected: PASS — all `kernel_common::net` tests including the new responder tests.

- [ ] **Step 2: Boot smoke WITHOUT a new assertion — observe the loopback outcome**

Run: `./tools/test-qemu.ps1` (passes on existing assertions). Then inspect the log:
```powershell
$s = Join-Path ([System.IO.Path]::GetTempPath()) "kernel-qemu-serial.log"
Select-String -Path $s -Pattern "net: replied to inbound ping|net: no inbound ping" | ForEach-Object { $_.Line }
```
Expected: one of
- `net: replied to inbound ping (seq 0)` → SLIRP looped the self-ping back; real round-trip. Go to Step 3a.
- `net: replied to inbound ping (self-demo)` → SLIRP did not loop it; the synthetic fallback ran. Go to Step 3b.

- [ ] **Step 3a: (loopback path) Assert the real reply**

In `tools/test-qemu.ps1`, in `$mustMatch1`, after the
`"net: ping 10.0.2.2: reply \(seq 0\)",` line, add:
```powershell
    "net: replied to inbound ping \(seq 0\)",
```

- [ ] **Step 3b: (self-demo path — ONLY if Step 2 showed "self-demo")**

If SLIRP did not loop the self-ping, assert the self-demo line instead — in
`$mustMatch1`, after the `"net: ping 10.0.2.2: reply \(seq 0\)",` line, add:
```powershell
    "net: replied to inbound ping \(self-demo\)",
```
and note in the commit + docs that the shipped proof is the synthetic self-demo (a
documented degraded outcome like Phase 9: the responder is real, the trigger
fabricated, because SLIRP user-networking delivers no inbound ICMP).

- [ ] **Step 4: Re-run the boot smoke with the assertion**

Run: `./tools/test-qemu.ps1`
Expected: PASS — the serial log shows the asserted inbound-ping line plus the
existing DHCP/ARP/ping lines, and both cross-boot self-healing boots stay green.

- [ ] **Step 5: Commit the test change**

```bash
git add tools/test-qemu.ps1
git commit -m "test: assert the inbound-ping reply (real loopback, or self-demo fallback)"
```

---

### Task 4: Documentation

**Files:**
- Create: `docs/learning/0038-inbound-ping-reply.md`
- Modify: `docs/roadmap/roadmap.md` (turn "Phase 20+ — Breadth" into a completed Phase 20 entry + a fresh Phase 21+ tail)
- Modify: `docs/glossary.md` (extend the ICMP entry: the OS now also replies to echo requests)
- Modify: `docs/superpowers/specs/2026-06-29-phase-20-inbound-ping-reply-design.md` (impl note with the spike outcome)

- [ ] **Step 1: Write learning note 0038**

Create `docs/learning/0038-inbound-ping-reply.md` — short (summary, not tutorial).
Cover: that replying is the mirror of requesting — `build_echo_reply` swaps the
Ethernet MACs and IPv4 addresses, flips the ICMP type to 0, and recomputes both
checksums (reusing `ipv4::checksum`); the testability constraint (SLIRP
user-networking forwards no inbound ICMP, so a real external host can't ping the
guest) and the **self-ping loopback** trick (ping our own IP, let SLIRP route it
back) that turns it into a real round-trip; the actual spike outcome (real
loopback vs the synthetic self-demo fallback, Phase 9 precedent); and that this
stayed inside the bounded-server driver (no persistent listener). Note the
deferral: a forever-listening ping responder, external pings, and ICMP error
types.

- [ ] **Step 2: Update the roadmap**

Replace the `## Phase 20+ — Breadth` heading with a completed
`## Phase 20 — Reply to an inbound ping  *(done — 2026-06-29)*` entry (goal /
you-learn / done-when matching the Phase 19 style and the shipped proof line —
note whether real loopback or self-demo), and add a fresh `## Phase 21+ — Breadth`
tail carrying the remaining long tail (a persistent ping responder / RX as a
long-lived service; external pings via a non-SLIRP backend; ICMP error replies;
DNS over the UDP layer; DHCP renewal/expiry + netmask/router/DNS options;
encrypting traffic with the Phase 14 channel; epoch revocation + CDT; per-component
crash ledgers; growable records; board boot 4c; more devices/HAL).

- [ ] **Step 3: Update the glossary**

In `docs/glossary.md`, extend the **ICMP** entry to note that as of Phase 20 the OS
also **replies** to an inbound Echo Request (build the reply by swapping
addresses + flipping the type), demonstrated via a self-ping looped through SLIRP.

- [ ] **Step 4: Add the spec implementation note**

In the spec, add a short `## Implementation note (2026-06-29, during build)`
recording what shipped: the `is_echo_request`/`build_echo_reply` responder, the
self-ping exchange, and the **spike outcome** — whether SLIRP looped the self-ping
back (real round-trip) or the synthetic self-demo fallback was used.

- [ ] **Step 5: Verify references**

Run: `pwsh tools/check-references.ps1`
Expected: PASS. Fix any dangling reference before committing.

- [ ] **Step 6: Commit**

```bash
git add docs/
git commit -m "docs: Phase 20 inbound-ping reply — learning note 0038, roadmap, glossary; spec impl note"
```

---

## Self-review notes

- **Spec coverage:** `is_echo_request` + `build_echo_reply` reusing `ipv4` (Task 1), the self-ping loopback exchange + synthetic fallback in `net_client` (Task 2), host+boot verification as the spike with loopback/self-demo branches (Task 3), docs incl. the spike outcome (Task 4). The no-driver-change, no-MAX_TASKS, self-ping params (`ident=0x4321`/`seq=0`/`payload="loopback"`), and the RX→TX no-aliasing point are all encoded.
- **Type/name consistency:** `icmp::{is_echo_request, build_echo_reply, build_echo_request, ICMP_ECHO_REPLY, ICMP_HDR_LEN, PROTO_ICMP}`, `NET_IP`, `gw_mac`, the proof strings `net: replied to inbound ping (seq 0)` / `(self-demo)` consistent across tasks. `build_echo_reply` reuses `ipv4::build_header(.., PROTO_ICMP, ..)` + `ipv4::checksum`; offsets (IPv4 src 26..30 / dst 30..34, ICMP type 34, payload 42..) match the 14+20-byte header layout used in Task 1's test.
- **TDD vs integration:** Task 1 is strict red-green host tests; Tasks 2–3 are kernel integration where the boot smoke is the spike (loopback vs synthetic) — consistent with the repo and the spec's spike-as-boot-smoke decision.
```
