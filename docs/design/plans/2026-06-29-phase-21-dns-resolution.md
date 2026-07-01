# Phase 21 — DNS resolution Implementation Plan

> **For agentic workers:** Implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Resolve `example.com` to an IPv4 address by querying SLIRP's DNS server (`10.0.2.3`) over the existing UDP layer — `net: dns example.com -> <ip>`.

**Architecture:** Pure DNS message logic (`build_query` + `parse_response`) joins `kernel_common::net::dns`, reusing the Phase 17 `udp` layer. `net_client` adds a DNS exchange as the last one before `NET_DONE`, through the unchanged bounded-server driver. The boot smoke spikes whether the harness resolves names; a synthetic self-demo is the documented fallback.

**Tech Stack:** Rust `no_std`, RISC-V64, QEMU `virt` + SLIRP, the IPv4/UDP layer (Phase 17), virtio-net (Phase 16).

## Global Constraints

- **Target/scope:** QEMU `virt` riscv64 only; no board. Unchanged QEMU flags.
- **Reuse, don't reinvent:** the query is wrapped with the existing `udp::build`/`udp::parse`. No change to `ipv4`/`udp`/`icmp`/`dhcp` or the driver.
- **Wire format:** big-endian. DNS A-record query: header (id, flags `0x0100`, QDCOUNT 1), QNAME as length-prefixed labels, QTYPE 1, QCLASS 1. `txid = 0xABCD`. UDP `src_port = 0xC000`, `dst_port = 53`, dst IP `10.0.2.3` reached via `gw_mac` (SLIRP routes all its virtual hosts through one MAC — no separate ARP).
- **DNS module is kernel-side** (called by `net_client`), so `&str::split`, range loops, and `match` are fine — no U-mode codegen constraint.
- **No driver/lifecycle change, no new task, no MAX_TASKS change.** DNS is the **last** real exchange before `NET_DONE` so a no-answer stalls only the net task (~5 driver exchanges total).
- **Spike + fallback:** the boot smoke reveals whether the harness resolves. If yes → assert `net: dns example.com -> \d+\.\d+\.\d+\.\d+` (any IPv4, value not pinned). If no answer → ship the synthetic self-demo (`net: dns example.com -> <ip> (self-demo)`, the wire query removed) and record the deviation (Phase 20 precedent).
- **Done-when (whole phase):** `./tools/test-qemu.ps1` shows `net: dns example.com -> <ip>` (or the self-demo fallback) after the existing net lines, with the cross-boot self-healing demo still passing and `cargo test` green.

## File structure

- `libs/common/src/net.rs` — add `pub mod dns` (after `pub mod icmp`) + host tests (Task 1).
- `kernel/src/main.rs` — add the DNS exchange to `net_client` (Task 2).
- `tools/test-qemu.ps1` — add the DNS assertion (Task 3).
- Docs (Task 4): `docs/learning/0039-*.md`, roadmap, glossary, spec impl note.

---

### Task 1: `dns` submodule (build_query + parse_response)

**Files:**
- Modify: `libs/common/src/net.rs` — add `pub mod dns` after the `icmp` module; add tests in `mod tests`.

**Interfaces:**
- Consumes: nothing outside `core` (self-contained DNS message logic).
- Produces: `dns::build_query(name: &str, txid: u16, out: &mut [u8]) -> usize`; `dns::parse_response(payload: &[u8], txid: u16) -> Option<[u8;4]>`. Consumed by Task 2.

- [ ] **Step 1: Write the failing tests**

