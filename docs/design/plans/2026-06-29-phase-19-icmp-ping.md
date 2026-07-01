# Phase 19 — ICMP echo (ping) the gateway Implementation Plan

> **For agentic workers:** Implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Send an ICMP Echo Request to the gateway `10.0.2.2` (from the adopted IP, to the ARP-resolved gateway MAC) and receive the Echo Reply — the OS's first IP round-trip it initiates by address.

**Architecture:** Pure ICMP logic joins `kernel_common::net::icmp`, reusing `ipv4::build_header(proto=1)` and `ipv4::checksum` (ICMP uses the same RFC 1071 checksum). `net_client` captures the gateway MAC it currently discards and adds a fourth exchange (ping) through the unchanged bounded-server driver. The boot smoke is the spike that reveals whether SLIRP answers.

**Tech Stack:** Rust `no_std`, RISC-V64, QEMU `virt` + SLIRP, the IPv4/UDP/DHCP layer (Phases 17/18), virtio-net (Phase 16).

## Global Constraints

- **Target/scope:** QEMU `virt` riscv64 only; no board. Unchanged QEMU flags.
- **Reuse, don't reinvent:** ICMP is IPv4 protocol 1 and its checksum is the existing `ipv4::checksum` (RFC 1071). No new checksum code; no change to `ipv4`/`udp`/`dhcp`.
- **Wire format:** big-endian. ICMP Echo Request = type 8, code 0, checksum, identifier, sequence, payload. Reply = type 0. `ident = 0x1234`, `seq = 0`, `payload = b"kernelOS"`.
- **Ping target:** the gateway `10.0.2.2`, Ethernet dst = the ARP-resolved gateway MAC, IPv4 src = `NET_IP` (the adopted address).
- **Driver/kernel unchanged otherwise:** the `net_component` driver is untouched (it already serves N exchanges); no new task; no MAX_TASKS change. Four driver exchanges total (DISCOVER, REQUEST, ARP, PING).
- **Spike + fallback:** the boot smoke reveals whether SLIRP replies. If it does → assert `net: ping 10.0.2.2: reply (seq 0)`. If it genuinely does not → ship the TX-only proof (`net: ping 10.0.2.2: no reply`, the request transmitted, the framing host-tested) and record the deviation, mirroring Phase 15.
- **Done-when (whole phase):** `./tools/test-qemu.ps1` shows `net: ping 10.0.2.2: reply (seq 0)` (or the documented fallback line) alongside the existing DHCP/ARP lines, with the cross-boot self-healing demo still passing and `cargo test` green.

## File structure

- `libs/common/src/net.rs` — add `pub mod icmp` (after `pub mod dhcp`) + host tests (Task 1).
- `kernel/src/main.rs` — capture `gw_mac` in `net_client`'s ARP block; add the ping step (Task 2).
- `tools/test-qemu.ps1` — add the ping assertion (Task 3).
- Docs (Task 4): `docs/learning/0037-*.md`, roadmap, glossary, spec impl note.

---

### Task 1: `icmp` submodule (build + parse)

**Files:**
- Modify: `libs/common/src/net.rs` — add `pub mod icmp` after the `dhcp` module; add tests in `mod tests`.

**Interfaces:**
- Consumes: `ipv4::{build_header, checksum, IPV4_HDR_LEN, ETHERTYPE_IPV4}`; the private `be16` and `pub const ETH_HDR_LEN` (via `super::`).
- Produces: `icmp::{PROTO_ICMP, ICMP_ECHO_REQUEST, ICMP_ECHO_REPLY, ICMP_HDR_LEN}`; `icmp::build_echo_request(src_mac:&[u8;6], dst_mac:&[u8;6], src_ip:[u8;4], dst_ip:[u8;4], ident:u16, seq:u16, payload:&[u8], frame:&mut [u8]) -> usize`; `icmp::parse_echo_reply(frame:&[u8], ident:u16, seq:u16) -> bool`. Consumed by Task 2.

