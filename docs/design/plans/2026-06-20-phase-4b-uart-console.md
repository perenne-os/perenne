# Phase 4b: Direct ns16550 UART console — Implementation Plan

> **For agentic workers:** Implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Discover the UART from the device tree, write a real MMIO ns16550 transmit driver, map its register page into every address space, and switch the kernel console off the SBI firmware path onto it. QEMU-testable; no board.

**Architecture:** Extend `dt::parse` to also return the UART base + reg-shift (matched by `compatible`, committed at `END_NODE`). A new `uart::put` does the ns16550 TX. `console` becomes a switchable backend (SBI → ns16550). `mem` maps the UART MMIO page `R-W-G` in `map_kernel_sections` (so it's in the master table and every per-task tree, since the kernel prints under user `satp`s). `kmain` flips the console after discovery.

**Tech Stack:** Rust `no_std`, RISC-V (riscv64gc), QEMU. Host tests: `cargo test -p kernel-arch-riscv64`. Bare: `cargo build -p kernel --target riscv64gc-unknown-none-elf`.

**Spec:** `docs/design/specs/2026-06-20-phase-4b-uart-console-design.md`

**Verified during planning:** the integrated `parse` below was run against the real QEMU DTB and returns `uart_base = 0x1000_0000`, `uart_reg_shift = 0` (plus the 4a RAM/timebase values). Key subtlety confirmed: the UART is matched by its `compatible` property (order-independent), so its `reg`/`reg-shift` are buffered per node and committed at `END_NODE` — unlike the name-matched `/memory` node.

---

## Task 1: extend the device-tree parser to discover the UART

**Files:**
- Modify: `arch/riscv64/src/dt.rs`

- [ ] **Step 1: Add the UART fields and update the host test (failing)**

In `arch/riscv64/src/dt.rs`, add fields to `MachineInfo`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MachineInfo {
    pub ram_base: usize,
    pub ram_size: usize,
    pub timebase_hz: u64,
    pub uart_base: usize,
    pub uart_reg_shift: u32,
}
```

Update the `parses_qemu_virt` test to assert the new fields:

```rust
    #[test]
    fn parses_qemu_virt() {
        let mi = parse(DTB).expect("should parse");
        assert_eq!(mi.ram_base, 0x8000_0000, "ram base");
        assert_eq!(mi.ram_size, 128 * 1024 * 1024, "ram size 128 MiB");
        assert_eq!(mi.timebase_hz, 10_000_000, "timebase 10 MHz");
        assert_eq!(mi.uart_base, 0x1000_0000, "uart base");
        assert_eq!(mi.uart_reg_shift, 0, "uart reg-shift");
    }
```

Run: `cargo test -p kernel-arch-riscv64 parses_qemu_virt`
Expected: FAIL — `MachineInfo` has no `uart_base`/`uart_reg_shift` yet (compile error).

- [ ] **Step 2: Add a `read_cells` helper and the UART parsing**

In `dt.rs`, add this helper just after `cstr_len`:

```rust
/// Read `cells` big-endian u32s starting at `off` in `val` as one integer.
fn read_cells(val: &[u8], off: usize, cells: usize) -> Option<u64> {
    let mut n: u64 = 0;
    for i in 0..cells {
        n = (n << 32) | be_u32(val, off + i * 4)? as u64;
    }
    Some(n)
}
```

Replace the body of `parse` with the integrated version (memory by name as
before; UART buffered per node and committed at `END_NODE`):

```rust
pub fn parse(dtb: &[u8]) -> Option<MachineInfo> {
    if be_u32(dtb, 0)? != FDT_MAGIC {
        return None;
    }
    let off_struct = be_u32(dtb, 8)? as usize;
    let off_strings = be_u32(dtb, 12)? as usize;

    let mut pos = off_struct;
    let mut depth: usize = 0;
    let mut is_mem = [false; 32];
    let mut addr_cells: u32 = 2;
    let mut size_cells: u32 = 2;
    let mut ram: Option<(usize, usize)> = None;
    let mut timebase: Option<u64> = None;

    // The UART is matched by its `compatible` property (which can appear in
    // any order within a node), so buffer per-node state and commit it when
    // the node closes.
    let mut node_is_uart = false;
    let mut node_reg: Option<usize> = None;
    let mut node_shift: u32 = 0;
    let mut uart: Option<(usize, u32)> = None;

    loop {
        let tok = be_u32(dtb, pos)?;
        pos += 4;
        match tok {
            FDT_BEGIN_NODE => {
                let name_len = cstr_len(dtb, pos)?;
                let name = dtb.get(pos..pos + name_len)?;
                depth += 1;
                if depth < is_mem.len() {
                    is_mem[depth] = name.starts_with(b"memory");
                }
                node_is_uart = false;
                node_reg = None;
                node_shift = 0;
                pos = (pos + name_len + 1 + 3) & !3;
            }
            FDT_END_NODE => {
                if node_is_uart && uart.is_none() {
                    if let Some(b) = node_reg {
                        uart = Some((b, node_shift));
                    }
                }
                depth = depth.checked_sub(1)?;
            }
            FDT_PROP => {
                let len = be_u32(dtb, pos)? as usize;
                let nameoff = be_u32(dtb, pos + 4)? as usize;
                let val_off = pos + 8;
                let val = dtb.get(val_off..val_off + len)?;
                let pname_len = cstr_len(dtb, off_strings + nameoff)?;
                let pname = dtb.get(off_strings + nameoff..off_strings + nameoff + pname_len)?;

                if depth == 1 && len >= 4 {
                    if pname == b"#address-cells" {
                        addr_cells = be_u32(val, 0)?;
                    } else if pname == b"#size-cells" {
                        size_cells = be_u32(val, 0)?;
                    }
                }
                if pname == b"timebase-frequency" && len >= 4 {
                    timebase = Some(be_u32(val, 0)? as u64);
                }
                if pname == b"reg" && len >= addr_cells as usize * 4 {
                    let base = read_cells(val, 0, addr_cells as usize)? as usize;
                    if depth < is_mem.len()
                        && is_mem[depth]
                        && len >= (addr_cells + size_cells) as usize * 4
                    {
                        let sz = read_cells(val, addr_cells as usize * 4, size_cells as usize)? as usize;
                        ram = Some((base, sz));
                    }
                    node_reg = Some(base);
                }
                if pname == b"compatible" && val.windows(7).any(|w| w == b"ns16550") {
                    node_is_uart = true;
                }
                if pname == b"reg-shift" && len >= 4 {
                    node_shift = be_u32(val, 0)?;
                }
                pos = (val_off + len + 3) & !3;
            }
            FDT_NOP => {}
            FDT_END => break,
            _ => return None,
        }
    }

    let (uart_base, uart_reg_shift) = uart?;
    Some(MachineInfo {
        ram_base: ram?.0,
        ram_size: ram?.1,
        timebase_hz: timebase?,
        uart_base,
        uart_reg_shift,
    })
}
```

Update the `parse` doc comment's first paragraph to mention it also returns
the UART (base + reg-shift), matched by `compatible`.

- [ ] **Step 3: Run the tests**

Run: `cargo test -p kernel-arch-riscv64 dt::`
Expected: PASS — `parses_qemu_virt` (now with UART), `rejects_bad_magic`,
`rejects_truncated_blob`. Then `cargo test -p kernel-arch-riscv64` → all green;
`cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf` → SUCCESS.

- [ ] **Step 4: Commit**

```bash
git add arch/riscv64/src/dt.rs
git commit -m "feat(dt): discover the ns16550 UART (base + reg-shift) from the device tree"
```

---

## Task 2: the ns16550 transmit driver

**Files:**
- Create: `arch/riscv64/src/uart.rs`
- Modify: `arch/riscv64/src/lib.rs` (declare the module)

- [ ] **Step 1: Create `arch/riscv64/src/uart.rs`**

```rust
//! ns16550 UART transmit driver (memory-mapped I/O).
//!
//! A 16550-compatible UART exposes byte registers; with `reg-shift = s`
//! they are spaced `1 << s` bytes apart. For output we use the transmit
//! holding register (THR, offset 0) and the line status register (LSR,
//! offset `5 << s`), whose THRE bit says THR can accept a byte. OpenSBI has
//! already configured the line (baud, 8N1); we only transmit.

/// LSR "transmit holding register empty" bit.
const LSR_THRE: u8 = 0x20;

/// Transmit one byte on the ns16550 at MMIO `base` (registers spaced by
/// `reg_shift`), spinning until the holding register is empty.
///
/// # Safety
/// `base` must be the MMIO base of an ns16550 UART that is mapped readable
/// and writable in the current address space.
#[cfg(target_arch = "riscv64")]
pub unsafe fn put(base: usize, reg_shift: u32, byte: u8) {
    let lsr = (base + (5usize << reg_shift)) as *const u8;
    let thr = base as *mut u8;
    // SAFETY: caller guarantees `base` is a mapped ns16550 register window;
    // THR/LSR are valid byte registers within it.
    unsafe {
        while core::ptr::read_volatile(lsr) & LSR_THRE == 0 {
            core::hint::spin_loop();
        }
        core::ptr::write_volatile(thr, byte);
    }
}
```

- [ ] **Step 2: Declare the module in `lib.rs`**

In `arch/riscv64/src/lib.rs`, add alongside the other gated modules (e.g.
after `pub mod timer;`):

```rust
#[cfg(target_arch = "riscv64")]
pub mod uart;
```

- [ ] **Step 3: Verify the arch crate builds for the bare target**

Run: `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf`
Expected: SUCCESS (no warnings — `put` is `pub`, so unused-until-Task-3 is fine).

Run: `cargo test -p kernel-arch-riscv64`
Expected: all green (host build skips the gated module).

- [ ] **Step 4: Commit**

```bash
git add arch/riscv64/src/uart.rs arch/riscv64/src/lib.rs
git commit -m "feat(uart): ns16550 transmit primitive (MMIO)"
```

---

## Task 3: make the console a switchable backend

**Files:**
- Modify: `arch/riscv64/src/console.rs`

- [ ] **Step 1: Replace `console.rs` with the switchable backend**

Replace the whole file with:

```rust
//! Kernel console: formatted text output, switchable between the SBI
//! firmware console (early boot) and a direct ns16550 UART (once the device
//! tree has been parsed — Phase 4b).
//!
//! `Console` implements `core::fmt::Write` by dispatching each byte to the
//! active backend, so the `print!`/`println!` macros work unchanged with no
//! allocator. `use_uart` flips the backend from SBI to the UART.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::sbi;

/// Active backend: `0` = SBI firmware console; non-zero = the ns16550 MMIO
/// base address. Starts at the SBI console so the earliest boot lines print
/// before the UART is discovered.
static UART_BASE: AtomicUsize = AtomicUsize::new(0);
static UART_SHIFT: AtomicUsize = AtomicUsize::new(0);

/// Switch console output to the discovered ns16550 UART. Until this is
/// called, output goes through the SBI firmware console.
pub fn use_uart(base: usize, reg_shift: u32) {
    UART_SHIFT.store(reg_shift as usize, Ordering::Relaxed);
    UART_BASE.store(base, Ordering::Relaxed); // store last: makes the switch visible
}

/// Zero-sized writer dispatching to the active console backend.
pub struct Console;

impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let base = UART_BASE.load(Ordering::Relaxed);
        if base == 0 {
            for b in s.bytes() {
                sbi::console_putchar(b);
            }
        } else {
            let shift = UART_SHIFT.load(Ordering::Relaxed) as u32;
            for b in s.bytes() {
                // SAFETY: `base` came from the device tree via use_uart, and
                // the UART page is mapped R+W in every address space (see
                // mem::map_kernel_sections).
                unsafe { crate::uart::put(base, shift, b) };
            }
        }
        Ok(())
    }
}

