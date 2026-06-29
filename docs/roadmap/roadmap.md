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

### Phase 6a — Block storage (virtio-blk)  *(done — 2026-06-24)*

- **Goal:** a virtio-blk driver — read and write disk sectors over a virtqueue,
  reusing the virtio transport and the PLIC interrupt path. A QEMU `-drive`
  backs it.
- **You learn:** block device I/O (a request is a *chain* — header + data +
  status descriptors, the data descriptor's `WRITE` flag flipping with
  direction), and how the same virtqueue machinery serves a very different
  device (see [learning note 0022](../learning/0022-block-storage.md)).
- **Done when:** `./tools/test-qemu.ps1` writes a sector and reads it back
  (round-trip), interrupt-driven, through an unprivileged driver. QEMU-only.

### Phase 6b — A minimal filesystem  *(done — 2026-06-25)*

- **Goal:** a simple filesystem over the block device — locate and read a file
  by name from a disk image built at boot time.
- **You learn:** the on-disk layout (superblock / directory / file extents) and
  the block-cache boundary between a filesystem and a block device — plus, in
  passing, a preemption-livelock fix (re-arm the timer on context switch) the
  first long-lived preemptible driver exposed (see
  [learning note 0023](../learning/0023-minimal-filesystem.md)).
- **Done when:** `./tools/test-qemu.ps1` (its disk image now built by the host
  `mkfs` tool) shows the kernel locate a file by name and read its multi-block
  contents off the disk — the `blk` driver now a call/reply read-block server,
  the kernel filesystem its client. QEMU-only.

### Phase 6c — The living knowledge base  *(done — 2026-06-25)*

- **Goal:** the self-healer loads `knowledge-base/entries/*.md` from the
  filesystem, parses them, and diagnoses a contained crash against the **real,
  runtime** knowledge base instead of the compiled-in `KB-0005`.
- **You learn:** closing the self-healing loop with persistence — the organism
  reads its own memory at boot, while diagnosis stays a pure in-kernel lookup
  (only its *data* moved to disk); plus where the code/disk line sits (the
  kernel decodes a raw trap into a token; the meaning keyed by that token is on
  disk) and reading the least that answers the question when each I/O is costly
  (see [learning note 0024](../learning/0024-living-knowledge-base.md)).
- **Done when:** ✅ a contained crash is diagnosed against a KB entry **loaded
  from disk** — the diagnosis prints `KB-0005`'s playbook text read off the
  image, selected by its `match-cause` token — proving the organism is no
  longer hardcoded. A new `match-cause` schema field + a `kernel-common::kb`
  frontmatter parser + a boot-time KB loader; `mkfs` packs the
  runtime-matchable entries; patients gate on the load. Write-back (recording a
  new issue to disk) is deferred — it needs a writable FS layer. QEMU-only.

## Phase 7 — Write-back & the learning organism  *(done — 2026-06-26)*

- **Goal:** the *write half* of Phase 6's storage arc. The filesystem gains an
  append-only write path, and the self-healer records a newly-seen crash class
  to disk — so a **second boot of the same image** loads that entry and
  diagnoses the formerly-novel crash. The organism learns across reboots, the
  payoff [ADR 0005](../decisions/0005-self-healing-knowledge-organism.md) is
  built around (and the half learning note 0024 named as deferred).
- **You learn:** that the kernel can *name* a symptom it has not *catalogued*
  (a novel crash = `cause_token` Some but `diagnose` None); crash-consistency
  from write *ordering* (data → directory → superblock-last commit point, no
  journal, no in-place mutation); and that the write — like 6c's read — happens
  off the crash path, gated ~one-block-per-tick by the `blk` IRQ-recovery
  constraint (see [learning note 0025](../learning/0025-write-back-learning-organism.md)).