- [ ] **Step 1: Write the failing tests**

In `libs/common/src/net.rs`, in `mod tests`, add:
```rust
    #[test]
    fn icmp_build_then_parse_reply() {
        let src_mac = [0x52u8, 0x54, 0x00, 0x12, 0x34, 0x56];
        let dst_mac = [0x52u8, 0x55, 0x0a, 0x00, 0x02, 0x02];
        let payload = b"kernelOS";
        let mut frame = [0u8; 128];
        let n = icmp::build_echo_request(&src_mac, &dst_mac, [10, 0, 2, 15], [10, 0, 2, 2], 0x1234, 0, payload, &mut frame);
        // IPv4 header self-verifies; the ICMP message checksums to 0.
        assert_eq!(ipv4::checksum(&frame[14..34]), 0);
        assert_eq!(ipv4::checksum(&frame[34..n]), 0, "icmp checksum verifies");
        // As built it is a request, not a reply.
        assert!(!icmp::parse_echo_reply(&frame[..n], 0x1234, 0));
        // Flip the ICMP type to Echo Reply -> parses; rejects wrong ident/seq.
        let mut reply = frame;
        reply[34] = icmp::ICMP_ECHO_REPLY;
        assert!(icmp::parse_echo_reply(&reply[..n], 0x1234, 0));
        assert!(!icmp::parse_echo_reply(&reply[..n], 0x9999, 0), "wrong ident");
        assert!(!icmp::parse_echo_reply(&reply[..n], 0x1234, 7), "wrong seq");
    }

    #[test]
    fn icmp_parse_reply_rejects_non_icmp() {
        let mut frame = [0u8; 64];
        frame[12..14].copy_from_slice(&0x0800u16.to_be_bytes()); // IPv4
        frame[14] = 0x45; // version 4, IHL 5
        frame[14 + 9] = 17; // protocol UDP, not ICMP
        assert!(!icmp::parse_echo_reply(&frame, 0x1234, 0));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p kernel-common icmp_`
Expected: FAIL — unresolved module `icmp`.

- [ ] **Step 3: Implement the `icmp` submodule**

