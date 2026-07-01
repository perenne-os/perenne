# Perenne — the grand demo

One command boots the whole system and proves every pillar at once. It runs **two
boots over one disk image**, so the finale is the OS **diagnosing a fault it
documented itself on the previous boot** — self-healing that *learns across
reboots*.

```powershell
./tools/test-qemu.ps1     # boots twice, headless; exit 0 = every milestone asserted
```
*(Prefer to watch it live? `./tools/run-qemu.ps1` — exit QEMU with `Ctrl-A` then `X`.)*

Everything below is **real, unedited serial output** from that run. Because drivers,
the network, and the self-healer are all independent components running
concurrently, the live log **interleaves** — that interleaving *is* the point: it's
a whole system breathing at once. Below it's grouped into acts for the story.

---

## Act 0 — from firmware to our own kernel

OpenSBI hands off; our freestanding Rust kernel takes over and greets you *by name*:

```
hello world from Perenne - Phase 4a (hart 0)
trap: breakpoint at 0x802036d0        ← the trap handler catches, and recovers from, an exception
survived breakpoint
console: ns16550a @ 0x10000000 (device tree)     ← the console was discovered, not hardcoded
dt: 192 MiB RAM @ 0x80000000, timebase 10000000 Hz
```

## Act 1 — the secure core wakes up

Virtual memory on; a write to read-only memory is **blocked by hardware** (W^X);
and the post-quantum key exchange establishes an encrypted-channel session:

```
paging: sv39 on (47999 of 48100 frames free)
trap: W^X store fault at 0x80258570 (probe)
wx: rodata write blocked               ← code can't be written, data can't be executed
frames: alloc/free ok
crypto: channel session established (ML-KEM)     ← post-quantum shared secret, ready to key AEAD
```

## Act 2 — capabilities: authority you can grant, refuse, and revoke

Every privileged action is gated by an **unforgeable capability**. Watch authority
*flow* and *fail safely* — each refusal is **contained**, and the system keeps running:

```
cap: 'broker' grant rejected (no capability in slot)   ← can't delegate what you don't hold
cap: 'broker' delegated Endpoint(0) to 'needy'         ← runtime delegation between components
cap: 'lease' revoked endpoint 7 from 1 holder(s)       ← authority taken back...
ipc: 'tenant' call rejected (no capability)            ← ...so the tenant's next call fails
crypto: 'nocap' seal refused (no Session capability)   ← crypto is capability-gated too
ipc: 'rogue' send rejected (no capability)             ← a component with no cap simply can't reach out
```

## Act 3 — encrypted IPC, real entropy, post-quantum

A `sealer` encrypts a message and an `opener` decrypts + verifies it; a hardware
entropy source seeds a reseedable pool that keys the ML-KEM round-trip:

```
ipc: 'sealer' -> 'opener' badge 0x0       ← authenticated-encrypted message across components
sched: task 'opener' exited (code 14)     ← decrypted, verified, AND rejected a tampered copy
irq: external IRQ 8 woke 'entropy'        ← the entropy driver is interrupt-driven
entropy: pool seeded from virtio-rng      ← real device entropy, not a fixed seed
entropy: pool serves on demand (draws differ)
pqc: ML-KEM-768 round-trip ok (pool-seeded)
```

## Act 4 — the network comes alive

Bottom-up, each step feeding the next — the OS **learns its own address**, reaches
the gateway, and **resolves a name to a live IP** — all through a NIC driver that is
itself an unprivileged component:

```
net: dhcp offered 10.0.2.15
net: dhcp leased 10.0.2.15 (ack)                          ← the full DHCP handshake
net: adopted ip 10.0.2.15                                 ← and it ADOPTS the leased address
net: resolved 10.0.2.2 -> 52:55:0a:00:02:02 (src 10.0.2.15)   ← ARP, sourced from the leased IP
net: ping 10.0.2.2: reply (seq 0)                         ← ICMP echo out...
net: replied to inbound ping (self-demo, seq 0)           ← ...and it answers one too
net: dns example.com -> 104.20.23.154                     ← a real, live DNS resolution
```

## Act 5 — the organism heals itself *(boot 1)*

