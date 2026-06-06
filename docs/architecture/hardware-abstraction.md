# Hardware Abstraction

Goal: be **hardware-agnostic** — run across very different devices today, and accommodate hardware that barely exists yet, without rewriting the kernel.

## The two boundaries

1. **`arch/` — architecture-specific code.** The parts that differ per CPU instruction set (boot sequence, trap/interrupt handling, low-level CPU control). The first is `arch/riscv64`. Supporting x86-64 or ARM64 later means writing sibling crates here, not changing the kernel.
2. **`hal/` — the Hardware Abstraction Layer.** The device-agnostic interface the rest of the system talks to. Drivers and the kernel use the HAL's uniform vocabulary; the HAL maps it onto whatever real hardware is present.

The kernel above these boundaries should not know or care whether it is running on a phone, a laptop, an IoT sensor, or an emulator.

## Future hardware: accelerators are devices, not kernel targets

A common misconception is that exotic hardware (quantum processors, AI chips) needs special early support. It does not, because of what these devices actually *are*:

- **QPUs (quantum), NPUs/TPUs (AI), GPUs** are all **accelerators** — coprocessors. A normal CPU stays in charge and hands them work to do. You do **not** boot a kernel "on" a QPU any more than you boot one "on" a GPU.
- Therefore they require **no early investment**. The same clean HAL boundary that lets us support a network card lets a future accelerator register as "just another device" when the time comes.

This is the payoff of [principle #2](../vision/principles.md) (clean boundaries over chasing trends): we design the *boundary* now and implement specific accelerator support only when there's a concrete reason to — likely many years out.

## Portability strategy

- **RISC-V first**, developed in the QEMU emulator (see [ADR 0003](../decisions/0003-first-target-riscv.md)).
- **Other CPUs are ports, not rewrites.** Keeping architecture-specific code isolated under `arch/` and everything else portable means reaching an x86-64 laptop or an ARM64 device is a contained effort.
- We implement **one** architecture well before adding a second, to avoid spreading effort thin (a [non-goal](../vision/north-star.md) is supporting many architectures at once early).