In `libs/common/src/net.rs`, after the `dhcp` module's closing `}` (before `#[cfg(test)]`), add:
```rust
/// ICMP echo (ping) over IPv4. Reuses `ipv4::build_header` (protocol 1) and
/// `ipv4::checksum` (ICMP uses the same RFC 1071 one's-complement checksum).
pub mod icmp {
    use super::ipv4;
    pub const PROTO_ICMP: u8 = 1;
    pub const ICMP_ECHO_REQUEST: u8 = 8;
    pub const ICMP_ECHO_REPLY: u8 = 0;
    /// ICMP echo header: type(1) + code(1) + checksum(2) + ident(2) + seq(2).
    pub const ICMP_HDR_LEN: usize = 8;

    /// Assemble Ethernet + IPv4 + ICMP Echo Request around `payload` into `frame`.
    /// Returns the total frame length.
    #[allow(clippy::too_many_arguments)]
    pub fn build_echo_request(
        src_mac: &[u8; 6],
        dst_mac: &[u8; 6],
        src_ip: [u8; 4],
        dst_ip: [u8; 4],
        ident: u16,
        seq: u16,
        payload: &[u8],
        frame: &mut [u8],
    ) -> usize {
        // Ethernet header.
        frame[0..6].copy_from_slice(dst_mac);
        frame[6..12].copy_from_slice(src_mac);
        frame[12..14].copy_from_slice(&ipv4::ETHERTYPE_IPV4.to_be_bytes());
        // IPv4 header (covers the ICMP message).
        let icmp_len = ICMP_HDR_LEN + payload.len();
        let ip = super::ETH_HDR_LEN;
        ipv4::build_header(src_ip, dst_ip, PROTO_ICMP, icmp_len, ident, &mut frame[ip..ip + ipv4::IPV4_HDR_LEN]);
        // ICMP echo request.
        let c = ip + ipv4::IPV4_HDR_LEN;
        frame[c] = ICMP_ECHO_REQUEST;
        frame[c + 1] = 0; // code
        frame[c + 2..c + 4].copy_from_slice(&0u16.to_be_bytes()); // checksum: zero, then fill
        frame[c + 4..c + 6].copy_from_slice(&ident.to_be_bytes());
        frame[c + 6..c + 8].copy_from_slice(&seq.to_be_bytes());
        frame[c + 8..c + 8 + payload.len()].copy_from_slice(payload);
        let csum = ipv4::checksum(&frame[c..c + icmp_len]);
        frame[c + 2..c + 4].copy_from_slice(&csum.to_be_bytes());
        c + icmp_len
    }

    /// True iff `frame` is an IPv4/ICMP **Echo Reply** with the matching identifier
    /// and sequence. Lenient on the reply's checksum (we trust the kernel demux).
    pub fn parse_echo_reply(frame: &[u8], ident: u16, seq: u16) -> bool {
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
        if frame[eth + 9] != PROTO_ICMP {
            return false;
        }
        let c = eth + ihl;
        frame[c] == ICMP_ECHO_REPLY
            && super::be16(&frame[c + 4..c + 6]) == ident
            && super::be16(&frame[c + 6..c + 8]) == seq
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p kernel-common icmp_`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add libs/common/src/net.rs
git commit -m "feat(net): ICMP echo request/reply build+parse (reuses ipv4 checksum, host-tested)"
```

---

### Task 2: Wire the ping into `net_client`

**Files:**
- Modify: `kernel/src/main.rs` — the `net_client` ARP block (capture `gw_mac`) and add the ping step before the `NET_DONE` call.

**Interfaces:**
- Consumes: `icmp::{build_echo_request, parse_echo_reply}` (Task 1); `NET_IP`, `NET_EP_CAP`, `NET_DONE`, `sched::call_message` (existing).
- Produces: the ping exchange + its log line.

- [ ] **Step 1: Capture the gateway MAC in the ARP block**

In `kernel/src/main.rs`, in `net_client`, replace the ARP block's result handling
(the `let mut resolved = false;` through the `if !resolved { println!("net: no ARP reply"); }`)
— currently:
```rust
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
```
with (capturing the MAC instead of a bool):
```rust
            let mut gw_mac: Option<[u8; 6]> = None;
            if rx_len != 0 {
                let rxf = core::slice::from_raw_parts(rx_frame as *const u8, net::ARP_FRAME_LEN);
                if let Some(mac) = net::parse_reply(rxf, [10, 0, 2, 2]) {
                    println!(
                        "net: resolved 10.0.2.2 -> {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} (src {}.{}.{}.{})",
                        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5],
                        NET_IP[0], NET_IP[1], NET_IP[2], NET_IP[3]
                    );
                    gw_mac = Some(mac);
                }
            }
            if gw_mac.is_none() {
                println!("net: no ARP reply");
            }
```

- [ ] **Step 2: Add the ping step**

In `net_client`, immediately before the `// --- tell the driver to exit ---`
comment and its `call_message(NET_EP_CAP, NET_DONE)`, insert:
```rust
            // --- PING the gateway (ICMP echo) from the adopted IP ---
            match gw_mac {
                Some(mac) => {
                    let ident = 0x1234u16;
                    let seq = 0u16;
                    let frame_len = {
                        let txf = core::slice::from_raw_parts_mut(tx_frame as *mut u8, 128);
                        net::icmp::build_echo_request(&src_mac, &mac, NET_IP, [10, 0, 2, 2], ident, seq, b"kernelOS", txf)
                    };
                    let rx_len = sched::call_message(NET_EP_CAP, 12 + frame_len);
                    let mut replied = false;
                    if rx_len != 0 {
                        let flen = rx_len.saturating_sub(12).min(2036);
                        let rxf = core::slice::from_raw_parts(rx_frame as *const u8, flen);
                        if net::icmp::parse_echo_reply(rxf, ident, seq) {
                            println!("net: ping 10.0.2.2: reply (seq {})", seq);
                            replied = true;
                        }
                    }
                    if !replied {
                        println!("net: ping 10.0.2.2: no reply");
                    }
                }
                None => println!("net: ping skipped (no gateway MAC)"),
            }
```

