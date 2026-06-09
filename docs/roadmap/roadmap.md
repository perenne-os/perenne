# Roadmap

A living document. Think in **years, not months** — each phase is small, real, and finishable, and teaches the concept the next one needs. Every phase gets its own design → plan → build cycle (specs in `docs/superpowers/specs/`, plans in `docs/superpowers/plans/`).

## Phase 0 — Foundation & vision  *(done — 2026-06-06)*

- **Goal:** repository skeleton, founding documents, ADRs, a compiling Rust workspace, pinned toolchain, and a verified QEMU/RISC-V dev environment.
- **You learn:** the toolchain, Cargo workspaces, how the project is organized and why.
- **Done when:** the workspace builds and tests pass with one command, QEMU boots the RISC-V firmware, and the founding docs are complete. *(No kernel logic yet.)*

## Phase 1 — Hello world from our own kernel  *(done — 2026-06-09)*

- **Goal:** boot a tiny `no_std` kernel in QEMU and print to the screen.
- **You learn:** the boot process, freestanding Rust, linker scripts, SBI calls (and why `build-std` wasn't needed yet — see [learning note 0002](../learning/0002-boot-and-hello-world.md)).
- **Done when:** `./tools/run-qemu.ps1` loads our kernel and it prints "hello world".

## Phase 2 — The kernel grows up

- **Goal:** memory management, interrupt/trap handling, and basic scheduling — still in QEMU.
- **You learn:** virtual memory, traps, context switching.
- **Done when:** the kernel can manage memory and switch between simple tasks.

## Phase 3 — Security spine

- **Goal:** capability-based isolation and post-quantum crypto primitives, designed in from here.
- **You learn:** capabilities, IPC, integrating audited crypto.
- **Done when:** components run with least authority and a PQC primitive is usable.

## Phase 4 — Real hardware

- **Goal:** boot on real hardware — an owned x86-64 laptop (first port) or a cheap RISC-V board.
- **You learn:** real boot/firmware, hardware quirks, porting via `arch/` and `hal/`.
- **Done when:** the kernel boots on a physical machine.

## Phase 5 — Self-healing seed

- **Goal:** the first working diagnosis/knowledge system reading the `knowledge-base/` schema.
- **You learn:** deterministic rule engines, the safety-cage discipline.
- **Done when:** the OS can match a known issue to a playbook and apply a caged, reversible fix.

## Phase 6+ — Breadth

- **Goal:** more hardware (ARM/phones), a fuller HAL, device drivers, and the long tail.
- **Done when:** never, really — this is where it becomes a real, growing OS.
