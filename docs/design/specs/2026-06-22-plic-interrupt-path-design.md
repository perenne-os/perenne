# Kernel — Design: the PLIC interrupt path (first interrupt-driven device)

- **Date:** 2026-06-22
- **Status:** Draft — awaiting review
- **Scope of this document:** drive the RISC-V PLIC (platform-level interrupt
  controller), handle a supervisor external interrupt, and let the U-mode
  virtio-rng entropy component **block for its device's interrupt** (via a
  capability-gated `wait_irq` syscall) instead of polling the virtqueue used
  ring. Fully QEMU-testable.

---

## 0. Where this sits

Every device interaction so far is **polled or synchronous**: the timer is an
SBI-arranged interrupt, but devices (RTC, virtio-rng) are polled, and IPC is a
synchronous rendezvous. The entropy component spins a used-ring poll loop
waiting for the device. This adds the first **asynchronous device interrupt**:
the PLIC routes the virtio-rng IRQ to the kernel, which wakes the blocked
driver — the missing core OS subsystem the roadmap names next.

It builds on: the trap handler (which already handles the timer interrupt and
decodes `scause`), the virtio-rng entropy component (the interrupt source and
the driver), the capability model, and the device-tree parser.

**Spike-verified facts** (kernel-side bring-up, this QEMU):
- PLIC base `0x0c00_0000`; hart 0's **S-mode context is 1**.
- Register offsets: priority `base + irq*4`; per-context enable bits `base +
  0x2000 + ctx*0x80 + (irq/32)*4`; threshold `base + 0x200000 + ctx*0x1000`;
  claim/complete `base + 0x200004 + ctx*0x1000`. For IRQ 8 / ctx 1: priority
  `+0x20`, enable `+0x2080` bit 8, threshold `+0x201000`, claim `+0x201004`.