Now the soul of the project. Three components crash on purpose; the OS **diagnoses
each against its on-disk knowledge base** and acts:

```
sched: task 'transient' killed by LoadPageFault
heal: diagnosed KB-0005 (...) -> playbook: Restart the component ...
heal: restarted 'transient' (attempt 1)
sched: task 'transient' exited (code 0)               ← crashed → diagnosed → restarted → RECOVERED

sched: task 'flaky' killed by LoadPageFault
heal: restarted 'flaky' (attempt 1)
... (crashes again) ...
heal: giving up on 'flaky' after 2 restarts (flagged for triage)   ← the restart cage is bounded

sched: task 'novel' killed by IllegalInstruction
heal: no known issue for IllegalInstruction (recording for write-back)
...
heal: recorded KB-0006 (illegal-instruction) to disk   ← a NEVER-SEEN fault, WRITTEN to disk
```

Along the way, you can **interrogate** the organism from the console:

```
> help
commands: help, kb, diag
> kb
KB-0005 (seen 1)  User-space component terminated by a fatal fault
> diag
last: KB-0005 -> Restart the component, up to a bounded number of retries.
```

## Act 6 — it *learns across reboots* — the finale *(boot 2, same disk image)*

The second boot reads the disk the first boot wrote — including the entry the OS
authored itself — and it now **understands a fault it had never seen before**, and
**acts on accumulated history**:

```
heal: loaded 2 KB entries from disk (scanned 2)     ← it wrote KB-0006 last boot; now it reads both
heal: diagnosed KB-0006 (Observed fault: illegal-instruction (auto-recorded)) -> playbook: ...
                                                     ↑ it diagnoses the crash it DOCUMENTED ITSELF
heal: KB-0005 escalated (seen 6) -- recurring; flag for triage   ← cross-boot recurrence noticed
heal: 'flaky' quarantined (KB-0005 chronic) -- not restarting    ← it STOPS a futile fix
heal: persisted KB-0005 (seen 6, escalated)
```

That last act — **quarantining a chronic fault instead of restarting it forever** —
is a decision that *provably requires persistent memory*. The OS didn't just
recover; it **remembered, learned, and changed its behavior**.

---

## What you just watched

| Pillar | Proof in the run |
|---|---|
| **Secure capability microkernel** | delegation, revocation, and every no-capability attempt *contained*, not fatal |
| **Post-quantum security** | ML-KEM session established; encrypted IPC verified + tamper-rejected |
| **Self-healing organism** | crash → diagnose → cage → **learn to disk** → count → **escalate** → **quarantine**, across two boots |
| **Real networking** | DHCP lease + adopt, ARP, ping (out & in), and a live DNS resolution |
| **Unprivileged drivers** | the clock, entropy, disk, and NIC each ran as isolated user-space components |

---

## Narration script (for a screen recording)

Short lines to speak over the boot, if you record it:

1. *"This is Perenne — a security-first, self-healing microkernel, written from scratch in Rust. One command boots the whole thing."*
2. *"It comes up on RISC-V, turns on virtual memory, and blocks a write to read-only memory in hardware. Then it establishes a post-quantum-keyed encrypted channel."*
3. *"Every privileged action needs an unforgeable capability. Here authority is delegated between components, then revoked — and a component without the right capability is simply refused, without crashing the system."*
4. *"Now the network: it leases an IP over DHCP and adopts it, resolves the gateway, pings it, answers a ping, and resolves a real domain name to a live address — all through a NIC driver that runs unprivileged."*
5. *"And this is the heart of it. Components crash on purpose. The OS diagnoses each against a knowledge base it reads from disk, restarts what it can, and — for a fault it's never seen — writes a new entry to disk."*
6. *"Watch the second boot. It reads back what it wrote, and now it understands a crash it had never seen before. It notices this issue keeps recurring, and instead of restarting it forever, it quarantines it. The OS didn't just recover — it remembered, learned, and changed its mind."*
7. *"That's Perenne. An OS that remembers."*

---

*More: the [3-minute visual tour](docs/architecture/showcase.md) · the
[roadmap](docs/roadmap/roadmap.md) · the [decisions](docs/decisions/).*