/// Implementation detail of `print!`/`println!`. Not for direct use.
pub fn _print(args: fmt::Arguments) {
    // Console output cannot fail; ignore the fmt::Result.
    let _ = Console.write_fmt(args);
}

/// Prints to the kernel console.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::console::_print(core::format_args!($($arg)*)));
}

/// Prints to the kernel console, with a trailing newline.
#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", core::format_args!($($arg)*)));
}
```

- [ ] **Step 2: Verify build + tests**

Run: `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf`
Expected: SUCCESS.

Run: `cargo test -p kernel-arch-riscv64`
Expected: all green.

- [ ] **Step 3: Commit**

```bash
git add arch/riscv64/src/console.rs
git commit -m "feat(console): switchable SBI -> ns16550 backend (use_uart)"
```

---

## Task 4: map the UART MMIO page into every address space

**Files:**
- Modify: `arch/riscv64/src/mem/mod.rs`

- [ ] **Step 1: Add a stored UART base**

In `arch/riscv64/src/mem/mod.rs`, near the `KERNEL_SATP` static, add:

```rust
/// MMIO base of the console UART (from the device tree), saved by [`init`]
/// so [`map_kernel_sections`] can map its page into the master table and
/// every per-task tree. Zero before `init`.
#[cfg(target_arch = "riscv64")]
static UART_MMIO_BASE: AtomicUsize = AtomicUsize::new(0);
```

- [ ] **Step 2: Map the UART page in `map_kernel_sections`**

Add the UART mapping at the end of the `unsafe` block in
`map_kernel_sections` (after the `__stack_start..__stack_top` map):

```rust
        paging::map_range(root, sym!(__stack_start), sym!(__stack_top), PTE_R | PTE_W | PTE_G);
        // Device MMIO: the console UART, one page, R-W-G (kernel-only, not
        // executable). Mapped in every tree because the kernel prints from
        // trap handlers while a user task's satp is active. Skipped if unset
        // (e.g. host/early paths) — start == 0 maps nothing here.
        let uart = UART_MMIO_BASE.load(Ordering::Acquire);
        if uart != 0 {
            paging::map_range(root, uart, uart + paging::PAGE_SIZE, PTE_R | PTE_W | PTE_G);
        }
