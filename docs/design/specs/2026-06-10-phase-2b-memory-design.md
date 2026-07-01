# Kernel — Phase 2b Design: Memory Management

- **Date:** 2026-06-10
- **Status:** Draft — awaiting review
- **Scope of this document:** Phase 2b only (physical frame allocation +
  virtual memory). Decomposition context lives in the
  [2a spec](2026-06-10-phase-2a-traps-design.md) and the
  [roadmap](../../roadmap/roadmap.md).

---

## 1. Goal

The kernel manages its own memory: a physical **frame allocator** hands out
4 KiB frames, and **Sv39 paging** is enabled with the kernel identity-mapped
under real W^X permissions (`.text` executable but not writable, `.rodata`
read-only, data writable but not executable).

**You learn:** physical vs. virtual addresses, the three-level Sv39 page
table walk, PTE permission bits, `satp` and `sfence.vma`, and why W^X
matters.

**Done when** `./tools/test-qemu.ps1` observes, in one boot, *in addition to
all Phase 2a milestones* (greeting, breakpoint recovery, ≥ 2 ticks):
1. a line confirming Sv39 paging is enabled, with frame-allocator stats,
2. a deliberate write to `.rodata` being **blocked by the MMU and recovered
   from** (execution continues past it), and
3. a frame alloc → write → free → re-alloc round-trip succeeding.

## 2. Non-goals (deferred)

- **Kernel heap / `GlobalAlloc`** (Box, Vec in-kernel) — its own concept;
  introduced when something (likely 2c task creation) needs dynamic
  allocation.
- **Device-tree (DTB) parsing** — RAM is hardcoded as 128 MiB at
  `0x8000_0000`; the QEMU scripts pin `-m 128M` to guarantee it. DTB
  parsing arrives with real hardware (Phase 4).
- **Higher-half kernel** — the kernel stays identity-mapped (VA = PA).
  Moving it to high addresses is a deliberate migration when user space
  arrives (Phase 3).
- **Megapages (2 MiB / 1 GiB leaves)** — 4 KiB leaf pages everywhere keeps
  one code path; megapages are an optimization with no payoff at 128 MiB.
- **Demand paging, swapping, copy-on-write** — nothing to demand-page yet.
- **Multi-hart TLB shootdown** — single hart; a local `sfence.vma` is
  complete.
- **Buddy/contiguous allocation** — nothing needs multi-frame allocations;
  the bitmap allocator hands out single frames.
- **Dedicated trap stack** — traps keep borrowing the interrupted stack
  (2a's open question); revisit in 2c with context switching.

## 3. Design

### 3.1 Components

New `mem` module in the arch crate, following the `trap.rs` pattern: pure
logic ungated (host-testable), hardware access gated to
`target_arch = "riscv64"`.

| Component | Location | Responsibility |
|-----------|----------|----------------|
| Frame allocator | `arch/riscv64/src/mem/frame.rs` *(new)* | Bitmap allocator: pure core (bitmap + range math, host-tested) plus the gated static instance over physical RAM. |
| Paging | `arch/riscv64/src/mem/paging.rs` *(new)* | Sv39 PTE flags and VPN index math (pure, host-tested); `PageTable` type and `map_page` walker (gated). |
| Orchestration | `arch/riscv64/src/mem/mod.rs` *(new)* | `mem::init()`: layout from linker symbols → allocator init → build root table → enable `satp`. Stats accessors for `kmain` prints. |
| CSR accessor | `arch/riscv64/src/csr.rs` | Add `satp_write` (unsafe, takes mode + root PPN) alongside the existing accessors. |
| Linker script | `kernel/kernel.ld` | `ALIGN(4K)` between sections; `__text_start`-style start/end symbols per section; 4 KiB guard gap below the stack. |
| Trap integration | `arch/riscv64/src/trap.rs` | New `Cause` variants for page faults (codes 12/13/15); W^X-probe recovery; rich panic diagnostics for unexpected faults. |
| Kernel binary | `kernel/src/main.rs` | `kmain`: traps → **`mem::init()` → proof prints → W^X probe → frame round-trip** → timer → park. |

### 3.2 Physical memory layout

```
0x8000_0000 ─ OpenSBI (M-mode firmware)     — never managed, never mapped
0x8020_0000 ─ kernel image                  — identity-mapped per section:
                .text    R-X
                .rodata  R--
                .data/.bss/.stack  RW-
                (4 KiB unmapped guard page below the stack)
kernel_end  ─ managed frames (≈126 MiB)     — identity-mapped RW-,
              owned by the frame allocator
0x8800_0000 ─ end of RAM (128 MiB, pinned by -m 128M)
```

Leaving OpenSBI's region unmapped means a stray pointer into it page-faults
instead of silently reading firmware memory (OpenSBI also protects itself
via PMP, but defense in depth is free here). No MMIO is mapped: console and
timer go through SBI `ecall`s into M-mode, where `satp` does not apply.

The free-RAM region is mapped RW *eagerly* so frames handed out by the
allocator are immediately usable — no fault-and-map machinery needed.

### 3.3 Frame allocator (bitmap)

- One bit per 4 KiB frame; 128 MiB ⇒ 32 768 frames ⇒ a 4 KiB static bitmap.
- **Pure core** (`BitmapAllocator`): operates on the bitmap plus a base
  frame number and count — no pointers, no CSRs — so allocation, free,
  exhaustion, and misuse detection are all host unit tests.
- API: `alloc() -> Option<PhysFrame>` (first-fit scan, frame is zeroed by
  the gated wrapper before hand-out), `free(PhysFrame)`, `free_frames()` /
  `total_frames()` counts.
- **Misuse panics loudly**: freeing a frame that is already free, or outside
  the managed range, is a kernel bug — a panic with the offending address
  beats silent corruption. (This is the bitmap's edge over the classic
  intrusive free-list, which corrupts silently on double-free.)
- O(n) first-fit scan is acceptable at 32 768 frames; revisit only if
  profiling ever says otherwise.
- The static instance lives in a single-hart cell (an `UnsafeCell` wrapper,
  not an atomic — `timer.rs`'s `AtomicU64` trick doesn't transfer to a
  bitmap-plus-counters structure). Its safety argument: one hart, and trap
  context never allocates in 2b. The accessor documents that invariant and
  asserts against re-entry, so a future violation fails loudly.

### 3.4 Paging (Sv39)

- Three-level page tables, 4 KiB leaf pages only, 512 PTEs per table.
- Pure parts (host-tested): PTE flag constants (V/R/W/X/A/D/G), VPN index
  extraction from a virtual address, PTE ↔ physical-address packing.
- `map_page(root, va, pa, flags)` walks the tree, allocating intermediate
  table pages from the frame allocator (zeroed) as needed. Mapping an
  already-mapped page panics — in 2b every mapping is made exactly once, so
  a remap is a bug.
- Leaf PTEs set `A` and `D` up front: QEMU's MMU may fault (or trap to
  firmware) when they are clear rather than setting them in hardware, and
  we have no swapping that would want the information.