- **Done when:** ✅ `./tools/test-qemu.ps1` runs **two boots over one disk
  image**: boot 1 (KB-0005 only) meets a novel illegal-instruction crash with no
  KB entry and `heal: recorded KB-0006 (illegal-instruction) to disk`; boot 2
  (the same image, not rebuilt) `heal: loaded 2 KB entries from disk` and
  `heal: diagnosed KB-0006 ...` — keyed by the entry the organism wrote itself
  the previous boot. New: `fs::append_plan` + `kb::serialize` (pure,
  host-tested), a `blk` write badge, a kernel append path (superblock-last
  commit), an `IllegalInstruction` cause + token, a novel-cause mailbox, and a
  KB-writer task; `mkfs` pads the image with spare capacity. Append-only
  (in-place updates, deletion, and a free-block allocator deferred). QEMU-only.

## Phase 8 — Capability delegation through IPC  *(done — 2026-06-27)*

- **Goal:** let a component delegate a capability it holds to another component
  at runtime, kernel-mediated and unforgeable — making the authority graph
  **dynamic** instead of frozen at boot. The foundational capability operation
  ADR 0007 (extensibility via capability-holding components) needs.
- **You learn:** that delegation is the kernel's existing reply-cap step made
  available to components (a component, not just the kernel, can install a cap
  into a peer); why it stays unforgeable (the kernel copies a cap the sender
  provably holds); and copy vs move/attenuated delegation (see
  [learning note 0026](../learning/0026-capability-delegation.md)).
