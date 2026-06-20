# 0012 — A real UART console (Phase 4b)

**One-line:** the kernel now drives the serial port itself (a direct
ns16550 MMIO driver) instead of going through the SBI firmware console.

## What changed
- `dt::parse` also discovers the UART (`compatible = "ns16550a"` → base +
  reg-shift). New `uart::put` transmits a byte (poll the LSR `THRE` bit,
  write the THR register).
- `console` is now a switchable backend: SBI for the earliest boot lines,
  then the ns16550 once the device tree is parsed (`console::use_uart`).
- The UART's MMIO page is mapped `R-W-G` in `map_kernel_sections`, so it
  exists in the master table AND every per-task page table.

## Two ideas worth remembering
- **MMIO needs mapping.** With SBI, printing was an `ecall` (M-mode did the
  I/O). Driving the UART directly is an S-mode store to `0x1000_0000`, so
  once paging is on that page must be mapped — and in *every* address space,
  because the kernel prints from trap handlers (`tick:`, IPC) while a user
  task's `satp` is active.
- **Match by name vs by property.** `/memory` is found by node name (known
  at BEGIN_NODE); the UART is found by its `compatible` property, which can
  appear in any order, so the decision is committed when the node closes.

## Proof
Host test: the parser returns the UART base/reg-shift from the real QEMU
DTB. Smoke test: `console: ns16550a @ 0x10000000` then all output (including
`tick:` lines under a user satp) flows through the direct driver.

## Still QEMU-only
Next (Phase 4c) is the physical board boot — the first step needing hardware.
