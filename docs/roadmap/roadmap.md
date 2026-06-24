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

Decomposed (2026-06-20), continuing the RISC-V path (ADR 0003: one
architecture well before a second). The hardware-agnostic groundwork is done
first in QEMU; physical-board bring-up follows once a board is in hand.

### Phase 4a — Device-tree-driven discovery  *(done — 2026-06-20)*

- **Goal:** read RAM base/size and the timer frequency from the firmware's
  device tree instead of hardcoding QEMU's values.
- **You learn:** the FDT binary format and why a portable kernel discovers
  its machine from firmware (see [learning note 0011](../learning/0011-device-tree.md)).
- **Done when:** `./tools/test-qemu.ps1` (booting `-m 192M`) shows the kernel
  discover 192 MiB of RAM and the timebase from the device tree, with the
  parser host-tested against a real QEMU DTB. QEMU-only; no board.

### Phase 4b — Direct ns16550 UART console  *(done — 2026-06-20)*

- **Goal:** discover the UART from the device tree and drive it directly
  (MMIO), replacing the SBI firmware console.
- **You learn:** memory-mapped I/O and a UART's transmit path, mapping
  device memory into every address space, and matching a device-tree node by
  property vs name (see [learning note 0012](../learning/0012-uart-console.md)).
- **Done when:** `./tools/test-qemu.ps1` shows the console switch to the
  discovered ns16550 and all output flow through the direct driver. QEMU-only;
  no board.

### Phase 4c — Physical board boot

- **Goal:** an SD boot image (U-Boot/OpenSBI + kernel) on a real RISC-V
  board; board-specific UART/quirks surface here.
- **Done when:** the kernel boots and prints on real hardware.

(If the RISC-V board route stalls, the owned x86-64 laptop remains a
separate, larger option — a new `arch/x86_64` with its own boot/discovery.)

## User-space components — realizing ADR 0007

The payoff of the capability microkernel: features and drivers live *outside*
the kernel as capability-holding user-space components
([ADR 0007](../decisions/0007-extensibility-user-space-components.md)). Each
shrinks the trusted core and is bounded by what it was granted. This is also
the substrate the self-healer (Phase 5) runs on.

### First component — RTC time driver  *(done — 2026-06-20)*

- **Goal:** move a real driver out of the kernel — a U-mode component that
  owns the goldfish RTC (its MMIO mapped into that component only) and serves
  a time-read over a capability-checked endpoint.
- **You learn:** a driver is an unprivileged task with a device mapping + an
  IPC endpoint; isolation + capabilities bound its authority; and U-mode code
  must avoid kernel `.text`/`.rodata` (inline asm for MMIO; report via exit
  code) (see [learning note 0013](../learning/0013-first-user-space-component.md)).
- **Done when:** `./tools/test-qemu.ps1` shows the RTC component block on
  recv, then read and report the live clock on a client's capability-checked
  request, with a rogue (no capability) refused. QEMU-only; no board.

### Second component — virtio-rng entropy driver  *(done — 2026-06-21)*

- **Goal:** a more complex user-space driver — the QEMU virtio-rng device
  (virtqueue + DMA) — providing real hardware entropy that seeds ML-KEM,
  retiring Phase 3c's fixed seed.
- **You learn:** what a virtio device is (the virtio-mmio handshake, a split
  virtqueue as shared DMA memory), why identity mapping makes DMA trivial, and
  that even a complex driver fits the user-space-component model (see
  [learning note 0016](../learning/0016-virtio-rng-entropy.md)).
- **Done when:** `./tools/test-qemu.ps1` (with `-device virtio-rng-device`)
  shows the component draw two differing entropy samples and the ML-KEM-768
  round-trip succeed seeded by them. QEMU-only.

### call/reply IPC  *(done — 2026-06-21)*

- **Goal:** request/response IPC so a server returns a value to the task that
  called it, instead of via an exit code or a one-way send.
- **You learn:** `call` is an atomic send + await-reply; the kernel binds each
  reply to its caller with a back-pointer (no reply capability needed for a
  single-hart, one-call-at-a-time server) (see
  [learning note 0017](../learning/0017-call-reply-ipc.md)).
- **Done when:** `./tools/test-qemu.ps1` shows the RTC client `call` the server
  and exit with the returned live-clock value. QEMU-only.

### Kernel entropy pool  *(done — 2026-06-21)*

- **Goal:** a reseedable kernel CSPRNG seeded by the virtio-rng component,
  serving entropy on demand to kernel crypto — replacing the one-shot boot
  seed for ML-KEM.
- **You learn:** a kernel RNG is a CSPRNG seeded by a hardware source (a finite
  read becomes unlimited output); reseeding *mixes* new entropy into existing
  state rather than replacing it (see
  [learning note 0018](../learning/0018-kernel-entropy-pool.md)).
- **Done when:** `./tools/test-qemu.ps1` shows the pool seeded from virtio-rng,
  serving distinct on-demand draws, reseeded, and keying the ML-KEM round-trip.
  QEMU-only.

### U-mode getrandom service  *(done — 2026-06-21)*

- **Goal:** let U-mode components draw from the kernel entropy pool, gated by a
  capability.
