# Kernel — Phase 2a Design: Trap Handling & Timer Heartbeat

- **Date:** 2026-06-10
- **Status:** Approved — ready for implementation planning
- **Scope of this document:** Phase 2a only. During brainstorming, Phase 2
  ("the kernel grows up") was decomposed into three sub-phases, each with its
  own spec → plan → build cycle:
  - **2a — Trap handling & timer heartbeat** *(this spec)*
  - **2b — Memory management** (physical allocator, virtual memory/paging)
  - **2c — Basic scheduling** (context switching between simple tasks)

  The order is deliberate: traps come first because timer interrupts (2c) and
  page faults (2b) both need the handler infrastructure built here.

---

## 1. Goal

Give the kernel its reflexes: a supervisor-mode trap handler that catches
**exceptions** (synchronous faults like an illegal instruction) and
**interrupts** (asynchronous events like the timer), proven live in QEMU.

**You learn:** the RISC-V trap CSRs (`stvec`, `scause`, `sepc`, `stval`,
`sstatus`, `sie`), trap entry/exit assembly and context saving, and the SBI
TIME extension — the modern SBI calling convention, beyond Phase 1's legacy
console call.

**Done when** `./tools/test-qemu.ps1` observes, in one boot:
1. a deliberate breakpoint exception being caught, reported, and **recovered
   from** (execution continues past it), and
2. at least two periodic timer-tick lines (~1 per second).

## 2. Non-goals (deferred)

- **External interrupts / PLIC** (UART receive, devices) — deferred until
  drivers need them.
- **Vectored trap dispatch** (`stvec` vectored mode) — an optimization;
  direct mode is simpler and sufficient.
- **Nested traps / interrupts-during-trap** — interrupts stay disabled while
  handling a trap; a trap raised *inside* the handler is an unrecoverable
  bug (see §3.4).
- **A kernel-owned tick-policy hook** — the dispatcher handles ticks inline
  for now; the hook appears in 2c when scheduling needs it. No speculative
  abstraction.
- **Ecosystem crates** (`riscv`, `riscv-rt`) — everything is hand-rolled,
  like Phase 1's SBI wrappers, because the trap path *is* the lesson.

## 3. Design

### 3.1 Components

Mechanism lives in the arch crate beside `sbi.rs` and `console.rs`; the
kernel binary only orchestrates. All new bare-metal modules are gated to
`target_arch = "riscv64"` like the existing ones, so host builds and tests
stay green.

| Component | Location | Responsibility |
|-----------|----------|----------------|
| CSR accessors | `arch/riscv64/src/csr.rs` *(new)* | Hand-rolled inline-asm read/write for `stvec`, `sstatus`, `sie`, `sepc`, `scause`, `stval`, `time`. |
| Trap core | `arch/riscv64/src/trap.rs` *(new)* | Assembly entry/exit (`stvec` direct mode), the `TrapFrame`, `scause` decoding, and the Rust dispatcher. |
| Timer | `arch/riscv64/src/timer.rs` *(new)* | SBI TIME-extension `set_timer` wrapper; start/re-arm at a fixed interval; tick counting and the heartbeat print. |
| Orchestration | `kernel/src/main.rs` | `kmain`: init traps → trigger `ebreak` → start timer → park in `wfi`. |

### 3.2 Trap flow

```
trap fires (exception or interrupt)
  → CPU jumps to __trap_entry (assembly, stvec direct mode)
      → push TrapFrame on the interrupted stack:
        all 31 GPRs + sepc + sstatus + scause + stval
      → call trap_handler(&mut TrapFrame)   (Rust)
          → match decoded scause:
              breakpoint exception   → print diagnostic, advance sepc
                                       past the ebreak, return
              supervisor timer intr  → count tick, print heartbeat
                                       periodically, re-arm timer, return
              anything else          → panic with decoded cause + full
                                       register dump
      → restore all registers from the TrapFrame
  → sret (resumes at sepc)
```

The **full** frame (vs. caller-saved-only) costs a few extra instructions per
trap, but it is exactly the structure context switching reuses in 2c — saving
a rewrite.

`scause` decoding (interrupt bit, cause codes → a Rust enum) is pure code
with no CSR access, so it lives in its own testable function and gets
host-runnable unit tests.

### 3.3 Timer

- Uses the **SBI TIME extension** (EID `0x54494D45`), not the legacy
  `set_timer`, introducing the proper SBI v2 EID/FID convention.
- Enabled by setting `sie.STIE` and `sstatus.SIE` after the trap handler is
  installed — never before.
- SBI timers are one-shot: each tick handler re-arms the next deadline
  (current `time` + interval). The interval is a named constant derived from
  QEMU virt's 10 MHz timebase so the heartbeat prints about once per second.
  Reading the timebase from the device tree is deferred (real hardware,
  Phase 4); the constant is documented as QEMU-virt-specific.
- The parked `wfi` loop in `kmain` is unchanged — the hart now simply wakes
  on each tick, handles it, and sleeps again.

### 3.4 Error handling

- **Unexpected traps panic** with the decoded cause, `sepc`, `stval`, and a
  full `TrapFrame` dump. This is deliberately the project's first real crash
  diagnostic — every later phase benefits.
- The existing panic handler (print + park) is sufficient; a panic inside the
  trap handler simply prints and parks like any other.
- Known limitation (captured for 2c): interrupts stay masked inside the
  handler (`sstatus.SIE` is cleared on trap entry), but a *fault* raised by
  the handler or panic path would re-enter `__trap_entry` and recurse — and
  the single 64 KiB stack has no guard page, so deep recursion corrupts
  memory silently instead of fault-stopping. Unreachable in 2a's single
  deliberate `ebreak`; must be revisited before 2c enables preemption.
  Printing from trap context is safe: `console_putchar`'s `ecall` traps to
  M-mode (OpenSBI), not back into our S-mode vector.

## 4. Testing

Test-first, per house discipline:

- **QEMU smoke test** (`tools/test-qemu.ps1`, extended): boots the kernel
  and asserts (with timeout) that the output contains the breakpoint
  recovery line and ≥ 2 timer-tick lines. Written first; fails before
  implementation.
- **Host unit tests**: `scause` decoding (interrupt vs. exception bit, cause
  codes, unknown causes) tested on the host, like the existing arch-crate
  tests.

## 5. Deliverables

1. `csr.rs`, `trap.rs`, `timer.rs` in `arch/riscv64/src/`, wired into
   `lib.rs` behind the existing `target_arch` gate.
2. `kmain` exercising the breakpoint and starting the heartbeat.
3. Extended QEMU smoke test + host unit tests, all green.
4. Learning note `docs/learning/0003-traps-and-interrupts.md`.
5. `docs/roadmap/roadmap.md` updated to record the 2a/2b/2c decomposition
   and mark 2a's status.
6. Glossary entries for new terms (trap, exception, interrupt, CSR,
   trap frame, `sret`, …).

## 6. Open questions (for later sub-phases)

- Where the kernel-owned tick hook lives once 2c needs it (likely a
  callback or trait the kernel registers with the arch crate).
- Whether 2b (memory) introduces a dedicated trap stack — currently traps
  borrow the interrupted stack, which is fine while the kernel is the only
  thing running.