In `libs/common/src/net.rs`, in `mod tests`, add:
```rust
    #[test]
    fn dns_build_query_encodes_name_and_qtype() {
        let mut out = [0u8; 64];
        let n = dns::build_query("example.com", 0xabcd, &mut out);
        // Header: id, flags 0x0100 (RD), QDCOUNT 1, AN/NS/AR 0.
        assert_eq!(&out[0..2], &0xabcdu16.to_be_bytes());
        assert_eq!(&out[2..4], &0x0100u16.to_be_bytes());
        assert_eq!(&out[4..6], &1u16.to_be_bytes(), "QDCOUNT 1");
        assert_eq!(&out[6..12], &[0, 0, 0, 0, 0, 0], "AN/NS/AR 0");
        // QNAME: 7 'example' 3 'com' 0
        assert_eq!(out[12], 7);
        assert_eq!(&out[13..20], b"example");
        assert_eq!(out[20], 3);
        assert_eq!(&out[21..24], b"com");
        assert_eq!(out[24], 0, "root label");
        // QTYPE A (1), QCLASS IN (1).
        assert_eq!(&out[25..27], &1u16.to_be_bytes());
        assert_eq!(&out[27..29], &1u16.to_be_bytes());
        assert_eq!(n, 29);
    }

    #[test]
    fn dns_parse_response_returns_first_a_record() {
        // Synthesize: header (id 0xabcd, QR set, ANCOUNT 1), the question, and one
        // A answer whose NAME is a compression pointer (0xc0 0x0c -> the question).
        let mut r = [0u8; 64];
        r[0..2].copy_from_slice(&0xabcdu16.to_be_bytes());
        r[2..4].copy_from_slice(&0x8180u16.to_be_bytes()); // QR + RD + RA
        r[4..6].copy_from_slice(&1u16.to_be_bytes()); // QDCOUNT
        r[6..8].copy_from_slice(&1u16.to_be_bytes()); // ANCOUNT
        // Question at offset 12: 7 example 3 com 0, QTYPE A, QCLASS IN.
        let q = [7u8, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 3, b'c', b'o', b'm', 0, 0, 1, 0, 1];
        r[12..12 + q.len()].copy_from_slice(&q);
        // Answer at offset 12+17=29.
        let mut i = 12 + q.len();
        r[i] = 0xc0; // name pointer ...
        r[i + 1] = 0x0c; // -> offset 12
        i += 2;
        r[i..i + 2].copy_from_slice(&1u16.to_be_bytes()); // TYPE A
        r[i + 2..i + 4].copy_from_slice(&1u16.to_be_bytes()); // CLASS IN
        r[i + 4..i + 8].copy_from_slice(&300u32.to_be_bytes()); // TTL
        r[i + 8..i + 10].copy_from_slice(&4u16.to_be_bytes()); // RDLENGTH
        r[i + 10..i + 14].copy_from_slice(&[93, 184, 216, 34]); // RDATA
        let end = i + 14;
        assert_eq!(dns::parse_response(&r[..end], 0xabcd), Some([93, 184, 216, 34]));
        // Rejections.
        assert!(dns::parse_response(&r[..end], 0x9999).is_none(), "wrong id");
        let mut no_ans = r;
        no_ans[6..8].copy_from_slice(&0u16.to_be_bytes()); // ANCOUNT 0
        assert!(dns::parse_response(&no_ans[..end], 0xabcd).is_none(), "no answers");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p kernel-common dns_`
Expected: FAIL — unresolved module `dns`.

- [ ] **Step 3: Implement the `dns` submodule**

