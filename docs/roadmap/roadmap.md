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

Decomposed into three sub-phases (2026-06-14), mirroring the 2a/2b/2c split.
Capabilities need an unprivileged component to *hold* them, so the privilege
transition comes first; PQC is the self-contained finale. Each sub-phase
gets its own design → plan → build cycle.

### Phase 3a — Privilege drop to user space  *(done — 2026-06-14)*

- **Goal:** the first transition to U-mode, a minimal `print`/`exit` syscall path, and proof the privilege boundary enforces.
- **You learn:** the U/S boundary (`sstatus.SPP`/`SPIE`, `sret`), the `sscratch` trap-stack swap, `ecall` syscalls and the ABI, the confused-deputy guard and `SUM`, the PTE `U` bit.
- **Done when:** `./tools/test-qemu.ps1` observes a U-mode task make a `print` syscall, exit cleanly, and a second U-mode task touching kernel memory contained — all in one boot, alongside the 2a/2b/2c milestones.

### Phase 3b — Capabilities & IPC

Decomposed (2026-06-14) into three sub-phases, mirroring 2a/2b/2c and
3a/3b/3c. The scheduling substrate comes first (something must *hold* a
capability and *be* a component), then isolation, then the capability/IPC
finale. Each sub-phase gets its own design → plan → build cycle.

#### Phase 3b-i — U-mode tasks in the run queue  *(done — 2026-06-14)*

- **Goal:** kernel and U-mode tasks share one round-robin run queue;
  per-task privilege is carried by the saved trapframe.
- **You learn:** forging a U-mode task's first run, scheduler-based
  termination, the `yield` syscall (see [learning note 0007](../learning/0007-user-scheduling.md)).
- **Done when:** `./tools/test-qemu.ps1` observes two U-mode tasks
  round-robin via `yield` and exit cleanly, a bad U-mode task contained
  while the scheduler keeps running, and a U-mode task preempted by the
  timer — all in one boot, alongside the 2a/2b/2c milestones.

#### Phase 3b-ii — Per-address-space isolation  *(done — 2026-06-19)*

- **Goal:** each component gets its own `satp`; the kernel is cloned into
  every address space; `satp` swaps on context switch.
- **You learn:** that `PTE_G` is only a TLB hint, building per-task page
  tables, swapping `satp` in the context switch (see
  [learning note 0008](../learning/0008-address-space-isolation.md)).
- **Done when:** `./tools/test-qemu.ps1` observes the 3b-i run-queue proofs
  (now each under its own `satp`) plus a `snoop` task that reaches into
  another task's memory being contained — all in one boot.

#### Phase 3b-iii — Capabilities + synchronous IPC + blocking  *(done — 2026-06-20)*

- **Goal:** unforgeable capability tokens, capability-checked syscalls, a
  synchronous send/recv endpoint, and blocking/wait-queue task states.
- **You learn:** capabilities as unforgeable table indices, the synchronous
  rendezvous, and blocking inside a syscall (see
  [learning note 0009](../learning/0009-capabilities-and-ipc.md)).
- **Done when:** `./tools/test-qemu.ps1` observes two isolated U-mode
  components communicating only through a capability-checked endpoint (the
  server blocks on recv; the client's value crosses address spaces and the
  server exits with it) and a rogue without the capability rejected — all in
  one boot.

With 3b-iii done, **Phase 3b (capabilities & IPC) is complete**, and the
Phase 3 security spine stands but for the PQC primitive (3c).

### Phase 3c — PQC primitive  *(done — 2026-06-20)*

- **Goal:** integrate an audited post-quantum crypto crate and expose one usable primitive (per ADR 0004).
- **You learn:** what a KEM is, integrating an external `no_std`/no-alloc crate into the bare kernel, and why the first real dependency means committing `Cargo.lock` (see [learning note 0010](../learning/0010-post-quantum-crypto.md)).
- **Done when:** `./tools/test-qemu.ps1` observes an ML-KEM-768 round-trip succeed on the bare kernel, with the wrapper host-tested (round-trip agrees, distinct seeds differ, a tampered ciphertext does not).

**Phase 3 (security spine) is complete:** U-mode (3a), the run queue + address-space isolation + capability-checked IPC (3b), and a post-quantum primitive (3c). Next is Phase 4 — real hardware.

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
