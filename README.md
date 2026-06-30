# Perenne

**An OS that remembers.** A from-scratch, **security-first**, **hardware-agnostic** operating-system kernel written in **Rust**, built around a **capability microkernel** and a **self-healing "knowledge organism"** — an OS that diagnoses, documents, and *recovers from* its own faults, and **learns from them across reboots**, instead of depending on a human support community.

> The name **Perenne** (*puh-REN-eh*, from *perennial*) is the thesis in one word: a system that dies back, recovers, and returns each cycle — renewed by what it learned. See [ADR 0008](docs/decisions/0008-project-name-perenne.md).

> **Status:** a coherent, demonstrable microkernel. **Phases 0–21 complete** — a secure capability core, drivers as isolated user-space components, post-quantum-keyed encrypted IPC, a full self-healing loop that learns across reboots, and a working network stack (DHCP, ping, DNS). Boots under QEMU/riscv64; all milestones verified by an automated boot test. *(This is a deliberate, multi-year, solo, open-source effort — correctness and security before speed and features.)*

---

## What it does today

Every capability below is real, runs on each boot, and is asserted by the automated test (`tools/test-qemu.ps1`).

### 🛡️ A secure capability microkernel
- Boots a freestanding `no_std` Rust kernel on RISC-V (QEMU `virt`), with Sv39 paging, W^X enforcement, trap handling, and a preemptive scheduler.
- **Drivers are unprivileged user-space components** ([ADR 0007](docs/decisions/0007-extensibility-user-space-components.md)): the RTC, the entropy source (virtio-rng), the disk (virtio-blk), and the **NIC (virtio-net)** each run in their own address space, holding only the **capabilities** they were granted. The kernel never touches their device registers.
- **Unforgeable capabilities** gate every privileged action; they can be **delegated** between components at runtime and **revoked** transitively.
- **Post-quantum cryptography** ([ADR 0004](docs/decisions/0004-post-quantum-crypto.md)): an ML-KEM-768 shared secret keys a ChaCha20-Poly1305 **encrypted IPC channel** between components (a tampered message is rejected; a component without the `Session` capability is refused).

### 🧬 A self-healing knowledge organism *(the soul of the project — [ADR 0005](docs/decisions/0005-self-healing-knowledge-organism.md))*
When a component crashes, the OS runs the **full loop, autonomously**:

```
detect → diagnose (against an on-disk knowledge base) → cage the fix → restart
      → learn (record a never-seen fault to disk) → count recurrence
      → escalate (chronic) → quarantine (stop a futile restart)
```

- It reads its knowledge base from **disk** (a minimal filesystem over virtio-blk), not a hardcoded table.
- It **records a fault it has never seen** as a new knowledge entry, so a **second boot of the same image diagnoses the crash it documented itself** — the OS learns across reboots.
- It tracks how often each issue recurs, **escalates** chronic ones, and **quarantines** a component instead of restarting it forever — a decision that provably requires its persistent memory.

### 🌐 A real network stack
Built bottom-up, each layer pure and host-tested, then proven over QEMU's user network:

- **virtio-net + ARP** — resolves the gateway's MAC.
- **DHCP** — the full DORA lease (DISCOVER/OFFER/REQUEST/ACK); the OS **adopts the leased IP** as its own.
- **ICMP** — pings the gateway and gets a reply, and answers an inbound echo request.
- **DNS** — resolves `example.com` to a **live IP address** over UDP.

A representative boot:
```
hello world from Perenne - Phase 4a (hart 0)
crypto: channel session established (ML-KEM)
net: dhcp leased 10.0.2.15 (ack)
net: adopted ip 10.0.2.15
net: resolved 10.0.2.2 -> 52:55:0a:00:02:02 (src 10.0.2.15)
net: ping 10.0.2.2: reply (seq 0)
net: dns example.com -> 172.66.147.243
sched: task 'transient' killed by LoadPageFault
heal: diagnosed KB-0005 (...) -> playbook: Restart the component ...
heal: restarted 'transient' (attempt 1)   ← recovered, and learned
```

## Why this exists