In `libs/common/src/net.rs`, after the `icmp` module's closing `}` (before
`#[cfg(test)]`), add:
```rust
/// Minimal DNS: build an A-record query, parse the first A record from a reply.
/// Big-endian on the wire. Kernel-side only (uses iterators / `&str`).
pub mod dns {
    /// Build a DNS A-record query for `name` into `out` (>= name length + 18).
    /// Returns the payload length.
    pub fn build_query(name: &str, txid: u16, out: &mut [u8]) -> usize {
        out[0..2].copy_from_slice(&txid.to_be_bytes());
        out[2..4].copy_from_slice(&0x0100u16.to_be_bytes()); // recursion desired
        out[4..6].copy_from_slice(&1u16.to_be_bytes()); // QDCOUNT
        out[6..12].copy_from_slice(&[0u8; 6]); // ANCOUNT/NSCOUNT/ARCOUNT
        let mut i = 12;
        for label in name.split('.') {
            let bytes = label.as_bytes();
            out[i] = bytes.len() as u8;
            i += 1;
            out[i..i + bytes.len()].copy_from_slice(bytes);
            i += bytes.len();
        }
        out[i] = 0; // root label
        i += 1;
        out[i..i + 2].copy_from_slice(&1u16.to_be_bytes()); // QTYPE A
        out[i + 2..i + 4].copy_from_slice(&1u16.to_be_bytes()); // QCLASS IN
        i + 4
    }

    /// Parse a DNS response: verify the id and the response flag, skip the
    /// question(s), and return the first A record's IP. `None` on a wrong id, a
    /// non-response, no answers, or no A record.
    pub fn parse_response(payload: &[u8], txid: u16) -> Option<[u8; 4]> {
        if payload.len() < 12 || be16(&payload[0..2]) != txid {
            return None;
        }
        if be16(&payload[2..4]) & 0x8000 == 0 {
            return None; // QR not set: not a response
        }
        let qdcount = be16(&payload[4..6]);
        let ancount = be16(&payload[6..8]);
        if ancount == 0 {
            return None;
        }
        let mut i = 12;
        // Skip the questions: NAME + QTYPE(2) + QCLASS(2).
        for _ in 0..qdcount {
            i = skip_name(payload, i)?;
            i += 4;
        }
        // Walk the answers for the first A record.
        for _ in 0..ancount {
            i = skip_name(payload, i)?;
            if i + 10 > payload.len() {
                return None;
            }
            let atype = be16(&payload[i..i + 2]);
            let rdlength = be16(&payload[i + 8..i + 10]) as usize;
            i += 10;
            if atype == 1 && rdlength == 4 && i + 4 <= payload.len() {
                return Some([payload[i], payload[i + 1], payload[i + 2], payload[i + 3]]);
            }
            i += rdlength;
        }
        None
    }

    fn be16(b: &[u8]) -> u16 {
        u16::from_be_bytes([b[0], b[1]])
    }

    /// Advance past a DNS name at `i`: a `0xC0` compression pointer is 2 bytes; a
    /// label is `1 + len` bytes; the root `0` is 1 byte. `None` if it runs off.
    fn skip_name(payload: &[u8], mut i: usize) -> Option<usize> {
        loop {
            let b = *payload.get(i)?;
            if b & 0xc0 == 0xc0 {
                return Some(i + 2); // pointer
            }
            if b == 0 {
                return Some(i + 1); // root
            }
            i += 1 + b as usize; // label
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p kernel-common dns_`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add libs/common/src/net.rs
git commit -m "feat(net): minimal DNS — build_query + parse_response (first A record, host-tested)"
```

---

### Task 2: DNS exchange in `net_client`

**Files:**
- Modify: `kernel/src/main.rs` — in `net_client`, insert the DNS block after the inbound-ping self-demo block (between it and the `// --- tell the driver to exit ---` comment).

**Interfaces:**
- Consumes: `dns::{build_query, parse_response}` (Task 1); `udp::{build, parse}` (Phase 17); `NET_IP`, `NET_EP_CAP`, `gw_mac`, `tx_frame`, `rx_frame`, `src_mac`, `sched::call_message` (in scope).
- Produces: the DNS exchange + its log line.

- [ ] **Step 1: Insert the DNS block**

In `kernel/src/main.rs`, in `net_client`, find the `// --- tell the driver to exit ---`
comment (with its `let _ = sched::call_message(NET_EP_CAP, NET_DONE);`) and insert
immediately BEFORE it:
```rust
            // --- DNS: resolve a name to an IPv4 address via SLIRP's resolver
            // (10.0.2.3, reached through gw_mac — SLIRP routes its virtual hosts
            // through one MAC). The LAST exchange: SLIRP forwards DNS to the host
            // resolver, so a no-answer would stall only this (everything above has
            // already printed). ---
            match gw_mac {
                Some(mac) => {
                    let txid = 0xABCDu16;
                    let src_port = 0xC000u16;
                    let mut q = [0u8; 64];
                    let qlen = net::dns::build_query("example.com", txid, &mut q);
                    let frame_len = {
                        let txf = core::slice::from_raw_parts_mut(tx_frame as *mut u8, 512);
                        net::udp::build(&src_mac, &mac, NET_IP, [10, 0, 2, 3], src_port, 53, 0, &q[..qlen], txf)
                    };
                    let rx_len = sched::call_message(NET_EP_CAP, 12 + frame_len);
                    let mut resolved = false;
                    if rx_len != 0 {
                        let flen = rx_len.saturating_sub(12).min(2036);
                        let rxf = core::slice::from_raw_parts(rx_frame as *const u8, flen);
                        if let Some(payload) = net::udp::parse(rxf, src_port) {
                            if let Some(ip) = net::dns::parse_response(payload, txid) {
                                println!("net: dns example.com -> {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
                                resolved = true;
                            }
                        }
                    }
                    if !resolved {
                        println!("net: dns example.com: no answer");
                    }
                }
                None => println!("net: dns skipped (no gateway MAC)"),
            }

```
(Keep the existing `// --- tell the driver to exit ---` block right after it.)

- [ ] **Step 2: Build the kernel**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: builds clean (no warnings — `dns::*` used).

- [ ] **Step 3: Commit**

```bash
git add kernel/src/main.rs
git commit -m "feat(net): net_client resolves example.com via DNS to SLIRP's 10.0.2.3"
```