- The virtio-rng device (slot `0x10008000`) is **IRQ 8** (DTB `interrupts`;
  each virtio-mmio slot's IRQ = its index + 1). Claiming returns `8`.
- virtio `InterruptStatus` `0x060` (bit 0 = used-buffer), `InterruptACK`
  `0x064` — the driver writes the status bits back to deassert.

## 1. Goal

The kernel drives the PLIC and handles a supervisor external interrupt; the
U-mode entropy component replaces its used-ring poll with a `wait_irq` syscall
(holding an `Interrupt` capability for its IRQ) and is woken by the kernel when
its device interrupts. The PLIC (shared infrastructure) is kernel-only; the
device (the component's MMIO) is acked by the component.

**You learn (kept brief):** how an interrupt controller routes a device IRQ to
the kernel (claim/complete, per-context enable/threshold), why a device
interrupt split between the kernel and a user-space driver must **mask the
source on delivery and unmask it when the driver is ready again** (or it
storms — the level-triggered line stays asserted until the driver acks the
device), and how a capability gates "wait for my interrupt" just as it gates
IPC and getrandom.

**Done when** `./tools/test-qemu.ps1` observes, alongside every existing
milestone:

1. **An interrupt-driven device** — the kernel logs the external interrupt
   waking the driver (`irq: external IRQ 8 woke 'entropy'`), and the entropy
   pool is still seeded and ML-KEM still keyed (the existing `entropy: pool …`
   and `pqc: …` lines) — proving the data flowed through the interrupt path,
   not the poll loop.

And off the bare target:

2. **Host unit tests** — `decode(scause|9) == SupervisorExternal`; the PLIC
   offset arithmetic for IRQ 8 / context 1; `interrupt_irq` cap lookup;
   `decode_syscall(10) == WaitIrq`.

## 2. Non-goals (deferred)

- **A general IRQ framework** — one device, one IRQ, one waiter; the PLIC
  driver targets hart 0's S-mode context only. Multi-hart / IRQ routing / nested
  priorities are out.
- **Interrupt-driving the other devices** (UART RX, RTC) — only virtio-rng.
- **Threaded/deferred interrupt handlers in the kernel** — the kernel handler
  only claims, masks, wakes the bound task, and completes; all device work
  stays in the U-mode driver.
- **Removing the timer interrupt path** — unchanged; this adds the *external*
  interrupt alongside it.
- **A shared "interrupt endpoint" / IPC-based delivery** — rejected in favor of
  a dedicated `wait_irq` (the unmask must pair with the wait).

## 3. Design

### 3.1 Components

| Component | Location | Responsibility |
|-----------|----------|----------------|
| PLIC driver | `arch/riscv64/src/plic.rs` *(new)* | Pure offset arithmetic (host-tested) + gated MMIO: `init`/`set_priority`/`enable`/`disable`/`claim`/`complete` for hart 0's S-mode context. |
| External-interrupt decode + handler | `arch/riscv64/src/trap.rs` | `Cause::SupervisorExternal` (scause code 9); the handler claims → masks → wakes the bound task → completes. |
| `sie.SEIE` enable | `arch/riscv64/src/csr.rs` | `sie_enable_external` (bit 9), alongside `sie_enable_timer`. |
| PLIC mapping | `arch/riscv64/src/mem/mod.rs` | Map the PLIC's pages R-W-G into the master table (and every per-task tree), like the UART. |
| `Capability::Interrupt` + `interrupt_irq` | `arch/riscv64/src/cap.rs` | New cap variant `Interrupt(u32)`; pure lookup (host-tested). |
| `WaitingIrq` + `wait_irq` + `wake_irq` | `arch/riscv64/src/task.rs`, `sched.rs` | New blocked state; the syscall service (unmask + park); the handler's wake. |
| `Syscall::WaitIrq` | `arch/riscv64/src/syscall.rs` | `a7 = 10` decode + dispatch. |
| DTB discovery | `arch/riscv64/src/dt.rs` | The PLIC base (`riscv,plic0` node) and each virtio-mmio node's IRQ (`interrupts`). |
| Driver rewire + wiring | `kernel/src/main.rs` | The entropy component blocks on `wait_irq` and acks the device; `kmain` sets up the PLIC and grants the `Interrupt` cap. |

### 3.2 The PLIC driver (`plic.rs`)

Pure const offset helpers (host-tested against the spike-verified values):
`priority_offset(irq) = irq*4`, `enable_offset(ctx, irq) = 0x2000 + ctx*0x80 +
(irq/32)*4`, `threshold_offset(ctx) = 0x200000 + ctx*0x1000`,
`claim_offset(ctx) = 0x200004 + ctx*0x1000`. `CONTEXT = 1` (hart 0 S-mode).

Gated functions over a stored `PLIC_BASE` (an `AtomicUsize`, set by `init`):
- `init(base)` — store base; set the context threshold to 0 (accept any
  priority). Leaves all sources **disabled** (the source enable is managed by
  `wait_irq`/the handler).
- `set_priority(irq, p)` — `p > 0` so the source can be delivered.
- `enable(irq)` / `disable(irq)` — set/clear the source's bit in the context
  enable word (read-modify-write).
- `claim() -> u32` — read the claim register (the highest-priority pending
  source for the context, or 0).
- `complete(irq)` — write `irq` back to the claim/complete register.

### 3.3 The interrupt → task path

- **`Cause::SupervisorExternal`** — `decode` maps `scause` interrupt code 9 to
  it (today it falls through to `Unknown → fatal`).
- **Handler** (in `trap_handler`):
  ```
  Cause::SupervisorExternal => {
      let irq = plic::claim();
      if irq != 0 {
          plic::disable(irq);              // mask: the device line is still
                                           // asserted until the U-mode driver acks
          if let Some(name) = sched::wake_irq(irq) {
              // one-shot log of the first delivery
          }
          plic::complete(irq);
      }
  }
  ```
- **`TaskState::WaitingIrq(u32)`** — a task blocked for IRQ `n`; `pick_next`
  skips it.
- **`sched::wake_irq(irq) -> Option<&'static str>`** — find the task in
  `WaitingIrq(irq)`, set it `Ready`, return its name (for the log); `None` if
  none waiting (the masked source stays pending and is redelivered at the next
  `wait_irq`).
- **`Capability::Interrupt(u32)`** + `cap::interrupt_irq(caps, idx) ->
  Option<u32>` — the unforgeable authority to wait on that IRQ.
- **`wait_irq(cap)` syscall** (`a7 = 10`, serviced in `sched`): look up the
  `Interrupt` cap → irq (else `a0 = usize::MAX`); `plic::enable(irq)` (unmask);
  `park_current(WaitingIrq(irq))`. Returns `a0 = 0` when woken. The syscall runs
  in the trap handler with interrupts off, so a now-pending interrupt is
  delivered only after the task has blocked — no lost or early wake.

**Why mask-on-deliver / unmask-on-wait:** the virtio line is level-triggered
and stays asserted until the driver writes `InterruptACK`. The driver acks in
U-mode, asynchronously, so the kernel cannot complete-and-leave-enabled (it
would re-fire in a storm). Masking on claim holds the source off until the
driver has acked and calls `wait_irq` again, which re-enables it. If no task is
waiting when an interrupt arrives, masking simply defers it to the next
`wait_irq` (no interrupt is lost).

### 3.4 The entropy component, rewired

Its per-draw loop changes from *poll the used ring* to *block on the interrupt,
then ack the device*:

```
… write descriptor + avail, mmio_w(QUEUE_NOTIFY, 0) …   // kick the device
sys_wait_irq(IRQ_CAP);                                   // block until interrupt
let status = mmio_r(INTERRUPT_STATUS);                   // 0x060
mmio_w(INTERRUPT_ACK, status);                           // 0x064 — deassert
… read the 4 words from the buffer, send to pqc …
```

The poll loop (`loop { fence; if dma_r16(used+2) == idx { break } }`) is
removed. The component already owns the virtio MMIO, so acking the device is
its job; it never touches the PLIC. It holds an `Endpoint(ENTROPY_EP)` cap (to
send) at cap slot 0 and an `Interrupt(rng_irq)` cap at cap slot 1.

### 3.5 Discovery and setup

- `dt::parse` gains `plic_base` (the `riscv,plic0` node's `reg`) and
  `virtio_mmio_irq: [u32; 8]` (each virtio-mmio node's `interrupts`, parallel to
  `virtio_mmio`). `virtio::find_rng(bases, irqs)` returns `(base, irq)`.
- `mem::init` takes the PLIC base, stores it, and `map_kernel_sections` maps the
  PLIC pages it uses (priority/enable around `base`, and the context-1
  threshold/claim page) R-W-G into every tree — the external interrupt fires
  while a U-mode task's `satp` is active.
- `kmain`: after discovering the RNG `(base, irq)`, `plic::init(plic_base)` +
  `plic::set_priority(irq, 1)`, enable `sie.SEIE`, grant the entropy component
  `Interrupt(irq)` at cap slot 1.

### 3.6 Error handling summary

| Situation | Behavior |
|-----------|----------|
| `wait_irq` with a `Interrupt` cap | Unmask the IRQ, block until it fires, return 0. |
| `wait_irq` with no/wrong cap | `a0 = usize::MAX`; not blocked. |
| External interrupt, no task waiting | Mask + complete; the still-asserted source is redelivered at the next `wait_irq` (no loss). |
| `claim()` returns 0 (spurious) | Do nothing. |
| Interrupt fires while the driver is between kick and `wait_irq` | Masked on claim (no waiter), then redelivered when `wait_irq` unmasks; the device line keeps it pending. |

## 4. Testing

- **Host unit tests** (`arch/riscv64`, `cargo test`):
  - `trap::decode(INTERRUPT_BIT | 9) == Cause::SupervisorExternal`.
  - `plic` offsets: `priority_offset(8) == 0x20`, `enable_offset(1, 8) ==
    0x2080`, `threshold_offset(1) == 0x201000`, `claim_offset(1) == 0x201004`.
  - `cap::interrupt_irq` — returns the irq for an `Interrupt` cap; `None` for
    wrong-type/empty/oob.
  - `syscall::decode_syscall(10) == Syscall::WaitIrq`.
  - `dt::parse` — `plic_base == 0x0c00_0000`; `virtio_mmio_irq` contains 8.
- **QEMU smoke test** (`tools/test-qemu.ps1`, extended; written first,
  failing): add `irq: external IRQ 8 woke 'entropy'`; keep the `entropy: pool …`
  / `pqc: …` lines (now delivered via the interrupt) and every other milestone.
- **Planning spike (done):** verified the PLIC base/offsets, the RNG IRQ (8),
  and that claiming returns it — before writing this.

## 5. Deliverables

1. `dt.rs`: `plic_base` + `virtio_mmio_irq`; host tests.
2. `plic.rs` (new): offset helpers (host-tested) + the gated driver; module
   declared in `lib.rs`.
3. `csr.rs`: `sie_enable_external`.
4. `mem/mod.rs`: map the PLIC pages.
5. `cap.rs`: `Capability::Interrupt(u32)` + `interrupt_irq` + host tests.
6. `task.rs`/`sched.rs`: `TaskState::WaitingIrq`, `wait_irq`, `wake_irq`.
7. `trap.rs`: `Cause::SupervisorExternal` + the handler; host test.
8. `syscall.rs`: `Syscall::WaitIrq` (`a7=10`) decode + dispatch + host test.
9. `virtio.rs`: `INTERRUPT_STATUS`/`INTERRUPT_ACK` constants.
10. `kernel/src/main.rs`: the entropy rewire (block on `wait_irq`, ack the
    device), the `Interrupt` grant, the PLIC setup + `sie.SEIE`, `sys_wait_irq`.
11. Extended QEMU smoke test + host tests, all green.
12. Short learning note `docs/learning/0020-plic-interrupts.md`.
13. Roadmap: the PLIC interrupt path marked done.
14. Glossary: only genuinely new terms (PLIC, IRQ, claim/complete).

## 6. Open questions (for later phases)

- **A general IRQ subsystem** (multiple devices/handlers, routing, priorities).
- **Interrupt-driven UART input** (a console RX path) and an RTC alarm.
- **Edge cases**: shared IRQs, spurious-interrupt counters, per-hart routing on
  an eventual multi-hart kernel.