- **Trusted & secure by design** — security is architected in from the first commit (memory-safe Rust, a tiny privileged core, capability-based isolation, post-quantum cryptography), not bolted on later.
- **Community-independent support** — instead of relying on forums, the OS builds a structured, growing memory of issues and proven fixes, and consults *itself* first. This is Perenne's defining differentiator.
- **Future-hardware-ready** — a clean hardware-abstraction boundary means tomorrow's chips (including AI accelerators and quantum coprocessors) slot in as "just another device" without rewrites.

## Key decisions

| Area | Choice | Why (one line) |
|------|--------|----------------|
| Language | **Rust** | Eliminates ~70% of OS vulnerability classes at compile time |
| Architecture | **Microkernel** (capability-based) | Smallest attack surface; isolated, restartable services |
| First target | **RISC-V on QEMU** | Cleanest to learn, future-forward; portable to other CPUs later |
| Cryptography | **Post-quantum baseline** | Trusted against future quantum attackers |
| Support model | **Self-healing knowledge organism** | Diagnoses and documents its own issues |
| Name | **Perenne** | Perennial — self-renewing, self-healing ([ADR 0008](docs/decisions/0008-project-name-perenne.md)) |

Full rationale lives in [`docs/decisions/`](docs/decisions/) (Architecture Decision Records).

## Repository layout

```
docs/            Vision, architecture, decisions (ADRs), roadmap, glossary, per-phase design+plan+learning notes
knowledge-base/  The self-healing organism's memory: issue + fix records and their schema
kernel/          The microkernel — freestanding no_std binary
arch/riscv64/    Architecture-specific code (first target): traps, paging, scheduler, caps/IPC, drivers
hal/             Hardware Abstraction Layer (the device-agnostic boundary)
libs/            Shared, host-tested libraries: common types, the filesystem/KB/network wire formats, crypto
tools/           Build and QEMU helper scripts (build / run / test)
```

## Getting started

**Prerequisites**
- [Rust](https://rustup.rs) (the pinned toolchain installs automatically via `rust-toolchain.toml`)
- [QEMU](https://www.qemu.org) with `qemu-system-riscv64` on your PATH
- On Windows: the MSVC toolchain **with the Windows SDK** (needed to link host test binaries)

**Build and test** (host build + unit tests, then the riscv64 cross-build)
```powershell
./tools/build.ps1
```

**Boot Perenne in QEMU**
```powershell
./tools/run-qemu.ps1   # exit QEMU with Ctrl-A then X
```
Expect the OpenSBI banner, then `hello world from Perenne - Phase 4a (hart 0)` and the boot proofs above.

**Automated boot check** (non-interactive; exit code 0 = pass) — boots the full system twice over one disk image to prove the organism learns across reboots:
```powershell
./tools/test-qemu.ps1
```

See [`docs/learning/0001-dev-environment.md`](docs/learning/0001-dev-environment.md) for environment notes (and the WSL2 alternative).

## How it was built

Perenne grows in small, finishable **phases**, each its own *design → plan → build → learning-note* cycle (so the reasoning is durable, not just the code):

**0** foundation · **1** hello-world kernel · **2** memory / traps / scheduling · **3** security spine (user mode, capabilities + IPC, post-quantum primitive) · **4** real-hardware groundwork (device tree, UART) · **5** self-healing seed (detect + caged fix) · **6–7** persistent storage + the living, *learning* knowledge base · **8–13** dynamic capabilities (delegation, revocation) + an interactive diagnosis shell + counter-driven escalation & quarantine · **14** post-quantum encrypted IPC · **15–21** the network stack (virtio-net, ARP, IPv4/UDP, DHCP, ICMP ping in/out, DNS).

Full details: [`docs/roadmap/roadmap.md`](docs/roadmap/roadmap.md). Each phase's spec, plan, and a short "what I learned" note live under [`docs/`](docs/).

## Documentation map

- [Vision](docs/vision/) — the north star and guiding principles
- [Architecture](docs/architecture/) — overview, security model, hardware abstraction, self-healing
- [Decisions](docs/decisions/) — the ADRs (the *why* behind every major choice)
- [Roadmap](docs/roadmap/roadmap.md) — the phase-by-phase journey
- [Glossary](docs/glossary.md) — plain-language definitions for newcomers
- [Learning notes](docs/learning/) — a short, honest note per phase
- [Knowledge base](knowledge-base/) — the self-healing organism's on-disk memory

## License

Licensed under the [Apache License 2.0](LICENSE).
