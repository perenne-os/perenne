# Kernel *(working title — see [ADR 0006](docs/decisions/0006-project-name-placeholder.md))*

A from-scratch, **security-first**, **hardware-agnostic** operating-system kernel written in **Rust**, built around a **microkernel** design and a **self-healing "knowledge organism"** — an OS that diagnoses and documents its own problems instead of depending on a human support community.

> **Status: Phase 0 — foundation.** No kernel functionality yet. This is the documented skeleton, the locked architectural decisions, and a verified development environment. Booting a real kernel is Phase 1.

This is a deliberate, multi-year, solo, open-source effort. Correctness and security come before speed and features. A tiny verified "hello world" kernel is considered a legitimate success.

## Why this exists

- **Trusted & secure by design** — security is architected in from the first commit (memory-safe Rust, a tiny privileged core, post-quantum cryptography), not bolted on later.
- **Community-independent support** — instead of relying on forums, the OS builds a structured, growing memory of issues and proven fixes, and consults *itself* first.
- **Future-hardware-ready** — a clean hardware-abstraction boundary means tomorrow's chips (including AI accelerators and quantum coprocessors) slot in as "just another device" without rewrites.

## Decisions so far

| Area | Choice | Why (one line) |
|------|--------|----------------|
| Language | **Rust** | Eliminates ~70% of OS vulnerability classes at compile time |
| Architecture | **Microkernel** (capability-based) | Smallest attack surface; isolated, restartable services |
| First target | **RISC-V on QEMU** | Cleanest to learn, future-forward; portable to other CPUs later |
| Cryptography | **Post-quantum baseline** | Trusted against future quantum attackers |
| Support model | **Self-healing knowledge organism** | Diagnoses and documents its own issues |
| Name | **Provisional ("Kernel")** | Deferred but rename-safe |

Full rationale lives in [`docs/decisions/`](docs/decisions/) (Architecture Decision Records).

## Repository layout

```
docs/            Vision, architecture, decisions (ADRs), roadmap, glossary, learning notes
knowledge-base/  The self-healing organism's memory: issue + fix records and their schema
kernel/          The microkernel (Rust) — placeholder until Phase 1
arch/riscv64/    Architecture-specific code (first target)
hal/             Hardware Abstraction Layer (the device-agnostic boundary)
services/        User-space services: drivers, filesystem, network (Phase 6+)
libs/            Shared libraries (common types, crypto)
tools/           Build and QEMU helper scripts
tests/           Integration / system test harnesses
```

## Getting started

**Prerequisites**
- [Rust](https://rustup.rs) (the pinned nightly toolchain installs automatically via `rust-toolchain.toml`)
- [QEMU](https://www.qemu.org) with `qemu-system-riscv64` on your PATH
- On Windows: the MSVC toolchain **with the Windows SDK** (needed to link host test binaries)

**Build and test**
```powershell
./tools/build.ps1
```

**Boot the RISC-V virtual machine** (currently boots OpenSBI firmware; our kernel arrives in Phase 1)
```powershell
./tools/run-qemu.ps1   # exit QEMU with Ctrl-A then X
```

See [`docs/learning/0001-dev-environment.md`](docs/learning/0001-dev-environment.md) for environment notes (and the WSL2 alternative).

## Roadmap

Phase 0 (foundation) → 1 (hello-world kernel) → 2 (memory/interrupts/scheduling) → 3 (security spine) → 4 (real hardware) → 5 (self-healing seed) → 6+ (breadth). Each phase gets its own design → plan → build cycle. Details: [`docs/roadmap/roadmap.md`](docs/roadmap/roadmap.md).

## Documentation map

- [Vision](docs/vision/) — north star and guiding principles
- [Architecture](docs/architecture/) — overview, security model, hardware abstraction, self-healing
- [Decisions](docs/decisions/) — the ADRs
- [Glossary](docs/glossary.md) — plain-language definitions for newcomers
- [Knowledge base](knowledge-base/) — the self-healing organism's memory

## License

Licensed under the [Apache License 2.0](LICENSE).