- [ ] **Step 3: Build the kernel**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: builds clean (no warnings — `gw_mac`, `icmp` both used).

- [ ] **Step 4: Commit**

```bash
git add kernel/src/main.rs
git commit -m "feat(net): net_client pings the gateway (ICMP echo) from the adopted IP + ARP'd MAC"
```

---

### Task 3: Verify — host tests + boot smoke (the spike)

**Files:**
- Modify: `tools/test-qemu.ps1` (add the ping assertion, matching the observed outcome).

- [ ] **Step 1: Host tests**

Run: `cargo test`
Expected: PASS — all `kernel_common::net` tests including the new `icmp_*`.

- [ ] **Step 2: Boot smoke WITHOUT a new assertion yet — observe what SLIRP does**

Run: `./tools/test-qemu.ps1` (it will pass on the existing assertions; we are
reading the new `net: ping …` line, not yet asserting it). Then inspect the serial
log:
```powershell
$s = Join-Path ([System.IO.Path]::GetTempPath()) "kernel-qemu-serial.log"
Select-String -Path $s -Pattern "net: ping" | ForEach-Object { $_.Line }
```
Expected: one of
- `net: ping 10.0.2.2: reply (seq 0)` → SLIRP answered; proceed to Step 3a.
- `net: ping 10.0.2.2: no reply` → SLIRP did not answer; proceed to Step 3b (fallback).

- [ ] **Step 3a: (reply path) Assert the reply**

In `tools/test-qemu.ps1`, in `$mustMatch1`, after the
`"net: resolved 10.0.2.2 -> 52:55:0a:00:02:02 \(src 10.0.2.15\)",` line, add:
```powershell
    "net: ping 10.0.2.2: reply \(seq 0\)",
```

- [ ] **Step 3b: (fallback path — ONLY if Step 2 showed "no reply")**

If SLIRP genuinely does not answer the gateway ping, do NOT fake a reply. Assert
the TX-only proof instead — in `$mustMatch1`, after the resolved line, add:
```powershell
    "net: ping 10.0.2.2: no reply",
```
and note in the commit + docs that the shipped proof is TX-only (the request is
transmitted and the framing is host-tested), a documented degraded outcome like
Phase 15's fallback. (Optionally, before falling back, try the DNS host `10.0.2.3`
as the target by changing the two `[10, 0, 2, 2]` ping addresses in `net_client` —
but only if it is ARP-resolvable; keep the gateway if unsure.)

- [ ] **Step 4: Re-run the boot smoke with the assertion**

Run: `./tools/test-qemu.ps1`
Expected: PASS — the serial log shows the asserted ping line plus the existing
DHCP/ARP lines, and both cross-boot self-healing boots stay green.

- [ ] **Step 5: Commit the test change**

```bash
git add tools/test-qemu.ps1
git commit -m "test: assert the ICMP ping of the gateway (reply, or TX-only fallback)"
```

---

### Task 4: Documentation

**Files:**
- Create: `docs/learning/0037-icmp-ping.md`
- Modify: `docs/roadmap/roadmap.md` (turn "Phase 19+ — Breadth" into a completed Phase 19 entry + a fresh Phase 20+ tail)
- Modify: `docs/glossary.md` (add an ICMP / ping entry)
- Modify: `docs/design/specs/2026-06-29-phase-19-icmp-ping-design.md` (impl note recording the spike outcome)

