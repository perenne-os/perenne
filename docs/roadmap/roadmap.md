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

Decomposed into three sub-phases (2026-06-10), each with its own design → plan → build cycle. Traps come first because timer interrupts (2c) and page faults (2b) both need the handler infrastructure.

### Phase 2a — Trap handling & timer heartbeat  *(done — 2026-06-10)*

- **Goal:** a supervisor trap handler that catches and recovers from exceptions, plus SBI timer interrupts producing a ~1 Hz heartbeat.
- **You learn:** the trap CSRs (`stvec`, `scause`, `sepc`, …), trap entry/exit and context saving, the SBI TIME extension ([learning note 0003](../learning/0003-traps-and-interrupts.md)).
- **Done when:** `./tools/test-qemu.ps1` sees a survived breakpoint and ≥ 2 timer ticks in one boot.

### Phase 2b — Memory management  *(done — 2026-06-13)*

- **Goal:** physical frame allocation and virtual memory (paging) — the kernel manages its own address space.
- **You learn:** physical vs. virtual addresses, page tables, the MMU.
- **Done when:** `./tools/test-qemu.ps1` observes Sv39 paging active with W^X section permissions proven, frame alloc/free round-trip, and all Phase 2a milestones — in one boot.

### Phase 2c — Basic scheduling  *(done — 2026-06-14)*

- **Goal:** context switching between simple in-kernel tasks, driven by the timer from 2a.
- **You learn:** context switching, run queues, the tick-policy hook.
- **Done when:** `./tools/test-qemu.ps1` observes three tasks round-robin cooperatively, then a non-yielding task preempted by the timer — all in one boot, alongside the 2a/2b milestones.

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