```

- [ ] **Step 3: `init` takes `uart_base` and stores it before building tables**

Change `init`'s signature and store the UART base first (the rest of `init`
is unchanged):

```rust
#[cfg(target_arch = "riscv64")]
pub fn init(ram_end: usize, uart_base: usize) {
    use paging::{PTE_G, PTE_R, PTE_W};

    // SAFETY: all sym! calls read linker-script symbol addresses ...
    unsafe {
        // Record the UART base BEFORE building any page table, so
        // map_kernel_sections maps its page into the master table (and,
        // later, every per-task tree).
        UART_MMIO_BASE.store(uart_base, Ordering::Release);

        let free_ram = (sym!(__kernel_end), ram_end);
        frame::ALLOCATOR.with(|a| a.init(free_ram.0, free_ram.1));
        // ... rest of init unchanged ...
```

Update `init`'s doc comment to mention it also takes the UART MMIO base
(mapped into every address space for the direct console).

- [ ] **Step 4: Verify the arch crate builds + host tests**

Run: `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf`
Expected: SUCCESS. (The kernel binary will NOT build yet — `kmain` still
calls `mem::init(ram_end)` with one arg; fixed in Task 6. Build only the arch
crate here.)

Run: `cargo test -p kernel-arch-riscv64`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/mem/mod.rs
git commit -m "feat(mem): map the UART MMIO page R-W-G in every address space"
```

---

## Task 5: update the smoke test for the UART console (red)

**Files:**
- Modify: `tools/test-qemu.ps1`

- [ ] **Step 1: Add the console pattern**

In `tools/test-qemu.ps1`, add to the `$mustMatch` array (just before
`"dt: 192 MiB RAM"`):

```powershell
    "console: ns16550a @ 0x10000000",
```

Update the header comment and PASS message to mention the 4b milestone, e.g.
add: "and the Phase 4b milestone — a direct ns16550 UART driver (discovered
from the device tree) carries all console output (the SBI firmware console is
replaced)."

- [ ] **Step 2: Run the smoke test — expect RED**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: FAIL — `kmain` doesn't switch the console yet (and currently won't
build because `mem::init` now needs two args). Either way the
`console: ns16550a @ 0x10000000` line is absent.