- **You learn:** a kernel-owned CSPRNG is exposed to user space as a syscall
  (it can't run unprivileged), and a capability can gate an ordinary syscall —
  the same unforgeable-index check IPC uses (see
  [learning note 0019](../learning/0019-getrandom-service.md)).
- **Done when:** `./tools/test-qemu.ps1` shows a component refused without the
  capability and served with it. QEMU-only.

### PLIC interrupt path  *(done — 2026-06-23)*

- **Goal:** the first interrupt-driven device — drive the PLIC and let the
  virtio-rng component block on its IRQ instead of polling.
- **You learn:** an interrupt controller (claim/complete, per-context
  enable/threshold), and why a kernel/userspace interrupt split masks on
  deliver (via claim's in-service state) and re-arms on ack — the
  level-triggered line storms otherwise (see
  [learning note 0020](../learning/0020-plic-interrupts.md)).
- **Done when:** `./tools/test-qemu.ps1` shows the external interrupt wake the
  entropy component (which still seeds the pool / keys ML-KEM). QEMU-only.

### One-shot reply capabilities  *(done — 2026-06-23)*

- **Goal:** let a server hold multiple calls in flight and reply out of order,
  by minting a one-shot reply capability per received call (replacing the
  single-caller binding).
- **You learn:** tracking one outstanding reply as a *capability per call*
  (minted on receive, consumed on reply) instead of a field; and why a blocking
  kernel needs no staleness guard (see
  [learning note 0021](../learning/0021-reply-capabilities.md)).
- **Done when:** `./tools/test-qemu.ps1` shows a deferrer server answer two
  clients out of order, each exiting with its own reply value. QEMU-only.

(With reply caps done, the IPC + capability + device layers are mature enough
to build on. The next work is a single large arc — Phase 6 below — rather than
more one-off increments.)

## Phase 5 — Self-healing seed

The soul of the project ([ADR 0005](../decisions/0005-self-healing-knowledge-organism.md)):
the OS diagnoses and fixes its own issues, deterministically and inside a
safety cage. Decomposed (2026-06-20) in the trust-preserving order —
diagnose before act.

### Phase 5a — Detect + deterministic diagnosis  *(done — 2026-06-20)*

- **Goal:** when the kernel contains a crashed component, match the crash to
  a known issue (a compiled-in knowledge record) and log the diagnosis +
  playbook. No action.
- **You learn:** the containment path is the detection point; a deterministic,
  host-tested rule engine turns a fault into an explainable diagnosis (see
  [learning note 0014](../learning/0014-self-healing-diagnosis.md)).
- **Done when:** `./tools/test-qemu.ps1` shows a deliberately faulty
  component contained and diagnosed (matched to KB-0005), with the rest of
  the system running on. QEMU-only.

### Phase 5b — The caged fix  *(done — 2026-06-20)*

- **Goal:** an isolated, capability-gated **user-space** healer that the
  kernel notifies of a crash and that applies the playbook — a **bounded,
  reversible, logged** restart — recovering the component.
- **You learn:** isolation + a capability cage make an acting agent safe
  (agency in user space, enforcement in the kernel); a restart is re-forging
  the first-run context, with the launch generation handed to the task (see
  [learning note 0015](../learning/0015-self-healing-the-caged-fix.md)).
- **Done when:** `./tools/test-qemu.ps1` shows a `transient` component crash,
  get restarted by the healer, and run to completion (recovered), while an
  always-crashing `flaky` is restarted only up to the bound and then flagged.
  QEMU-only.

**Phase 5 (self-healing seed) is complete:** the OS detects, deterministically
diagnoses (5a), and applies a caged, bounded fix (5b) for a contained
component crash.

## Phase 6 — Persistent storage & the living knowledge base

**North star (chosen 2026-06-23):** make the self-healing knowledge organism
**real**. Today the runtime knowledge base is a single compiled-in stub
(`KB-0005` in `heal.rs`); the healer cannot read the actual
`knowledge-base/entries/*.md`, persist anything, or learn. This phase gives the
OS **persistent storage** and turns the organism into a real, growable memory
read from disk — the project's #1 differentiator
([ADR 0005](../decisions/0005-self-healing-knowledge-organism.md)).

A single coherent arc, pursued as a phase (not one-off candidates), decomposed
into three sub-phases — each its own design → plan → build cycle, each meatier
than the recent increments:

### Phase 6a — Block storage (virtio-blk)

- **Goal:** a virtio-blk driver — read and write disk sectors over a virtqueue,
  reusing the virtio transport and the PLIC interrupt path. A QEMU `-drive`
  backs it.
- **You learn:** block device I/O (request header + data + status descriptors),
  and how the same virtqueue machinery serves a very different device.
- **Done when:** `./tools/test-qemu.ps1` writes a sector and reads it back
  (round-trip), interrupt-driven. QEMU-only.

### Phase 6b — A minimal filesystem

- **Goal:** a simple filesystem over the block device — locate and read a file
  by name from a disk image built at boot time.
- **You learn:** the on-disk layout (superblock / directory / file extents) and
  the block-cache boundary between a filesystem and a block device.
- **Done when:** the kernel reads a named file's contents off the disk.

### Phase 6c — The living knowledge base

- **Goal:** the self-healer loads `knowledge-base/entries/*.md` from the
  filesystem, parses them, and diagnoses a contained crash against the **real,
  runtime** knowledge base instead of the compiled-in `KB-0005` — and (stretch)
  records a newly-seen issue back to disk.
- **You learn:** closing the self-healing loop with persistence — the organism
  reads and grows its own memory.
- **Done when:** a contained crash is diagnosed against a KB entry **loaded from
  disk**, proving the organism is no longer hardcoded.

## Phase 7+ — Breadth

- **Goal:** more hardware (ARM/phones), a fuller HAL, more device drivers, and
  the long tail — including the deferred capability-delegation and interactive-
  shell directions.
- **Done when:** never, really — this is where it becomes a real, growing OS.
