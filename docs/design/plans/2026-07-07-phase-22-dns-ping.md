# Phase 22 — Ping the DNS-Resolved IP (Plan)

This document is the historical record and task list used to implement Phase 22.

## Goal
Use the live IPv4 address resolved via DNS (Phase 21) to send a real ICMP Echo Request and parse the Echo Reply, proving end-to-end IP routing via the gateway.

## Tasks

### Task 1: Spec and Plan Documentation [DONE]
- [x] Create spec document `docs/design/specs/2026-07-07-phase-22-dns-ping-design.md`
- [x] Create plan document `docs/design/plans/2026-07-07-phase-22-dns-ping.md`

### Task 2: Implement DNS-Resolved Ping in `kernel/src/main.rs` [DONE]
- [x] In `net_client`, capture the resolved IP address from `net::dns::parse_response`.
- [x] Construct an ICMP Echo Request targeted at the resolved IP, using the gateway MAC address (`mac`) as destination MAC.
- [x] Transmit the request via the user-space NIC driver using IPC (`sched::call_message`).
- [x] Wait for the reply, parse it using `net::icmp::parse_echo_reply`.
- [x] Print the outcome:
  - Success: `net: ping example.com (a.b.c.d): reply (seq 0)`
  - Timeout/No reply: `net: ping example.com (a.b.c.d): no reply`

### Task 3: Update Automated Boot Test in `tools/test-qemu.ps1` [DONE]
- [x] Add regex pattern for `net: ping example.com` to `$mustMatch1` in the first boot verification sequence.
- [x] Pattern: `"net: ping example.com \(\d+\.\d+\.\d+\.\d+\): (reply \(seq 0\)|no reply)"`
- [x] Update the final green pass output message to mention Phase 22.

### Task 4: Update Roadmap and Documentation [DONE]
- [x] Modify `docs/roadmap/roadmap.md` to list Phase 22 as completed.
- [x] Move `Phase 22+ — Breadth` to `Phase 23+ — Breadth`.

### Task 5: Build and Verify [DONE]
- [x] Run host builds and tests: `./tools/build.ps1`
- [x] Run QEMU automated boot test: `./tools/test-qemu.ps1`