- Enable sequence in `mem::init()`: build the full root table first, then
  `sfence.vma` → `satp_write(Sv39 | root_ppn)` → `sfence.vma`. Identity
  mapping makes the switch safe: the instruction after the write fetches
  through the new tables at the same address.

### 3.5 Trap integration & the W^X probe

- `Cause` gains `InstructionPageFault` (12), `LoadPageFault` (13),
  `StorePageFault` (15) — decoded in the existing pure `decode()`.
- **W^X probe** (the 2b analogue of 2a's deliberate `ebreak`): `kmain` sets
  a static `EXPECTING_WX_FAULT` flag, then performs a volatile write to a
  `.rodata` address. The MMU raises a store page fault; the dispatcher sees
  the flag set, clears it, advances `sepc` past the store using the existing
  `instruction_len_at`, and returns. `kmain` confirms the flag was consumed
  and prints the proof line; if the write *didn't* fault, the flag is still
  set and `kmain` panics — W^X being broken must fail the boot test.
- Any page fault *without* the flag set is fatal: panic with decoded cause,
  `sepc`, and `stval` (the faulting address) — the full-frame dump already
  exists for unknown causes and is reused.
- `instruction_len_at`'s SAFETY comment ("no paging yet") is updated: still
  sound because `sepc` points into `.text`, which is mapped R-X (readable).

### 3.6 Stack guard page

The 2a spec documented that the 64 KiB boot stack has no guard page, so an
overflow corrupts memory silently. Paging fixes this almost for free: the
linker script places a 4 KiB gap below the stack, and the mapping loop
leaves it unmapped. An overflow now store-page-faults.

Honest caveat: the fault re-enters `__trap_entry`, whose own stack pushes
fault again (2a's known re-entry limitation); `sp` descends through the
guard (~14 frames) and lands on mapped memory below, corrupting a few
hundred bytes before the handler runs and panics with `stval` pointing into
the guard page. That is still a loud, attributable crash instead of silent
corruption — the full fix (a dedicated trap stack) stays with 2c.

### 3.7 Error handling summary

| Failure | Behavior |
|---------|----------|
| Unexpected page fault | Panic: decoded cause + `sepc` + `stval` + frame dump. |
| Frame exhaustion | `alloc()` returns `None`; `mem::init()` panics on it (no table memory = cannot boot); later callers decide for themselves. |
| Double-free / out-of-range free | Panic with the offending frame address. |
| Remap of a mapped page | Panic (every 2b mapping is made exactly once). |
| W^X probe doesn't fault | `kmain` panics — silently-broken W^X must not pass. |

## 4. Testing

Test-first, per house discipline:

- **QEMU smoke test** (`tools/test-qemu.ps1`, extended; written first,
  failing): pins `-m 128M`, keeps all 2a patterns, adds patterns for the
  paging-enabled line, the W^X-blocked line, and the frame round-trip line.
  `tools/run-qemu.ps1` gets the same `-m 128M` pin.
- **Host unit tests** (pure cores):
  - bitmap allocator: alloc/free round-trip, same-frame reuse after free,
    exhaustion returns `None`, double-free and out-of-range free panic,
    free counts;
  - paging math: VPN index extraction for the three levels, PTE flag
    packing, PTE ↔ physical address round-trip;
  - `decode()` for the three new page-fault causes.

## 5. Deliverables

1. `mem/` (frame.rs, paging.rs, mod.rs) in `arch/riscv64/src/`, plus the
   `csr.rs` and `trap.rs` extensions.
2. `kernel.ld` with 4 KiB-aligned, symbol-delimited sections and the stack
   guard gap; `kmain` exercising init, the W^X probe, and the frame
   round-trip.
3. Extended QEMU smoke test + host unit tests, all green.
4. Learning note `docs/learning/0004-memory-and-paging.md`.
5. Roadmap updated: 2b marked done with date.
6. Glossary entries (frame, page, page table, PTE, MMU, TLB, `satp`, Sv39,
   `sfence.vma`, identity mapping, W^X, guard page).

## 6. Open questions (for later phases)

- When the kernel heap arrives (likely 2c), does it sit on the frame
  allocator as a linked-list allocator, or something simpler?
- 2c must finally resolve the trap-stack question: context switches and
  preemption make "borrow the interrupted stack" untenable, and the guard
  page's recursion caveat (§3.6) goes away with it.
- Higher-half migration belongs to the phase that introduces user space
  (Phase 3): user VA range, kernel VA range, and the trampoline design.
