# Kernel — Phase 4a Design: Device-tree-driven hardware discovery

- **Date:** 2026-06-20
- **Status:** Draft — awaiting review
- **Scope of this document:** Phase 4a only — parsing the device tree (the
  `dtb` pointer OpenSBI passes to `kmain`) to discover RAM base/size and the
  timer frequency at boot, replacing the hardcoded QEMU `RAM_END` and
  `TIMEBASE_HZ`. Fully testable in QEMU; **no physical board required.**

---

## 0. Where 4a sits

Phase 4 ("real hardware") is large and forks on the target (the author owns
x86-64/ARM64 but not RISC-V — [ADR 0003](../decisions/0003-first-target-riscv.md)),
so it is decomposed. Per ADR 0003's "implement one architecture well before
a second" and the incremental-phase discipline, Phase 4 continues the
RISC-V path:

- **4a (this doc) — device-tree-driven discovery.** Remove the kernel's
  hardcoded QEMU machine specifics by reading them from the DTB. QEMU-only,
  no hardware.
- **4b — real UART + board bring-up.** Discover the console UART from the
  DTB and drive real serial hardware; boot on an actual RISC-V board (needs
  buying one) — deferred until hardware is in hand.
- Further board-quirk sub-phases as needed.

4a is the genuine first step of any real boot: real hardware does not have
QEMU's 128 MiB / 10 MHz, so the kernel must learn its machine from the
firmware-provided device tree rather than from constants.

## 1. Goal

The kernel discovers, at boot, from the **flattened device tree (FDT)** that
OpenSBI passes in `a1` (the `dtb` argument to `kmain`, currently ignored):