- [ ] **Step 3: Commit (red test pinned)**

```bash
git add tools/test-qemu.ps1
git commit -m "test: smoke test asserts the direct ns16550 console (red)"
```

---

## Task 6: switch the console in `kmain` (green)

**Files:**
- Modify: `kernel/src/main.rs`

- [ ] **Step 1: Import `console` and wire the switch in**

In `kernel/src/main.rs`, add `console` to the arch `use` line:

```rust
    use kernel_arch_riscv64::{cap::Capability, console, dt, mem, println, sched, timer, trap};
```

In `kmain`, the 4a code currently reads (roughly):

```rust
        let machine = unsafe { dt::from_ptr(dtb) };
        println!(
            "dt: {} MiB RAM @ {:#x}, timebase {} Hz",
            machine.ram_size >> 20,
            machine.ram_base,
            machine.timebase_hz
        );

        mem::init(machine.ram_base + machine.ram_size);
```

Replace that with (switch the console right after discovery, then announce
it; pass the UART base to `mem::init`):

```rust
        let machine = unsafe { dt::from_ptr(dtb) };
        // Phase 4b: switch the console from the SBI firmware path to the
        // discovered UART. The MMU is still off, so these first direct
        // writes hit the UART's physical MMIO; mem::init maps the page next.
        console::use_uart(machine.uart_base, machine.uart_reg_shift);
        println!("console: ns16550a @ {:#x} (device tree)", machine.uart_base);
        println!(
            "dt: {} MiB RAM @ {:#x}, timebase {} Hz",
            machine.ram_size >> 20,
            machine.ram_base,
            machine.timebase_hz
        );

        mem::init(machine.ram_base + machine.ram_size, machine.uart_base);
```

