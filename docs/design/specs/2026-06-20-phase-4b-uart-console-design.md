# Kernel — Phase 4b Design: Direct ns16550 UART console

- **Date:** 2026-06-20
- **Status:** Draft — awaiting review
- **Scope of this document:** Phase 4b only — discovering the UART from the
  device tree, writing a real MMIO ns16550 driver, and switching the kernel
  console off the SBI firmware path onto it. Fully testable in QEMU; **no
  physical board required.**

---

## 0. Where 4b sits

Phase 4 ("real hardware") was decomposed (2026-06-20) along the RISC-V path:
**4a** (device-tree discovery — done) → **4b** (this doc) → **4c** (physical
board boot — deferred, needs hardware). 4a made the kernel read its RAM and
timebase from the firmware device tree. 4b adds the project's **first real
device driver**: a direct UART, discovered from the same device tree,
replacing the SBI firmware console.

The kernel already prints on real boards via the SBI console
(`sbi::console_putchar`, which OpenSBI provides everywhere), so 4b is not
about *being able* to print — it is about driving real hardware directly (a
step toward device drivers and the HAL), which is high-value and entirely
QEMU-testable. The actual silicon boot stays in 4c.

## 1. Goal

The kernel discovers its console UART from the device tree (QEMU virt:
`compatible = "ns16550a"`, `reg` base `0x1000_0000`, `reg-shift 0`), drives
it through a real MMIO **ns16550** transmit driver, and routes all console
output through it instead of the SBI firmware call.

**You learn (kept brief):** memory-mapped I/O and a real UART's transmit
path (poll the line-status `THRE` bit, write the holding register); that a
device's MMIO must be **mapped into the page table** once paging is on — and
into *every* address space, because the kernel prints from trap handlers
while a user task's `satp` is active; and one more device-tree wrinkle —
matching a node by a *property* (`compatible`) rather than its name.

**Done when** `./tools/test-qemu.ps1` observes, alongside every existing
milestone (greeting, breakpoint recovery, `dt:` line, paging, W^X, frames,
the 3b IPC + 3c PQC lines, ≥ 2 ticks):

1. **The console switches to the discovered UART** — a line
   `console: ns16550a @ 0x10000000 (device tree)`, after which **all** output
   flows through the direct driver. Every subsequent milestone line still
   appears (now produced by our ns16550 code), including the `tick:` lines —
   which the timer handler prints **while a user task's `satp` is active**,
   proving the UART MMIO is mapped in per-task address spaces.

And off the bare target:

2. **Host unit test** — `parse()` on the committed real QEMU DTB returns
   `uart_base = 0x1000_0000` and `uart_reg_shift = 0` (in addition to the
   existing RAM/timebase assertions).

## 2. Non-goals (deferred)

- **UART input / RX + interrupts** — output (TX) only. Reading keystrokes
  (and a serial IRQ) is later.
- **A HAL serial trait / device registry** — 4b ships a concrete ns16550
  driver. Abstracting console/serial behind a HAL interface waits until a
  second backend exists to justify it (the architecture docs: implement one
  well before abstracting). The `hal` crate stays a placeholder.
- **`/chosen stdout-path`** — the UART is found by matching
  `compatible = "ns16550"`/`"ns16550a"`, which is correct for QEMU virt and
  most boards. Following the firmware's explicitly-chosen console via
  `stdout-path`/aliases is a later refinement.
- **Non-ns16550 UARTs** (e.g. PL011, board-specific) — added when a board
  needs them (4c+).
- **Baud / line configuration** — OpenSBI already initializes the UART
  (8N1, baud); the driver only transmits. Reconfiguring is out of scope.
- **Physical-board boot** — 4c, and needs hardware.

## 3. Design

### 3.1 Components