- **RAM** — base address and size (the `/memory` node's `reg`), replacing
  the hardcoded `RAM_END = 0x8800_0000` in `mem`.
- **Timer frequency** — `timebase-frequency`, replacing the hardcoded
  `TIMEBASE_HZ = 10_000_000` in `timer`.

A small hand-rolled `no_std` parser reads exactly these values. The kernel
then maps the discovered RAM and derives its heartbeat from the discovered
frequency.

**You learn (kept brief):** the FDT binary format (header, the
BEGIN_NODE/END_NODE/PROP token stream, the strings block, big-endian cells)
and why a portable kernel reads its memory map and clock rate from firmware
instead of hardcoding an emulator's.

**Done when** `./tools/test-qemu.ps1` — booting QEMU with a **non-default
`-m 192M`** — observes, alongside every existing milestone (greeting,
breakpoint recovery, paging, W^X, frame round-trip, the 3b IPC + 3c PQC
lines, ≥ 2 ticks):

1. **Dynamic RAM discovery** — a line `dt: 192 MiB RAM @ 0x80000000`
   (192, not the old hardcoded 128 — proving the value came from the DTB),
   and paging/frame allocation operate over that discovered RAM.
2. **Dynamic timebase** — a line reporting the discovered
   `timebase-frequency` (10 MHz on QEMU), and the heartbeat (≥ 2 ticks)
   runs off it.

And off the bare target:

3. **Host unit tests pass** — `parse()` on a committed real QEMU DTB
   returns `ram_base = 0x8000_0000`, `ram_size = 128 MiB`,
   `timebase_hz = 10_000_000`; a bad-magic blob returns `None`.

## 2. Non-goals (deferred)

- **Booting on physical hardware** — 4a is QEMU-only. Real board bring-up is
  4b (and needs a board).
- **UART/console from the DTB + a real UART driver** — the console still
  uses the existing SBI console path; discovering and driving the board's
  UART is **4b**.
- **Reserving the DTB / initrd memory regions** — 4a parses the DTB early
  (before the allocator), extracts the values, and does not touch the blob
  again, so it need not be reserved for this phase. (A robust kernel
  reserves it; deferred — noted.)
- **A full / generic device-tree API** — only the three values
  (`ram_base`, `ram_size`, `timebase_hz`) are parsed. No general node/prop
  query surface, no `#address/size-cells` beyond reading the root's, no
  per-CPU iteration, no multi-hart.
- **ACPI / x86 discovery** — RISC-V FDT only.
- **Robust fallback** — if the DTB is missing/invalid the kernel panics
  loudly (QEMU always supplies a valid one); no built-in default machine.

## 3. Design

### 3.1 Components

| Component | Location | Responsibility |
|-----------|----------|----------------|
| `MachineInfo` + `parse` | `arch/riscv64/src/dt.rs` *(new)* | Pure (`no_std`, host-tested): `parse(dtb: &[u8]) -> Option<MachineInfo>` where `MachineInfo { ram_base: usize, ram_size: usize, timebase_hz: u64 }`. |
| `from_ptr` | `arch/riscv64/src/dt.rs` (gated) | `from_ptr(ptr: usize) -> MachineInfo`: read the FDT header's `totalsize`, form a `&[u8]` of that length at `ptr`, call `parse`, and `expect` a valid result. |
| `mem::init(ram_end)` | `arch/riscv64/src/mem/mod.rs` | Replace the `RAM_END` constant with a parameter; free RAM = `__kernel_end .. ram_end`. |
| `timer::init(timebase_hz)` | `arch/riscv64/src/timer.rs` | Replace the `TIMEBASE_HZ`/`TICK_INTERVAL` constants with a value stored at init (an `AtomicU64` tick interval) and used by `arm_next`. |
| `kmain` | `kernel/src/main.rs` | Parse the DTB right after `trap::init()`; print the discovered machine; pass `ram_base + ram_size` to `mem::init` and `timebase_hz` to `timer::init`. |

### 3.2 The FDT, and what we read

The DTB is a big-endian binary: a header (magic `0xd00dfeed`, `totalsize`,
`off_dt_struct`, `off_dt_strings`, …), a **structure block** (a stream of
u32 tokens: `BEGIN_NODE` 0x1 + null-padded name, `END_NODE` 0x2, `PROP` 0x3
+ u32 len + u32 name-offset + value, `NOP` 0x4, `END` 0x9), and a **strings
block** (null-terminated property names, indexed by the PROP name-offset).

`parse` walks the structure block, tracking the current node name and depth:

- At the **root** node, read `#address-cells` and `#size-cells` (u32 each;
  default 2 and 2 per the FDT spec if absent) — for QEMU riscv64 these are
  2/2, i.e. 64-bit address and size cells.
- For the node whose name starts with `"memory"` (a child of root), read its
  `reg` property: `address-cells` u32s of base then `size-cells` u32s of
  size, big-endian → `ram_base`, `ram_size`.
- Read the `timebase-frequency` property (a u32 under `/cpus`) →
  `timebase_hz`.

`parse` returns `Some(MachineInfo)` once all three are found, else `None`
(invalid magic, truncation, or a missing value). It is a bounded walk over
the structure block with no allocation; every offset/length is checked
against the blob length so a malformed blob yields `None` rather than a
panic or out-of-bounds read.

### 3.3 Reading the blob safely (`from_ptr`)

The `dtb` argument is a physical pointer into RAM. `from_ptr` (gated,
`unsafe` internally) reads the 8-byte header prefix (magic + `totalsize`),
sanity-checks the magic, forms `core::slice::from_raw_parts(ptr, totalsize)`,
and hands that to the pure `parse`. It runs in `kmain` **before**
`mem::init` — while the MMU is still off, so the physical pointer is directly
readable, and before the frame allocator is armed, so the blob's frames are
untouched. The three discovered values are copied into Rust locals; the blob
is never read again, so it needs no post-paging mapping and no reservation
(§2). A `None` from `parse` (or a bad magic) panics with a clear message.

### 3.4 Wiring discovery into `mem` and `timer`

- **`mem::init(ram_end: usize)`** — the only change is the source of
  `ram_end`: the `RAM_END` constant is removed and the value is passed in
  (`MachineInfo.ram_base + MachineInfo.ram_size`). Everything else (kernel
  identity map, free-RAM map, W^X) is unchanged. (`ram_base` is also used to
  sanity-check/print; the kernel image already sits at 0x8020_0000 within
  RAM.)
- **`timer::init(timebase_hz: u64)`** — replaces the `TIMEBASE_HZ` /
  `TICK_INTERVAL` constants. The per-second tick interval (= `timebase_hz`)
  is stored in an `AtomicU64` at init; `arm_next` reads it. `timer::start`
  is unchanged otherwise; `kmain` calls `timer::init(mi.timebase_hz)` before
  `timer::start()`.

### 3.5 `kmain` ordering

```
kmain(hartid, dtb):
    println greeting
    trap::init()                      // no memory needed
    ebreak probe (unchanged)
    let mi = dt::from_ptr(dtb)        // MMU off: physical read of the FDT
    println!("dt: {} MiB RAM @ {:#x}, timebase {} Hz",
             mi.ram_size >> 20, mi.ram_base, mi.timebase_hz)
    mem::init(mi.ram_base + mi.ram_size)   // paging over discovered RAM
    ... wx_probe, frame_roundtrip, pqc_demo (unchanged) ...
    timer::init(mi.timebase_hz)
    ... spawn the 3b demo tasks ...
    timer::start()
    sched::enter()
```

Parsing before `trap::init`? No — keep `trap::init` first so any fault
during parsing is caught. Parsing happens after `trap::init` and before
`mem::init` (which needs `ram_end`).

### 3.6 Error handling summary

| Failure | Behavior |
|---------|----------|
| DTB magic wrong / blob truncated | `parse` → `None`; `from_ptr` panics "device tree invalid". |
| `/memory reg` or `timebase-frequency` missing | `parse` → `None`; `from_ptr` panics "device tree missing memory/timebase". |
| A length/offset in the blob exceeds its bounds | `parse` returns `None` (checked walk), never an OOB read. |
| Everything else (W^X probe, traps, etc.) | Unchanged. |

## 4. Testing

Test-first where it fits (the parser is the host-testable core):

- **Capture a fixture:** run `qemu-system-riscv64 -machine virt,dumpdtb=<file>`
  once to dump the real QEMU virt DTB; commit it under the repo (e.g.
  `arch/riscv64/tests/fixtures/qemu-virt.dtb`) and `include_bytes!` it.
- **Host unit tests** (`arch/riscv64`, `cargo test`):
  - `parse(fixture)` → `ram_base == 0x8000_0000`, `ram_size == 128 * 1024 *
    1024`, `timebase_hz == 10_000_000`.
  - `parse(&[0u8; 16])` (bad magic) → `None`.
  - a truncated copy of the fixture → `None` (bounded walk, no panic).
- **QEMU smoke test** (`tools/test-qemu.ps1`, extended; written first,
  failing): switch the boot to **`-m 192M`** and add `dt: 192 MiB RAM`
  (dynamic-discovery proof) plus the timebase line; keep every existing
  pattern (they must still pass over the discovered RAM/clock).

## 5. Deliverables

1. `arch/riscv64/src/dt.rs`: `MachineInfo`, `parse` (pure, tested),
   `from_ptr` (gated); module declared in `lib.rs`.
2. `mem::init(ram_end)` and `timer::init(timebase_hz)`; the `RAM_END` and
   `TIMEBASE_HZ`/`TICK_INTERVAL` constants removed.
3. `kmain` parses the DTB, prints the discovered machine, and wires the
   values into `mem`/`timer`.
4. A committed QEMU DTB fixture + host unit tests; the smoke test on
   `-m 192M` asserting dynamic discovery.
5. Short learning note `docs/learning/0011-device-tree.md`.
6. Roadmap: Phase 4 decomposed (4a/4b/…); 4a marked done with date.
7. Glossary: device tree / FDT, `timebase-frequency`, memory node — only
   genuinely new terms.

## 6. Open questions (for later sub-phases)

- **4b — real UART:** discover the console UART (compatible string + MMIO
  base) from the DTB and drive it directly; boot on a physical RISC-V board.
- **Reserving firmware regions:** honor the FDT memory-reservation block and
  the DTB/initrd ranges once the allocator must coexist with them.
- **Multi-hart / per-CPU:** parse all harts; bring up secondary harts.
- **The x86 alternative:** if the RISC-V board route stalls, the owned
  x86-64 laptop remains an option (a separate `arch/x86_64` with its own
  discovery via multiboot/UEFI/ACPI) — a much larger, separate effort.