- **Done when:** ✅ `./tools/test-qemu.ps1` shows a `broker` delegate the RTC
  endpoint capability to a `needy` client that holds **no** RTC cap (`cap:
  'broker' delegated Endpoint(0) to 'needy'`), which then reads the live clock
  through it (`sched: task 'needy' exited (code <ns>)`) — and a grant of a slot
  the broker doesn't hold is refused (`cap: 'broker' grant rejected (no
  capability in slot)`). New: a `grant` syscall (a7 = 11), pure `cap::cap_at`
  (the unforgeability guard), `ipc_grant` + `install_cap` riding the send/recv
  rendezvous, and `Task.pending_grant`. Copy semantics; move/attenuation/
  revocation deferred. QEMU-only.

## Phase 9 — Diagnosis-aware interactive shell  *(done — 2026-06-27)*

- **Goal:** the first interactive surface — a console whose commands let a human
  *interrogate the self-healing organism* (`kb` lists the loaded knowledge base,
  `diag` shows the last diagnosis). Introduces the project's first device
  **input** (UART receive). Principle #5 ("the OS should explain itself") made
  concrete.
- **You learn:** a UART receive path + a pure line discipline; that the shell is
  a kernel task because the console is kernel-owned; and — the real lesson — why
  *character input does not suit QEMU's edge-delivered PLIC* (it drops async
  re-assertions that one-shot completion IRQs never hit), so the console **polls**
  while rng/blk stay interrupt-driven (see
  [learning note 0027](../learning/0027-interactive-shell.md)).
- **Done when:** ✅ `./tools/test-qemu.ps1` shows the shell answer `help`/`kb`/
  `diag` against the organism with live data —
  `KB-0005  User-space component terminated by a fatal fault` and
  `last: KB-0005 -> Restart the component, up to a bounded number of retries.` A
  pure host-tested `LineBuffer`; `uart::get` + UART-IRQ device-tree discovery;
  `heal::entry`/`last_diagnosis`. The command pipeline is asserted via a boot
  self-demo (reliable serial-input injection isn't available in this CI harness;
  a live keystroke reaching the shell is verified manually). QEMU-only.

## Phase 10 — Revisable knowledge: the "seen N times" counter  *(done — 2026-06-27)*

- **Goal:** make the organism's memory **revisable in place** — the self-healer
  increments a per-entry "seen N times" counter on each recurring diagnosis and
  persists it on disk, so the count accumulates across reboots. The seed of the
  organism noticing *recurrence*.
- **You learn:** that **in-place mutation needs fixed-width fields** (a 5-digit
  `seen: 00000` keeps the update a same-length overwrite — no shifting, no
  rewrite, no free-block allocator); and the same off-the-crash-path persistence
  discipline as Phase 7 (see
  [learning note 0028](../learning/0028-revisable-kb.md)).
- **Done when:** ✅ `./tools/test-qemu.ps1` shows `heal: persisted KB-0005
  (seen 4)` on the first boot and `heal: persisted KB-0005 (seen 8)` on a second
  boot of the same image (the counter carried over and grew), with the shell `kb`
  command showing `KB-0005 (seen 8)`. A fixed-width `seen` field +
  `kb::set_seen_in_block` (pure, host-tested); `heal` per-entry counter + dirty
  flag; the KB-writer persists changed counters in place via `fs_write_block`.
  QEMU-only.

## Phase 11 — Counter-driven escalation  *(done — 2026-06-27)*

- **Goal:** the organism's first **adaptive** behavior — when an issue's
  cross-boot `seen` count crosses a threshold, escalate it (latch a chronic
  flag, persist it, report "flag for triage"). A decision driven by accumulated
  history.
- **You learn:** to make a decision **provably require persistent memory** (set
  the threshold above one boot's count, so escalation can only happen because
  the counter carried over); and that a second fixed-width field rides Phase
  10's in-place update path almost for free (see
  [learning note 0029](../learning/0029-counter-driven-escalation.md)).
- **Done when:** ✅ `./tools/test-qemu.ps1` shows KB-0005 reach `seen 4` on the
  first boot (not escalated) and, on a second boot of the same image, cross the
  threshold at `seen 6` → `heal: KB-0005 escalated (seen 6) -- recurring; flag
  for triage`, persisted (`persisted KB-0005 (seen 8, escalated)`). A
  fixed-width `escalated` flag + a generalized `kb` in-place writer; `heal`
  threshold + latch; the KB-writer persists both fields together. Escalation
  changes the organism's reporting/knowledge, not the restart cage. QEMU-only.

## Phase 12 — Act on escalation: quarantine a chronic fault  *(done — 2026-06-27)*

- **Goal:** the organism *acts* on what it learned — when a crash diagnoses to an
  escalated (chronic) issue, the kernel **quarantines** the component (stops the
  futile restart) instead of restarting it. Recognize chronic (11) → stop the
  futile fix (12).
- **You learn:** that the action needs **no new persistence** — quarantine is the
  behavioral consequence of Phase 11's persisted `escalated` flag, re-derived per
  crash; and that the action (like the escalation driving it) **requires
  cross-boot memory**, finally making the restart cage pay off across time (see
  [learning note 0030](../learning/0030-quarantine-on-escalation.md)).
- **Done when:** ✅ `./tools/test-qemu.ps1` shows `flaky` restarted-to-bound and
  flagged on the first boot (KB-0005 not yet escalated), but on a second boot of
  the same image — once KB-0005 escalates at `seen 6` —
  `heal: 'flaky' quarantined (KB-0005 chronic) -- not restarting` with **no**
  restart (the counter ends at 6, not 8). A single quarantine branch in
  `exit_current`, gated on the escalated flag; Phase 5b restart behavior for
  non-escalated issues is unchanged. Per-issue-class; per-component ledgers
  deferred. QEMU-only.

## Phase 13 — Capability revocation  *(done — 2026-06-27)*

- **Goal:** the kernel can **take back** delegated authority — revoke an endpoint
  capability so every holder's next use of it fails. The revocation half of a
  real capability system (the partner to Phase 8's delegation), and a concrete
  trust-posture guarantee: granted authority is controllable, not permanent.
- **You learn:** transitive revocation via a **sweep** (clear every `Endpoint(ep)`
  slot in every CSpace) needs no derivation tree — a cleared slot is just `None`,
  so the existing lookup already fails; authority to revoke is holding the cap;
  and the epoch/generation alternative (O(1), re-grantable) and its trade-offs
  (see [learning note 0031](../learning/0031-capability-revocation.md)).
- **Done when:** ✅ `./tools/test-qemu.ps1` shows a `lease_server` hand a `tenant`
  an endpoint capability, answer one call, then revoke it
  (`cap: 'lease' revoked endpoint 7 from 1 holder(s)`) — the tenant's second call
  is rejected and it exits with the marker (`sched: task 'tenant' exited
  (code 13)`). A pure `cap::revoke_in_caps`, `sched::revoke_endpoint`, and a
  `revoke` syscall (a7 = 12) authorized by holding the cap. Per-endpoint
  transitive sweep; epoch/CDT revocation deferred. QEMU-only.

## Phase 14 — Encrypted IPC channel  *(done — 2026-06-28)*

- **Goal:** put the post-quantum secret to work — the ML-KEM shared secret (built
  in 3c, unused until now) keys a ChaCha20-Poly1305 AEAD session, and two
  components exchange an authenticated-encrypted message over IPC.
- **You learn:** that AEAD gives confidentiality **and** integrity (a tampered
  ciphertext fails the tag); that crypto is capability-gated (a `Session` cap,
  like `Randomness` gates `getrandom`); the honest threat model (a kernel session
  service — the kernel sees plaintext; E2E-from-the-kernel needs U-mode crypto);
  and two bare-metal gotchas — ML-KEM keygen needs a big stack (establish at boot,
  not in the syscall path) and a 64-bit U-mode constant faults from a `.rodata`
  load (see [learning note 0032](../learning/0032-encrypted-ipc-channel.md)).
- **Done when:** ✅ `./tools/test-qemu.ps1` shows `crypto: channel session
  established (ML-KEM)`, the `opener` decrypt + verify + tamper-reject
  (`sched: task 'opener' exited (code 14)`), and the no-capability refusal
  (`crypto: 'nocap' seal refused …`, `exited (code 15)`). Audited
  `chacha20poly1305` (`no_std`); pure host-tested `seal`/`open`; a `channel`
  module + `Session`-gated `seal`/`open` syscalls. Kernel session service;
  fixed-seed at boot; U-mode/E2E crypto deferred. QEMU-only.

## Phase 15 — virtio-net + ARP: the OS's first network exchange  *(done — 2026-06-28)*

- **Goal:** open **networking** — a `virtio-net` driver brings up the NIC and does
  the OS's first network exchange: ARP-resolve the gateway (`10.0.2.2`),
  transmitting a request and parsing the reply (its MAC).
- **You learn:** the two-queue virtio device (RX/TX, a pre-posted receive
  buffer + the `virtio_net_hdr`); that ARP is the smallest self-contained network
  proof (QEMU SLIRP answers it, no IP/TCP stack needed); and a spike-validated
  device bring-up that reused the host-tested `arp` wire format (see
  [learning note 0033](../learning/0033-virtio-net-arp.md)).
- **Done when:** ✅ `./tools/test-qemu.ps1` (with `-netdev user -device
  virtio-net-device`) shows `net: resolved 10.0.2.2 -> 52:55:0a:00:02:02`. Pure
  host-tested `kernel_common::net` (ARP build/parse); a virtio-net two-queue
  bring-up; `mem::map_device`. The driver runs **in the kernel** for now — moving
  it to a user-space component (ADR 0007), like rng/blk, is a deferred refinement.
  QEMU-only.

## Phase 16 — The NIC becomes a user-space component  *(done — 2026-06-28)*

- **Goal:** pay back Phase 15's flagged deviation — relocate the `virtio-net`
  driver out of the kernel into an unprivileged, capability-holding **U-mode
  component** (ADR 0007), joining `rng`/`blk`, so the kernel never touches the NIC
  registers. Done before any IP/UDP stack grows on top.
- **You learn:** the `blk` model applied to a NIC — a U-mode component can't call
  the host-tested `kernel_common::net`, so the **driver does only raw device
  mechanics** over a shared identity-mapped DMA page while the **kernel
  `net_resolver` client** builds/parses the ARP frame (pure logic stays
  kernel-side); and two U-mode gotchas — no `for … in`/`Range` iterators in
  `.user_text` (they may not inline → a call into kernel `.text` → an
  `InstructionPageFault`), and a one-shot interrupt driver should `sys_exit` (not
  idle on `recv`) so it doesn't park its last PLIC claim in-service (see
  [learning note 0034](../learning/0034-user-space-nic.md)).
- **Done when:** ✅ `./tools/test-qemu.ps1` shows `ipc: 'net' blocks on recv`,
  `net: resolved 10.0.2.2 -> 52:55:0a:00:02:02`, and `sched: task 'net' exited
  (code 0)` — the gateway MAC produced by the **U-mode driver**, with the rest of
  the system (including the cross-boot KB write-back) running on. A U-mode
  `net_component` (`.user_text`, two-queue bring-up + IRQ-driven exchange) + a
  kernel `net_resolver`; `mem::map_device`/`net_resolve_gateway` retired;
  MAX_TASKS 25→27; the per-boot smoke deadline 60s→90s (the NIC's scheduled work
  shifts the demo a few ticks). QEMU-only.

## Phase 17 — Minimal IP/UDP stack: DHCP-learn-our-IP  *(done — 2026-06-29)*

- **Goal:** the first real protocol layer on the Phase 16 NIC — IPv4 + UDP — proven
  by a **DHCP** exchange: broadcast a DISCOVER and parse the OFFER, so the OS
  **learns its own IP (`10.0.2.15`) from the network** instead of hardcoding it
  (echoing Phase 4a reading RAM/timebase from firmware).
- **You learn:** that a packet is nested build/parse over one buffer
  (`Ethernet[ IPv4[ UDP[ DHCP ] ] ]`), each layer a pure slice-in/slice-out
  function (so the whole stack is host-testable with no device); the IPv4 header
  checksum (RFC 1071 one's-complement — a valid header re-checksums to 0, and UDP's
  checksum is optional over IPv4 so we send 0); why DHCP is the right first UDP
  milestone (SLIRP answers it **locally** — hermetic, unlike host-forwarded DNS —
  and it needs the BOOTP **broadcast flag** since we have no IP yet); and
  generalizing the NIC driver from one-shot to a **bounded server** (re-post RX per
  exchange, exit on a `NET_DONE` sentinel so the last device IRQ isn't parked
  in-service) (see [learning note 0035](../learning/0035-ip-udp-dhcp.md)).
- **Done when:** ✅ `./tools/test-qemu.ps1` shows both
  `net: resolved 10.0.2.2 -> 52:55:0a:00:02:02` and `net: dhcp offered 10.0.2.15`,
  then `sched: task 'net' exited (code 0)`, with the cross-boot self-healing demo
  still passing. New host-tested `kernel_common::net::{ipv4, udp, dhcp}`; the
  bounded-server `net_component`; `net_resolver` → `net_client` (ARP then DHCP). We
  *read* the offered IP but do not yet complete the lease (REQUEST/ACK) or adopt it
  (the source IP stays the hardcoded `10.0.2.15`). QEMU-only.

## Phase 18+ — Breadth

- **Goal:** the long tail — complete the DHCP lease (REQUEST/ACK) and **adopt** the
  offered IP; DNS over the same UDP layer; ICMP echo (ping); receiving unsolicited
  datagrams as an ongoing service (not a one-shot); encrypting UDP payloads with
  the Phase 14 channel; U-mode (end-to-end) crypto; epoch/generation revocation + a
  capability derivation tree; per-component crash ledgers; growable records (a
  free-block allocator, multi-block directories); more hardware (physical RISC-V
  board boot 4c, ARM/phones), a fuller HAL, and more device drivers.
- **Done when:** never, really — this is where it becomes a real, growing OS.