---

### Task 3: Verify — host tests + boot smoke (the spike)

**Files:**
- Modify: `tools/test-qemu.ps1` (add the DNS assertion, matching the observed outcome).

- [ ] **Step 1: Host tests**

Run: `cargo test`
Expected: PASS — all `kernel_common::net` tests including the new `dns_*`.

- [ ] **Step 2: Boot smoke WITHOUT a new assertion — observe whether DNS resolves**

Run: `./tools/test-qemu.ps1` (passes on existing assertions). Then inspect the log:
```powershell
$s = Join-Path ([System.IO.Path]::GetTempPath()) "kernel-qemu-serial.log"
Select-String -Path $s -Pattern "net: dns" | ForEach-Object { $_.Line }
```
Expected: one of
- `net: dns example.com -> <ip>` → the harness resolved it; real round-trip. Go to Step 3a.
- `net: dns example.com: no answer` → no host DNS / no answer. Go to Step 3b (self-demo).
- (Nothing / the line missing) → the DNS query hung the net task (no answer, driver
  blocked). Treat as Step 3b: switch to the self-demo (which sends no wire query).

- [ ] **Step 3a: (resolved path) Assert the real resolution**

In `tools/test-qemu.ps1`, in `$mustMatch1`, after the
`"net: replied to inbound ping \(self-demo, seq 0\)",` line, add:
```powershell
    "net: dns example.com -> \d+\.\d+\.\d+\.\d+",
```

- [ ] **Step 3b: (self-demo path — ONLY if Step 2 showed "no answer" or nothing)**

If the harness does not resolve names (or the wire query hung), replace the DNS
block's body in `net_client` so it does NOT send a wire query — instead run the
real `parse_response` on a synthesized answer (the responder logic is real; only
the answer is fabricated, the Phase 20 precedent). Replace the inserted block from
Task 2 with:
```rust
            // --- DNS (self-demo): this harness's SLIRP forwards DNS to a host
            // resolver that does not answer here, and an unanswered query would
            // hang the bounded driver. Prove the DNS message logic on a
            // synthesized answer instead (logic real; answer fabricated). ---
            {
                let txid = 0xABCDu16;
                let mut q = [0u8; 64];
                let qlen = net::dns::build_query("example.com", txid, &mut q);
                // Synthesize a response: copy the query header+question, set QR +
                // ANCOUNT 1, and append one A answer (name pointer -> the question).
                let mut r = [0u8; 96];
                r[..qlen].copy_from_slice(&q[..qlen]);
                r[2..4].copy_from_slice(&0x8180u16.to_be_bytes()); // QR + RD + RA
                r[6..8].copy_from_slice(&1u16.to_be_bytes()); // ANCOUNT 1
                let mut j = qlen;
                r[j] = 0xc0;
                r[j + 1] = 0x0c; // name -> offset 12 (the question)
                r[j + 2..j + 4].copy_from_slice(&1u16.to_be_bytes()); // TYPE A
                r[j + 4..j + 6].copy_from_slice(&1u16.to_be_bytes()); // CLASS IN
                r[j + 6..j + 10].copy_from_slice(&300u32.to_be_bytes()); // TTL
                r[j + 10..j + 12].copy_from_slice(&4u16.to_be_bytes()); // RDLENGTH
                r[j + 12..j + 16].copy_from_slice(&[93, 184, 216, 34]); // RDATA
                j += 16;
                if let Some(ip) = net::dns::parse_response(&r[..j], txid) {
                    println!("net: dns example.com -> {}.{}.{}.{} (self-demo)", ip[0], ip[1], ip[2], ip[3]);
                } else {
                    println!("net: dns example.com: no answer");
                }
            }
```
Then in `tools/test-qemu.ps1`, in `$mustMatch1`, after the inbound-ping line, add:
```powershell
    "net: dns example.com -> \d+\.\d+\.\d+\.\d+ \(self-demo\)",
```
and note in the commit + docs that the shipped proof is the synthetic self-demo (a
documented degraded outcome like Phase 20: the DNS logic is real, the answer
fabricated, because SLIRP forwards DNS to a host resolver that does not answer in
this harness).

- [ ] **Step 4: Re-run the boot smoke with the assertion**

Run: `./tools/test-qemu.ps1`
Expected: PASS — the serial log shows the asserted DNS line plus the existing net
lines, and both cross-boot self-healing boots stay green.

- [ ] **Step 5: Commit the test (and any self-demo) change**