| Component | Location | Responsibility |
|-----------|----------|----------------|
| UART discovery | `arch/riscv64/src/dt.rs` | Extend `MachineInfo` with `uart_base: usize` and `uart_reg_shift: u32`; in `parse`, find the `ns16550`-compatible node and read its `reg` base + `reg-shift`. Host-tested. |
| ns16550 TX driver | `arch/riscv64/src/uart.rs` *(new, gated)* | `put(base, reg_shift, byte)`: spin until `LSR.THRE`, then write `THR`. |
| Device MMIO mapping | `arch/riscv64/src/mem/mod.rs` | `init(ram_end, uart_base)`; `map_kernel_sections` also maps the UART page `R-W-G` (no `U`, no `X`) so it is present in the master table and every per-task tree. |
| Switchable console | `arch/riscv64/src/console.rs` | A backend selector (SBI by default, ns16550 once set); `use_uart(base, reg_shift)` flips it. |
| `kmain` | `kernel/src/main.rs` | After the DTB parse, switch the console to the UART and announce it; pass `uart_base` to `mem::init`. |

### 3.2 Discovering the UART (the property-match wrinkle)

4a's `parse` matches the `/memory` node by **name** (`"memory…"`), known at
`BEGIN_NODE`, so reading its `reg` regardless of property order is fine. The
UART must be matched by its **`compatible` property** — and an FDT node's
properties may appear in any order, so the `reg` can precede `compatible`.
The parser therefore buffers per-node state — a "this node is a UART" flag
(set when `compatible` contains `"ns16550"`), the node's `reg` base, and its
`reg-shift` — and **commits** them when the node closes (`END_NODE`): if the
node was a UART and had a `reg`, record `uart_base`/`uart_reg_shift`. The
memory walk is unchanged. `MachineInfo` gains `uart_base` and
`uart_reg_shift` (default shift 0 if the property is absent). `parse` now
requires RAM, timebase, **and** a UART, else returns `None`.

### 3.3 The ns16550 transmit path

A 16550 UART exposes byte registers; with `reg-shift = s` they are spaced
`1 << s` bytes apart (QEMU virt: `s = 0`). For transmit we need two:

- `THR` (transmit holding register) at offset `0`.
- `LSR` (line status register) at offset `5 << s`; bit 5 (`0x20`, `THRE`) is
  set when `THR` can accept a byte.

```
fn put(base, reg_shift, byte):
    let lsr = (base + (5 << reg_shift)) as *const u8
    while read_volatile(lsr) & 0x20 == 0 { spin }   // wait for THRE
    write_volatile(base as *mut u8, byte)            // write THR
```

Byte-width volatile MMIO access; no initialization (OpenSBI already
configured the line). Gated to `riscv64`; exercised by the smoke test.

### 3.4 Mapping the UART MMIO — and why in every address space

With the SBI console, output is an `ecall`: M-mode firmware performs the
MMIO, so the kernel needs no mapping. A direct driver writes `0x1000_0000`
**in S-mode**, so once paging is on that page must be mapped in the kernel
page table — as **device** memory: `R | W | G`, no `U` (kernel-only), no `X`.

Crucially, the kernel prints from **trap handlers** (the timer `tick:` line,
the IPC logs, `exit_current`) while a **user task's `satp`** is the active
address space. So the UART page must be mapped in *every* tree, not just the
master one. The clean place is `map_kernel_sections`, which both `init` (the
master table) and `build_user_space` (every per-task tree) already call:
add a UART-page mapping there. The base is a runtime value (from the DTB),
so `mem` stores it in a static (set by `init`, like `KERNEL_SATP`) that
`map_kernel_sections` reads.

```
mem::init(ram_end, uart_base):
    UART_MMIO_BASE.store(uart_base)        // before building any table
    ... arm allocator ...
    root = alloc; map_kernel_sections(root)   // maps the UART page too
    map free RAM; save kernel satp; satp_write

map_kernel_sections(root):
    ... text/rodata/data/stack as before ...
    let base = UART_MMIO_BASE.load()
    map_range(root, base, base + PAGE_SIZE, PTE_R | PTE_W | PTE_G)
```