- [ ] **Step 2: Build the kernel binary**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: SUCCESS.

- [ ] **Step 3: Run the smoke test — expect GREEN**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS` including `console: ns16550a @ 0x10000000`. Every
prior milestone still prints — now through the direct driver, including the
`tick:` lines emitted under a user task's `satp` (proving the UART page is
mapped in per-task trees). If output stops after the `console:` line, the
UART page mapping is wrong (a store fault on `0x10000000` once paging is on);
if it stops under scheduling, the per-task mapping is missing. Diagnose;
don't weaken the test.

- [ ] **Step 4: Commit**

```bash
git add kernel/src/main.rs
git commit -m "feat: Phase 4b live - kmain drives the console over the discovered ns16550 UART"
```

---

## Task 7: docs — short learning note, roadmap, glossary; final verification

**Files:**
- Create: `docs/learning/0012-uart-console.md`
- Modify: `docs/roadmap/roadmap.md`
- Modify: `docs/glossary.md`
- Modify: `docs/learning/README.md`

- [ ] **Step 1: Write a short learning note**

Create `docs/learning/0012-uart-console.md`:

```markdown
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
```

- [ ] **Step 2: Update the roadmap (4b done, 4c next)**

In `docs/roadmap/roadmap.md`, replace the `### Phase 4b — Real UART + board
bring-up` block with:

```markdown
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
separate, larger option — a new `arch/x86_64`.)
```

- [ ] **Step 3: Add glossary entries**

In `docs/glossary.md`, add entries (in the file's format) for: **MMIO
(memory-mapped I/O)** (device registers accessed as if they were memory at
fixed physical addresses; the kernel must map those pages to touch them in
S-mode), **UART** (the serial-port controller that turns bytes into the
serial signal — the kernel's text console), **ns16550** (the ubiquitous
16550-family UART, used by QEMU virt and many boards; programmed via a few
byte registers), and **`THR`/`LSR`/`THRE`** (the 16550 transmit-holding and
line-status registers, and the "holding register empty" status bit the
driver polls before sending). Reuse existing device-tree/SBI terms.

- [ ] **Step 4: Update the learning-notes index**

In `docs/learning/README.md`, add under the notes list:

```markdown
- [0012 — A real UART console (Phase 4b)](0012-uart-console.md)
```

- [ ] **Step 5: Final verification (run all, paste results)**

Run: `cargo test -p kernel-arch-riscv64` → expect all green.
Run: `cargo build --workspace` → expect success.
Run (PowerShell): `./tools/check-references.ps1` → expect PASS (fix any of YOUR broken references).
Run (PowerShell): `./tools/test-qemu.ps1` → expect `BOOT TEST PASS`.

- [ ] **Step 6: Commit**

```bash
git add docs/learning/0012-uart-console.md docs/roadmap/roadmap.md docs/glossary.md docs/learning/README.md
git commit -m "docs: Phase 4b learning note, roadmap (4b done), glossary terms"
```

---

## Done-when checklist (maps to spec §1)

- [ ] Console switches to the discovered UART — smoke pattern `console: ns16550a @ 0x10000000`; all later output flows through the direct driver.
- [ ] Trap-handler output under a user satp works — `tick: 2` still prints (proves the UART page is mapped in per-task trees).
- [ ] Host test — `parse()` returns `uart_base = 0x1000_0000`, `uart_reg_shift = 0` (plus RAM/timebase).
- [ ] `check-references` clean; `cargo build --workspace` green; `BOOT TEST PASS`.
```