- [ ] **Step 1: Write learning note 0037**

Create `docs/learning/0037-icmp-ping.md` — short (summary, not tutorial). Cover:
that ICMP is IPv4 protocol 1 and **reuses the same RFC 1071 checksum** as the IPv4
header (so the whole "new protocol" is ~30 lines on top of `ipv4`); that ping is
the smallest IP round-trip the OS *initiates by address* and how it **chains** onto
the existing flow (adopted IP as source + ARP-resolved gateway MAC as destination);
that the boot smoke was the spike for "does SLIRP answer the gateway"; and the
actual outcome (reply vs TX-only fallback). Note the deferral: no RTT/stats, no
ICMP error types, no inbound-ping replies.

- [ ] **Step 2: Update the roadmap**

Replace the `## Phase 19+ — Breadth` heading with a completed
`## Phase 19 — ICMP echo (ping) the gateway  *(done — 2026-06-29)*` entry (goal /
you-learn / done-when matching the Phase 18 style and the shipped proof line), and
add a fresh `## Phase 20+ — Breadth` tail carrying the remaining long tail (ping
RTT/stats and arbitrary hosts; replying to inbound pings; DNS over the UDP layer;
lease renewal/expiry + netmask/router/DNS options; encrypting traffic with the
Phase 14 channel; epoch revocation + CDT; per-component crash ledgers; growable
records; board boot 4c; more devices/HAL).

- [ ] **Step 3: Update the glossary**

In `docs/glossary.md`, after the UDP/DHCP entries, add an **ICMP** entry: the
IPv4 control/diagnostic protocol (protocol 1); **ping** = an Echo Request (type 8)
answered by an Echo Reply (type 0), matched by identifier + sequence; Phase 19
pings the gateway `10.0.2.2` from the adopted IP, reusing the IPv4 checksum.

- [ ] **Step 4: Add the spec implementation note**

In the spec, add a short `## Implementation note (2026-06-29, during build)`
recording what shipped: the `icmp` submodule reusing `ipv4::checksum`, the
`gw_mac` capture + ping step, and the **spike outcome** — whether SLIRP replied
to the gateway ping (full round-trip) or the TX-only fallback was used.

- [ ] **Step 5: Verify references**

Run: `pwsh tools/check-references.ps1`
Expected: PASS. Fix any dangling reference before committing.

- [ ] **Step 6: Commit**

```bash
git add docs/
git commit -m "docs: Phase 19 ICMP ping — learning note 0037, roadmap, glossary; spec impl note"
```

---

## Self-review notes

- **Spec coverage:** `icmp` build/parse reusing `ipv4` (Task 1), `gw_mac` capture + ping step in `net_client` (Task 2), host+boot verification as the spike with reply/fallback branches (Task 3), docs incl. the spike outcome (Task 4). Gateway target, `ident=0x1234`/`seq=0`/`payload="kernelOS"`, no-MAX_TASKS-change, and the TX-only fallback are all encoded.
- **Type/name consistency:** `icmp::{PROTO_ICMP, ICMP_ECHO_REQUEST, ICMP_ECHO_REPLY, ICMP_HDR_LEN, build_echo_request, parse_echo_reply}`, `gw_mac: Option<[u8;6]>`, `ident`/`seq`, the proof string `net: ping 10.0.2.2: reply (seq 0)` consistent across tasks. `build_echo_request` reuses `ipv4::build_header(…, PROTO_ICMP, …)` and `ipv4::checksum`; `parse_echo_reply` mirrors `udp::parse`'s header walk.
- **TDD vs integration:** Task 1 is strict red-green host tests; Tasks 2–3 are kernel integration where the boot smoke is the spike — consistent with how every device/network phase in this repo is verified, and with the spec's spike-as-boot-smoke decision.
```