(One 4 KiB page covers the ns16550's small register window.)

### 3.5 The switchable console

`console.rs` keeps the SBI path for early boot (before the UART is known)
and switches to the ns16550 once discovered. The selector is two atomics:

```
static UART_BASE: AtomicUsize = 0;   // 0 = use the SBI console
static UART_SHIFT: AtomicUsize = 0;

write_str(s):
    let base = UART_BASE.load();
    if base == 0 { for b in s.bytes(): sbi::console_putchar(b) }
    else { let sh = UART_SHIFT.load(); for b in s.bytes(): uart::put(base, sh, b) }

pub fn use_uart(base, reg_shift): UART_SHIFT.store(reg_shift); UART_BASE.store(base);
```

`_print` and the `print!`/`println!` macros are unchanged — they call
`write_str`, which now dispatches. Newline handling is unchanged (we do not
translate `\n`; OpenSBI didn't either, and QEMU's serial shows it fine).

### 3.6 `kmain` ordering

```
println greeting        // SBI (UART unknown)
trap::init(); ebreak; "survived breakpoint"   // SBI
let m = dt::from_ptr(dtb)                       // SBI; MMU off
console::use_uart(m.uart_base, m.uart_reg_shift)   // switch backend
println!("console: ns16550a @ {:#x} (device tree)", m.uart_base)  // FIRST UART line
println!("dt: {} MiB RAM @ ...", ...)
mem::init(m.ram_base + m.ram_size, m.uart_base)  // maps the UART page
... wx_probe, frame_roundtrip, pqc_demo ...
timer::init(m.timebase_hz)
... spawn tasks ...; timer::start(); sched::enter()
```

Switching the console **before** `mem::init` is safe: the MMU is still off,
so the early direct-UART writes hit physical `0x1000_0000` directly; once
`mem::init` maps the page, later writes (including under user `satp`s) work
through the mapping. No window is unmapped.

### 3.7 Error handling summary

| Failure | Behavior |
|---------|----------|
| Device tree missing a `ns16550` UART (or RAM/timebase) | `parse` → `None`; `from_ptr` panics (QEMU always has one; a real board without ns16550 is a 4c concern). |
| Kernel writes the UART before its page is mapped, post-paging | Cannot happen: `map_kernel_sections` maps it in the active table before paging is enabled, and in every per-task tree. |
| `THRE` never sets (broken UART) | `put` spins — acceptable for a console on QEMU; a real driver would time out (deferred). |
| Everything else (traps, W^X, etc.) | Unchanged. |

## 4. Testing

- **Host unit tests** (`arch/riscv64`): extend the existing DTB test —
  `parse(fixture)` now also asserts `uart_base == 0x1000_0000` and
  `uart_reg_shift == 0`; the bad-magic/truncated cases still return `None`.
- **QEMU smoke test** (`tools/test-qemu.ps1`, extended; written first,
  failing): add `console: ns16550a @ 0x10000000 (device tree)`; every prior
  pattern must still pass (now produced by the direct driver, including the
  `tick:` lines emitted under a user `satp`).

## 5. Deliverables

1. `dt.rs`: `MachineInfo.uart_base`/`uart_reg_shift` and the
   compatible-matched, `END_NODE`-committed UART discovery; host test updated.
2. `uart.rs` (new): the ns16550 `put` transmit primitive.
3. `mem`: `init(ram_end, uart_base)` and the UART-page mapping in
   `map_kernel_sections` (via a stored base).
4. `console.rs`: the SBI→ns16550 switchable backend + `use_uart`.
5. `kmain`: switch the console after discovery and announce it.
6. Extended QEMU smoke test + host unit test, all green.
7. Short learning note `docs/learning/0012-uart-console.md`.
8. Roadmap: 4b marked done with date; 4c (physical board) noted as next.
9. Glossary: UART, ns16550, MMIO (memory-mapped I/O), `THR`/`LSR`/`THRE` —
   only genuinely new terms.

## 6. Open questions (for later phases)

- **4c — physical board boot:** an SD boot image (U-Boot/OpenSBI + kernel)
  on a real RISC-V board; board-specific UART/`reg-shift`/quirks surface
  here. Needs hardware.
- **UART RX + a serial IRQ:** reading input; wiring the UART interrupt
  through the trap path.
- **A HAL serial/console interface:** once a second console backend (another
  UART type, or a framebuffer) exists, lift the concrete driver behind a HAL
  trait.
- **`stdout-path`:** honor the firmware's explicitly-chosen console device.
- **Device-memory attributes / a general MMIO mapping helper** as more
  devices arrive.