```bash
git add tools/test-qemu.ps1 kernel/src/main.rs
git commit -m "test: assert DNS resolution (real, or self-demo fallback)"
```

---

### Task 4: Documentation

**Files:**
- Create: `docs/learning/0039-dns-resolution.md`
- Modify: `docs/roadmap/roadmap.md` (turn "Phase 21+ — Breadth" into a completed Phase 21 entry + a fresh Phase 22+ tail)
- Modify: `docs/glossary.md` (add a DNS entry)
- Modify: `docs/design/specs/2026-06-29-phase-21-dns-resolution-design.md` (impl note with the spike outcome)

- [ ] **Step 1: Write learning note 0039**

Create `docs/learning/0039-dns-resolution.md` — short (summary, not tutorial).
Cover: the DNS message shape (a 12-byte header + a question with the name as
**length-prefixed labels** + answers); the one parsing subtlety — **name
compression** (an answer's NAME is usually a `0xC0` pointer back into the packet,
so a generic `skip_name` is needed); that DNS slotted straight onto the Phase 17
UDP layer (the query is just a UDP payload); the hermeticity difference (SLIRP
answers ARP/DHCP/gateway-ping **locally** but **forwards** DNS to the host
resolver, so it depends on host networking, and an unanswered query hangs the
bounded driver); and the actual spike outcome (real resolution vs the synthetic
self-demo fallback). Note the deferral: caching, AAAA/other records, and using the
resolved IP.

- [ ] **Step 2: Update the roadmap**

Replace the `## Phase 21+ — Breadth` heading with a completed
`## Phase 21 — DNS resolution (name → IP)  *(done — 2026-06-29)*` entry (goal /
you-learn / done-when matching the Phase 20 style and the shipped proof line —
note whether real or self-demo), and add a fresh `## Phase 22+ — Breadth` tail
carrying the remaining long tail (use the resolved IP for a real exchange; DNS
caching + AAAA/other records; a persistent ping responder / RX as a long-lived
service; DHCP renewal/expiry + netmask/router/DNS options; encrypting traffic with
the Phase 14 channel; epoch revocation + CDT; per-component crash ledgers; growable
records; board boot 4c; more devices/HAL).

- [ ] **Step 3: Update the glossary**

In `docs/glossary.md`, after the ICMP/DHCP entries, add a **DNS** entry: how a
name becomes an address — a UDP query (port 53) for an **A record**, the name
encoded as length-prefixed labels, the answer's name usually a **compression
pointer**; Phase 21 resolves `example.com` via SLIRP's resolver `10.0.2.3` (which
forwards to the host).

- [ ] **Step 4: Add the spec implementation note**

In the spec, add a short `## Implementation note (2026-06-29, during build)`
recording what shipped: the `dns` module (`build_query`/`parse_response` with
compression-pointer skipping), the `net_client` DNS exchange, and the **spike
outcome** — whether the harness resolved `example.com` (real round-trip) or the
synthetic self-demo fallback was used.

- [ ] **Step 5: Verify references**

Run: `pwsh tools/check-references.ps1`
Expected: PASS. Fix any dangling reference before committing.

- [ ] **Step 6: Commit**

```bash
git add docs/
git commit -m "docs: Phase 21 DNS resolution — learning note 0039, roadmap, glossary; spec impl note"
```

---

## Self-review notes

- **Spec coverage:** `build_query` + `parse_response` (Task 1), the DNS exchange in `net_client` reusing `udp` (Task 2), host+boot verification as the spike with resolve/self-demo branches (Task 3), docs incl. the spike outcome (Task 4). The gw_mac-for-10.0.2.3, DNS-last, `txid=0xABCD`/`src_port=0xC000`, example.com, any-IPv4-assertion, and no-MAX_TASKS-change constraints are all encoded.
- **Type/name consistency:** `dns::{build_query, parse_response}`, `txid`, `src_port`, the proof strings `net: dns example.com -> <ip>` / `(self-demo)` consistent across tasks. The DNS query is wrapped with `udp::build(.., src_port, 53, ..)` and the reply read with `udp::parse(.., src_port)` — matching the Phase 17 `udp` signatures. The synthesized-response layout in Task 3b matches the offsets `parse_response` walks in Task 1.
- **TDD vs integration:** Task 1 is strict red-green host tests; Tasks 2–3 are kernel integration where the boot smoke is the spike (real vs synthetic) — consistent with the repo and the spec's spike-as-boot-smoke decision.
```
