# Phase 2b — Memory Management Implementation Plan

> **For agentic workers:** Implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Physical frame allocation (bitmap) and Sv39 paging with the kernel identity-mapped under W^X permissions, proven live in QEMU.

**Architecture:** New `mem` module in the arch crate following the `trap.rs` pattern — pure logic (bitmap math, PTE encoding) ungated and host-tested, hardware access gated to `target_arch = "riscv64"`. The linker script gains 4 KiB-aligned, symbol-delimited sections and an unmapped stack guard page. `mem::init()` builds the page tables with frames from the allocator, then enables `satp`. The trap dispatcher learns page faults and a one-shot W^X probe (the 2b analogue of 2a's deliberate `ebreak`).

**Tech Stack:** Rust `no_std`, RISC-V Sv39, QEMU virt + OpenSBI, PowerShell test scripts.

**Spec:** `docs/design/specs/2026-06-10-phase-2b-memory-design.md`

**Conventions reminder:** commits use the house style (`feat(arch):`, `test:`, `docs:`), no AI co-author line. Host tests: `cargo test` (whole workspace works on Windows host). Cross-build: `cargo build -p kernel --target riscv64gc-unknown-none-elf`. Integration: `./tools/test-qemu.ps1`.

---

### Task 1: Failing QEMU smoke test (test-first)

**Files:**
- Modify: `tools/test-qemu.ps1`
- Modify: `tools/run-qemu.ps1`

- [ ] **Step 1: Pin RAM and add the Phase 2b patterns to the smoke test**

In `tools/test-qemu.ps1`, update the header comment (lines 1–5) to:

```powershell
# Boot smoke test: cross-builds the kernel, boots it headless under QEMU
# (riscv64 virt + OpenSBI), captures the serial console, and asserts the
# Phase 2a milestones (greeting, survived breakpoint, >= 2 timer ticks)
# plus the Phase 2b milestones (Sv39 paging on, W^X blocking a rodata
# write, a frame alloc/free round-trip).
# Usage: ./tools/test-qemu.ps1     (exit code 0 = pass, 1 = fail)
```

In the `Start-Process` argument list, add the RAM pin (the spec hardcodes
128 MiB instead of parsing the DTB, so the scripts must guarantee it):

```powershell
$qemu = Start-Process qemu-system-riscv64 -PassThru -NoNewWindow -ArgumentList @(
    "-machine", "virt",
    "-m", "128M",
    "-display", "none",
    "-serial", "file:$serialLog",
    "-bios", "default",
    "-kernel", $kernelElf
)
```

Replace the `$mustMatch` array with:

```powershell
$mustMatch = @(
    "hello world",
    "trap: breakpoint",
    "survived breakpoint",
    "paging: sv39 on",
    "wx: rodata write blocked",
    "frames: alloc/free ok",
    "tick: 2(?!\d)"
)
```

Update the PASS message:

```powershell
Write-Host "BOOT TEST PASS: 2a milestones plus paging, W^X, and frame round-trip all observed." -ForegroundColor Green
```

- [ ] **Step 2: Pin RAM in the run script**

In `tools/run-qemu.ps1`, change the QEMU invocation to:

```powershell
qemu-system-riscv64 -machine virt -m 128M -nographic -bios default `
    -kernel "$repo/target/riscv64gc-unknown-none-elf/debug/kernel"
```

- [ ] **Step 3: Run the test to verify it fails on exactly the new patterns**

Run: `./tools/test-qemu.ps1`
Expected: `BOOT TEST FAIL: missing within 30s: paging: sv39 on, wx: rodata write blocked, frames: alloc/free ok` (all 2a patterns still observed).

- [ ] **Step 4: Commit**

```powershell
git add tools/test-qemu.ps1 tools/run-qemu.ps1
git commit -m "test: extend QEMU smoke test for Phase 2b paging milestones (failing)"
```

---

### Task 2: Bitmap frame allocator — pure core (TDD)

**Files:**
- Create: `arch/riscv64/src/mem/mod.rs`
- Create: `arch/riscv64/src/mem/frame.rs`
- Modify: `arch/riscv64/src/lib.rs`

- [ ] **Step 1: Wire the module skeleton**

In `arch/riscv64/src/lib.rs`, after the `pub mod trap;` declaration, add:

```rust
/// Memory management: bitmap frame allocator and Sv39 paging. Pure logic (bitmap and PTE math, host-testable); the gated parts (statics, page-table walker, satp) live inside.
pub mod mem;
```

Create `arch/riscv64/src/mem/mod.rs`:

```rust
//! Memory management: physical frame allocation and Sv39 paging.
//!
//! Same layout discipline as [`crate::trap`]: pure logic is ungated and
//! unit-tested on the host; everything touching real memory or CSRs is
//! gated to `target_arch = "riscv64"`.

pub mod frame;
```

- [ ] **Step 2: Write the failing tests**

Create `arch/riscv64/src/mem/frame.rs` with the doc comment, types, *unimplemented* methods, and the full test module:

```rust
//! Physical frame allocation: a bitmap over 4 KiB frames.
//!
//! One bit per frame (set = allocated); 128 MiB of RAM needs only a
//! 4 KiB bitmap. Chosen over the classic intrusive free-list because the
//! core is pure logic (host-testable) and misuse — double-free,
//! out-of-range free — panics loudly instead of corrupting silently.

/// Size of one physical frame (and one page): 4 KiB.
pub const FRAME_SIZE: usize = 4096;

/// Worst case the bitmap must cover: 128 MiB / 4 KiB.
const MAX_FRAMES: usize = 32_768;

/// A physical frame, identified by its 4 KiB-aligned base address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysFrame(pub usize);

/// Bitmap allocator over a contiguous range of physical frames.
/// Pure logic: no pointers, no CSRs — everything here runs on the host.
pub struct BitmapAllocator {
    /// Bit `i` set = frame `base + i` is allocated.
    bitmap: [u64; MAX_FRAMES / 64],
    /// Frame number (address / FRAME_SIZE) of the first managed frame.
    base: usize,
    /// Number of managed frames.
    count: usize,
    /// Number of currently free frames.
    free: usize,
}

impl BitmapAllocator {
    /// An empty allocator managing nothing; [`init`](Self::init) arms it.
    pub const fn new() -> Self {
        Self { bitmap: [0; MAX_FRAMES / 64], base: 0, count: 0, free: 0 }
    }

    /// Manage the frames in `[start_addr, end_addr)`. Both must be
    /// 4 KiB-aligned; the range must fit the bitmap.
    pub fn init(&mut self, start_addr: usize, end_addr: usize) {
        todo!()
    }

    /// Hand out the lowest free frame (first-fit), or `None` when empty.
    pub fn alloc(&mut self) -> Option<PhysFrame> {
        todo!()
    }

    /// Return a frame. Panics on misuse — an unaligned address, an
    /// unmanaged frame, or a double free is a kernel bug worth a loud
    /// stop, not silent corruption.
    pub fn free(&mut self, frame: PhysFrame) {
        todo!()
    }

    /// Frames currently free.
    pub fn free_frames(&self) -> usize {
        self.free
    }

    /// Frames managed in total.
    pub fn total_frames(&self) -> usize {
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 256 managed frames starting where the kernel image would end.
    fn allocator() -> BitmapAllocator {
        let mut a = BitmapAllocator::new();
        a.init(0x8030_0000, 0x8040_0000);
        a
    }

    #[test]
    fn first_alloc_is_the_lowest_frame() {
        let mut a = allocator();
        assert_eq!(a.alloc(), Some(PhysFrame(0x8030_0000)));
        assert_eq!(a.alloc(), Some(PhysFrame(0x8030_1000)));
    }

    #[test]
    fn free_then_realloc_recycles_the_same_frame() {
        let mut a = allocator();
        let f = a.alloc().unwrap();
        a.alloc().unwrap();
        a.free(f);
        assert_eq!(a.alloc(), Some(f), "first-fit must reuse the freed hole");
    }

    #[test]
    fn exhaustion_returns_none() {
        let mut a = BitmapAllocator::new();
        a.init(0x8030_0000, 0x8030_2000); // exactly 2 frames
        assert!(a.alloc().is_some());
        assert!(a.alloc().is_some());
        assert_eq!(a.alloc(), None);
    }

    #[test]
    fn counts_track_alloc_and_free() {
        let mut a = allocator();
        assert_eq!(a.total_frames(), 256);
        assert_eq!(a.free_frames(), 256);
        let f = a.alloc().unwrap();
        assert_eq!(a.free_frames(), 255);
        a.free(f);
        assert_eq!(a.free_frames(), 256);
    }

    #[test]
    #[should_panic(expected = "double free")]
    fn double_free_panics() {
        let mut a = allocator();
        let f = a.alloc().unwrap();
        a.free(f);
        a.free(f);
    }

    #[test]
    #[should_panic(expected = "unmanaged")]
    fn out_of_range_free_panics() {
        let mut a = allocator();
        a.free(PhysFrame(0x8800_0000));
    }

    #[test]
    #[should_panic(expected = "unaligned")]
    fn unaligned_free_panics() {
        let mut a = allocator();
        a.free(PhysFrame(0x8030_0008));
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p kernel-arch-riscv64`
Expected: FAIL — every new test panics at `todo!()` (the `should_panic` tests fail because the panic message doesn't match).

- [ ] **Step 4: Implement the three methods**

Replace the three `todo!()` bodies:

```rust
    /// Manage the frames in `[start_addr, end_addr)`. Both must be
    /// 4 KiB-aligned; the range must fit the bitmap.
    pub fn init(&mut self, start_addr: usize, end_addr: usize) {
        assert!(start_addr % FRAME_SIZE == 0, "unaligned start {start_addr:#x}");
        assert!(end_addr % FRAME_SIZE == 0, "unaligned end {end_addr:#x}");
        assert!(start_addr < end_addr, "empty range");
        let count = (end_addr - start_addr) / FRAME_SIZE;
        assert!(count <= MAX_FRAMES, "range exceeds bitmap capacity");
        self.base = start_addr / FRAME_SIZE;
        self.count = count;
        self.free = count;
    }

    /// Hand out the lowest free frame (first-fit), or `None` when empty.
    /// The O(n) scan is irrelevant at 32k frames; revisit only if
    /// profiling ever disagrees.
    pub fn alloc(&mut self) -> Option<PhysFrame> {
        for i in 0..self.count {
            let (word, bit) = (i / 64, i % 64);
            if self.bitmap[word] & (1 << bit) == 0 {
                self.bitmap[word] |= 1 << bit;
                self.free -= 1;
                return Some(PhysFrame((self.base + i) * FRAME_SIZE));
            }
        }
        None
    }

    /// Return a frame. Panics on misuse — an unaligned address, an
    /// unmanaged frame, or a double free is a kernel bug worth a loud
    /// stop, not silent corruption.
    pub fn free(&mut self, frame: PhysFrame) {
        assert!(frame.0 % FRAME_SIZE == 0, "free of unaligned address {:#x}", frame.0);
        let n = frame.0 / FRAME_SIZE;
        assert!(
            n >= self.base && n < self.base + self.count,
            "free of unmanaged frame {:#x}",
            frame.0
        );
        let i = n - self.base;
        let (word, bit) = (i / 64, i % 64);
        assert!(self.bitmap[word] & (1 << bit) != 0, "double free of frame {:#x}", frame.0);
        self.bitmap[word] &= !(1 << bit);
        self.free += 1;
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p kernel-arch-riscv64`
Expected: PASS (all new tests plus the existing trap/arch tests).

- [ ] **Step 6: Verify the cross-build still compiles**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: success.

- [ ] **Step 7: Commit**

```powershell
git add arch/riscv64/src/lib.rs arch/riscv64/src/mem
git commit -m "feat(arch): bitmap frame allocator pure core with host tests"
```

---

### Task 3: SingleHartCell — interior mutability with a re-entry tripwire (TDD)

**Files:**
- Modify: `arch/riscv64/src/mem/mod.rs`

- [ ] **Step 1: Write the failing tests**

Append to `arch/riscv64/src/mem/mod.rs` (type stub plus tests; `with` body is `todo!()` for now):

```rust
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

/// Interior mutability for a single-hart kernel: gives `&mut` access to
/// a static without a real lock. `timer.rs` gets away with a bare
/// `AtomicU64`, but a bitmap-plus-counters struct can't be a single
/// atomic, so the exclusivity argument moves here and is enforced:
/// re-entrant access (e.g. a trap handler interrupting a call in
/// progress) panics. In 2b trap context never allocates; the panic
/// keeps that invariant honest if a later phase forgets.
pub struct SingleHartCell<T> {
    inner: UnsafeCell<T>,
    in_use: AtomicBool,
}

// SAFETY: one hart, and `with` panics on re-entry, so the `&mut` handed
// to the closure is never aliased.
unsafe impl<T> Sync for SingleHartCell<T> {}

impl<T> SingleHartCell<T> {
    pub const fn new(value: T) -> Self {
        Self { inner: UnsafeCell::new(value), in_use: AtomicBool::new(false) }
    }

    /// Run `f` with exclusive access to the value.
    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_gives_mutable_access() {
        static CELL: SingleHartCell<u32> = SingleHartCell::new(7);
        let seen = CELL.with(|v| {
            *v += 1;
            *v
        });
        assert_eq!(seen, 8);
    }

    #[test]
    #[should_panic(expected = "re-entrant")]
    fn reentrant_access_panics() {
        static CELL: SingleHartCell<u32> = SingleHartCell::new(0);
        CELL.with(|_| CELL.with(|_| {}));
    }
}
```

(`use` lines go at the top of the file, after the module docs and `pub mod frame;`.)

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p kernel-arch-riscv64 mem::tests`
Expected: FAIL — both tests hit `todo!()`.

- [ ] **Step 3: Implement `with`**

```rust
    /// Run `f` with exclusive access to the value.
    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        assert!(
            !self.in_use.swap(true, Ordering::Acquire),
            "re-entrant access to single-hart cell"
        );
        // SAFETY: the in_use flag guarantees no other &mut exists (see
        // the Sync impl note).
        let result = f(unsafe { &mut *self.inner.get() });
        self.in_use.store(false, Ordering::Release);
        result
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p kernel-arch-riscv64`
Expected: PASS.

- [ ] **Step 5: Commit**

```powershell
git add arch/riscv64/src/mem/mod.rs
git commit -m "feat(arch): SingleHartCell with re-entry tripwire for static kernel state"
```

---

### Task 4: Sv39 PTE and address math — pure (TDD)

**Files:**
- Create: `arch/riscv64/src/mem/paging.rs`
- Modify: `arch/riscv64/src/mem/mod.rs` (add `pub mod paging;` after `pub mod frame;`)

- [ ] **Step 1: Write the failing tests**

Create `arch/riscv64/src/mem/paging.rs` with constants, stubbed functions, and tests:

```rust
//! Sv39 paging: three-level page tables mapping 39-bit virtual
//! addresses, 4 KiB leaf pages only (megapages are a deferred
//! optimization — one code path for now).
//!
//! Pure here: PTE encoding and virtual-address index math, host-tested.
//! Gated below (later task): the `PageTable` walker that touches real
//! memory.

/// Size of one page (= one frame): 4 KiB.
pub const PAGE_SIZE: usize = 4096;

/// PTE flag bits (Sv39 leaf and non-leaf entries share the layout).
pub const PTE_V: u64 = 1 << 0; // valid
pub const PTE_R: u64 = 1 << 1; // readable
pub const PTE_W: u64 = 1 << 2; // writable
pub const PTE_X: u64 = 1 << 3; // executable
// 1 << 4 is U (user-accessible) — deliberately absent until Phase 3.
pub const PTE_G: u64 = 1 << 5; // global: valid in every address space
pub const PTE_A: u64 = 1 << 6; // accessed
pub const PTE_D: u64 = 1 << 7; // dirty

/// The 9-bit page-table index for `va` at `level` (2 = root, 0 = leaf).
pub fn vpn(va: usize, level: usize) -> usize {
    todo!()
}

/// Build a PTE pointing at physical address `pa` (4 KiB-aligned) with
/// `flags`; V is always set.
pub fn pte_for(pa: usize, flags: u64) -> u64 {
    todo!()
}

/// The physical address a PTE points at.
pub fn pte_to_pa(pte: u64) -> usize {
    todo!()
}

pub fn pte_is_valid(pte: u64) -> bool {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vpn_extracts_all_three_levels() {
        // 0x8020_0000: bits 38:30 = 2, bits 29:21 = 1, bits 20:12 = 0.
        let va = 0x8020_0000;
        assert_eq!(vpn(va, 2), 2);
        assert_eq!(vpn(va, 1), 1);
        assert_eq!(vpn(va, 0), 0);
    }

    #[test]
    fn vpn_isolates_nine_bits() {
        // All-ones VA: every level reads 0x1ff, nothing bleeds across.
        let va = usize::MAX;
        for level in 0..3 {
            assert_eq!(vpn(va, level), 0x1ff);
        }
    }

    #[test]
    fn pte_round_trips_the_physical_address() {
        let pa = 0x8020_1000;
        let pte = pte_for(pa, PTE_R | PTE_X);
        assert_eq!(pte_to_pa(pte), pa);
    }

    #[test]
    fn pte_for_sets_valid_and_keeps_flags() {
        let pte = pte_for(0x8030_0000, PTE_R | PTE_W);
        assert!(pte_is_valid(pte));
        assert_ne!(pte & PTE_R, 0);
        assert_ne!(pte & PTE_W, 0);
        assert_eq!(pte & PTE_X, 0);
    }

    #[test]
    fn zero_pte_is_invalid() {
        assert!(!pte_is_valid(0));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p kernel-arch-riscv64 paging`
Expected: FAIL — every test hits `todo!()`.

- [ ] **Step 3: Implement the four functions**

```rust
/// The 9-bit page-table index for `va` at `level` (2 = root, 0 = leaf).
pub fn vpn(va: usize, level: usize) -> usize {
    (va >> (12 + 9 * level)) & 0x1ff
}

/// Build a PTE pointing at physical address `pa` (4 KiB-aligned) with
/// `flags`; V is always set. PTE layout: PPN in bits 53:10, flags 9:0.
pub fn pte_for(pa: usize, flags: u64) -> u64 {
    ((pa as u64 >> 12) << 10) | flags | PTE_V
}

/// The physical address a PTE points at.
pub fn pte_to_pa(pte: u64) -> usize {
    ((pte >> 10) << 12) as usize
}

pub fn pte_is_valid(pte: u64) -> bool {
    pte & PTE_V != 0
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p kernel-arch-riscv64`
Expected: PASS.

- [ ] **Step 5: Commit**

```powershell
git add arch/riscv64/src/mem
git commit -m "feat(arch): Sv39 PTE encoding and VPN math with host tests"
```

---

### Task 5: Page-fault causes in the trap decoder (TDD)

**Files:**
- Modify: `arch/riscv64/src/trap.rs`

- [ ] **Step 1: Write the failing tests**

Add to the test module in `arch/riscv64/src/trap.rs`:

```rust
    #[test]
    fn decodes_page_faults() {
        assert_eq!(decode(12), Cause::InstructionPageFault);
        assert_eq!(decode(13), Cause::LoadPageFault);
        assert_eq!(decode(15), Cause::StorePageFault);
    }

    #[test]
    fn page_fault_codes_as_interrupts_stay_unknown() {
        // Interrupt bit + code 13 is NOT a load page fault.
        assert_eq!(
            decode(INTERRUPT_BIT | 13),
            Cause::Unknown { interrupt: true, code: 13 }
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p kernel-arch-riscv64 trap`
Expected: FAIL — `InstructionPageFault` etc. don't exist (compile error counts as the failing state).

- [ ] **Step 3: Add the variants, decode arms, and a `fatal` helper**

Extend the `Cause` enum (after `SupervisorTimer`):

```rust
    /// Instruction fetch from an unmapped/non-executable page (code 12).
    InstructionPageFault,
    /// Load from an unmapped/unreadable page (code 13).
    LoadPageFault,
    /// Store to an unmapped/unwritable page (code 15) — what the W^X
    /// probe deliberately triggers.
    StorePageFault,
```

Extend `decode` (before the `_` arm):

```rust
        (false, 12) => Cause::InstructionPageFault,
        (false, 13) => Cause::LoadPageFault,
        (false, 15) => Cause::StorePageFault,
```

Refactor the dispatcher's fatal path into a reusable helper (place it
next to `trap_handler`, gated like it):

```rust
/// Unrecoverable trap: print everything we know, then panic. `stval`
/// holds the faulting address for page faults.
#[cfg(target_arch = "riscv64")]
fn fatal(kind: &str, frame: &TrapFrame) -> ! {
    crate::println!(
        "FATAL TRAP ({kind}): sepc={:#x} stval={:#x}",
        frame.sepc, frame.stval
    );
    crate::println!("{frame:#x?}");
    panic!("unhandled trap");
}
```

Update `trap_handler`'s match — page faults are fatal for now (the W^X
probe arm comes in Task 9), and the `Unknown` arm delegates to `fatal`:

```rust
        Cause::InstructionPageFault => fatal("instruction page fault", frame),
        Cause::LoadPageFault => fatal("load page fault", frame),
        Cause::StorePageFault => fatal("store page fault", frame),
        Cause::Unknown { interrupt, code } => {
            crate::println!("trap: unknown cause interrupt={interrupt} code={code}");
            fatal("unknown", frame);
        }
```

- [ ] **Step 4: Run tests and the cross-build**

Run: `cargo test -p kernel-arch-riscv64` — expected: PASS.
Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf` — expected: success.

- [ ] **Step 5: Commit**

```powershell
git add arch/riscv64/src/trap.rs
git commit -m "feat(arch): decode page-fault causes; factor fatal-trap diagnostics"
```

---

### Task 6: Linker script — 4 KiB sections, symbols, stack guard

**Files:**
- Modify: `kernel/kernel.ld`

- [ ] **Step 1: Rewrite the SECTIONS block**

Replace the whole `SECTIONS { ... }` block in `kernel/kernel.ld` with
(header comment above it stays, but change its parenthetical from
"no virtual memory yet — that is Phase 2" to "Phase 2b identity-maps
exactly these sections, so their boundaries must be 4 KiB-aligned"):

```ld
SECTIONS
{
    . = BASE_ADDRESS;

    /* Section start/end symbols feed mem::init()'s W^X mapping:
     * .text R-X, .rodata R--, .data+.bss RW-, stack RW-. Each region
     * is 4 KiB-aligned because permissions are per-page. */
    __text_start = .;
    .text : {
        *(.text.boot)            /* _start must be the first code */
        *(.text .text.*)
    }
    . = ALIGN(4K);
    __text_end = .;

    __rodata_start = .;
    .rodata : {
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
    }
    . = ALIGN(4K);
    __rodata_end = .;

    __data_start = .;
    .data : {
        *(.data .data.*)
        *(.sdata .sdata.*)
    }

    /* boot.rs zeroes [__bss_start, __bss_end) in 8-byte steps. */
    .bss : ALIGN(8) {
        __bss_start = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        . = ALIGN(8);
        __bss_end = .;
    }
    . = ALIGN(4K);
    __data_end = .;              /* .data and .bss map as one RW region */

    /* 4 KiB guard page below the stack. mem::init() leaves it unmapped,
     * so a stack overflow store-faults instead of silently corrupting
     * .bss (2a's documented limitation). */
    . += 4K;

    /* Boot stack for the single hart we run. */
    .stack (NOLOAD) : {
        __stack_start = .;
        . += 64K;
        __stack_top = .;
    }
    __kernel_end = .;            /* frame allocator manages from here */

    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr)
    }
}
```

(`__stack_start` is already 4 KiB-aligned: `__data_end` is `ALIGN(4K)`
and the guard adds exactly one page. `__kernel_end` is too: 64 KiB is a
multiple of 4 KiB.)

- [ ] **Step 2: Cross-build and confirm boot is unbroken**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf` — expected: success.
Run: `./tools/test-qemu.ps1`
Expected: still FAIL, but **only** on the three Phase 2b patterns — `hello world`, `trap: breakpoint`, `survived breakpoint`, and `tick: 2` all still observed, proving the relayout didn't break boot.

- [ ] **Step 3: Commit**

```powershell
git add kernel/kernel.ld
git commit -m "feat: 4K-aligned kernel sections with W^X symbols and stack guard page"
```

---

### Task 7: `satp` accessor

**Files:**
- Modify: `arch/riscv64/src/csr.rs`

- [ ] **Step 1: Add the Sv39 mode constant and `satp_write`**

Append to `arch/riscv64/src/csr.rs`:

```rust
/// `satp` mode field (bits 63:60) selecting Sv39 translation.
pub const SATP_MODE_SV39: usize = 8 << 60;

/// Point address translation at a root page table and switch it on:
/// `value` = mode bits | root-table PPN (physical address >> 12).
/// Fenced on both sides so no stale translations straddle the switch.
///
/// # Safety
/// The table must identity-map every address the kernel touches —
/// the code executing this function, its stack, and all statics —
/// with correct permissions. Anything less turns the instruction
/// after `csrw` into a page fault (or a silent wild fetch).
#[inline]
pub unsafe fn satp_write(value: usize) {
    unsafe {
        asm!(
            "sfence.vma",
            "csrw satp, {}",
            "sfence.vma",
            in(reg) value,
            options(nostack),
        )
    };
}
```

(No `nomem` here, unlike the other accessors: this changes how all
memory is addressed, and the compiler must not cache memory across it.)

- [ ] **Step 2: Cross-build**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: success (`SATP_MODE_SV39` and `satp_write` are dead code until Task 8 — that's fine for one commit; if the build warns about unused items, allow it, the next task consumes them).

- [ ] **Step 3: Commit**

```powershell
git add arch/riscv64/src/csr.rs
git commit -m "feat(arch): satp accessor with Sv39 mode and translation fences"
```

---

### Task 8: Gated allocator statics, page-table walker, and `mem::init()`

**Files:**
- Modify: `arch/riscv64/src/mem/frame.rs`
- Modify: `arch/riscv64/src/mem/paging.rs`
- Modify: `arch/riscv64/src/mem/mod.rs`

- [ ] **Step 1: Add the global allocator instance and zeroing wrappers**

Append to `arch/riscv64/src/mem/frame.rs`:

```rust
/// The kernel's one allocator instance; `mem::init()` arms it over
/// the free RAM after the kernel image.
#[cfg(target_arch = "riscv64")]
pub(crate) static ALLOCATOR: super::SingleHartCell<BitmapAllocator> =
    super::SingleHartCell::new(BitmapAllocator::new());

/// Allocate a frame, zeroed. Zeroing on alloc (not free) means a
/// recycled frame never leaks the previous user's bytes — cheap
/// hygiene the security spine (Phase 3) will rely on.
#[cfg(target_arch = "riscv64")]
pub fn alloc_zeroed() -> Option<PhysFrame> {
    let frame = ALLOCATOR.with(|a| a.alloc())?;
    // SAFETY: the frame is managed RAM we exclusively own, identity-
    // mapped RW (or the MMU is still off during mem::init), so writing
    // FRAME_SIZE bytes at its base is in-bounds.
    unsafe { core::ptr::write_bytes(frame.0 as *mut u8, 0, FRAME_SIZE) };
    Some(frame)
}

/// Return a frame to the allocator. Panics on double-free or an
/// unmanaged address (see [`BitmapAllocator::free`]).
#[cfg(target_arch = "riscv64")]
pub fn free(frame: PhysFrame) {
    ALLOCATOR.with(|a| a.free(frame));
}
```

- [ ] **Step 2: Add the page-table type and walker**

Append to `arch/riscv64/src/mem/paging.rs`:

```rust
/// One Sv39 page table: 512 PTEs, exactly one 4 KiB frame.
#[cfg(target_arch = "riscv64")]
#[repr(C, align(4096))]
pub struct PageTable {
    pub entries: [u64; 512],
}

/// Map the 4 KiB page at virtual `va` to physical `pa` in the tree
/// rooted at `root`, creating intermediate tables from the frame
/// allocator as needed. Panics if `va` is already mapped: in 2b every
/// mapping is made exactly once, so a remap is a bug.
///
/// Leaf PTEs get `A` always and `D` when writable, set up front: the
/// spec lets an MMU fault instead of setting them in hardware, and we
/// have no swapping that would want the information.
///
/// # Safety
/// `root` must point at a valid (zero-initialized or in-construction)
/// page table, and `pa` at memory the caller may map. Called with the
/// MMU off (mem::init) or on identity-mapped table frames.
#[cfg(target_arch = "riscv64")]
pub unsafe fn map_page(root: *mut PageTable, va: usize, pa: usize, flags: u64) {
    debug_assert!(va % PAGE_SIZE == 0 && pa % PAGE_SIZE == 0);
    let mut table = root;
    for level in [2, 1] {
        let idx = vpn(va, level);
        // SAFETY: `table` is a valid table per the contract; idx < 512.
        let pte = unsafe { (*table).entries[idx] };
        table = if pte_is_valid(pte) {
            pte_to_pa(pte) as *mut PageTable
        } else {
            let frame = super::frame::alloc_zeroed().expect("out of frames for page table");
            // Non-leaf PTE: V only (R/W/X all zero means "next level").
            // SAFETY: same as the read above.
            unsafe { (*table).entries[idx] = pte_for(frame.0, 0) };
            frame.0 as *mut PageTable
        };
    }
    let idx = vpn(va, 0);
    // SAFETY: `table` now points at the leaf-level table.
    unsafe {
        assert!(!pte_is_valid((*table).entries[idx]), "remap of va {va:#x}");
        let dirty = if flags & PTE_W != 0 { PTE_D } else { 0 };
        (*table).entries[idx] = pte_for(pa, flags | PTE_A | dirty);
    }
}

/// Identity-map `[start, end)` (4 KiB-aligned) with `flags`.
///
/// # Safety
/// Same contract as [`map_page`].
#[cfg(target_arch = "riscv64")]
pub unsafe fn map_range(root: *mut PageTable, start: usize, end: usize, flags: u64) {
    let mut addr = start;
    while addr < end {
        // SAFETY: forwarded from the caller.
        unsafe { map_page(root, addr, addr, flags) };
        addr += PAGE_SIZE;
    }
}
```

- [ ] **Step 3: Add `mem::init()` and the stats accessors**

Append to `arch/riscv64/src/mem/mod.rs`:

```rust
/// End of RAM on QEMU virt with `-m 128M` (pinned by the run/test
/// scripts). **QEMU-specific constant** like timer.rs's TIMEBASE_HZ —
/// real hardware (Phase 4) must read the memory map from the device
/// tree instead.
#[cfg(target_arch = "riscv64")]
const RAM_END: usize = 0x8800_0000;

#[cfg(target_arch = "riscv64")]
extern "C" {
    static __text_start: u8;
    static __text_end: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __data_start: u8;
    static __data_end: u8;
    static __stack_start: u8;
    static __stack_top: u8;
    static __kernel_end: u8;
}

/// The address of a linker-script symbol. Only the address is
/// meaningful; the "value" must never be read.
#[cfg(target_arch = "riscv64")]
macro_rules! sym {
    ($name:ident) => {
        // SAFETY: the symbol is defined by kernel.ld; addr_of! takes
        // its address without creating a reference to its (meaningless)
        // contents.
        unsafe { core::ptr::addr_of!($name) as usize }
    };
}

/// Bring memory management up: arm the frame allocator over free RAM,
/// identity-map the kernel with W^X section permissions, enable Sv39.
///
/// Call exactly once, after `trap::init()` (a paging mistake should
/// fault loudly, not hang) and before `timer::start()` (no interrupts
/// while the world is being remapped).
#[cfg(target_arch = "riscv64")]
pub fn init() {
    use paging::{PTE_G, PTE_R, PTE_W, PTE_X};

    let free_ram = (sym!(__kernel_end), RAM_END);
    frame::ALLOCATOR.with(|a| a.init(free_ram.0, free_ram.1));

    let root = frame::alloc_zeroed().expect("no frame for root page table").0
        as *mut paging::PageTable;
    // SAFETY: the ranges are the 4 KiB-aligned kernel.ld sections of our
    // own image plus the RAM the allocator was just armed with; the MMU
    // is still off, so all writes land in physical memory we own.
    unsafe {
        paging::map_range(root, sym!(__text_start), sym!(__text_end), PTE_R | PTE_X | PTE_G);
        paging::map_range(root, sym!(__rodata_start), sym!(__rodata_end), PTE_R | PTE_G);
        paging::map_range(root, sym!(__data_start), sym!(__data_end), PTE_R | PTE_W | PTE_G);
        // The guard page between __data_end and __stack_start stays
        // unmapped: stack overflow faults instead of corrupting .bss.
        paging::map_range(root, sym!(__stack_start), sym!(__stack_top), PTE_R | PTE_W | PTE_G);
        // Free RAM mapped eagerly so allocated frames are immediately
        // usable — no fault-and-map machinery in 2b.
        paging::map_range(root, free_ram.0, free_ram.1, PTE_R | PTE_W | PTE_G);
        // SAFETY: everything the kernel touches is now identity-mapped.
        crate::csr::satp_write(crate::csr::SATP_MODE_SV39 | (root as usize >> 12));
    }
}

/// Frames currently free (for boot diagnostics).
#[cfg(target_arch = "riscv64")]
pub fn free_frames() -> usize {
    frame::ALLOCATOR.with(|a| a.free_frames())
}

/// Frames managed in total (for boot diagnostics).
#[cfg(target_arch = "riscv64")]
pub fn total_frames() -> usize {
    frame::ALLOCATOR.with(|a| a.total_frames())
}
```

- [ ] **Step 4: Host tests and cross-build**

Run: `cargo test -p kernel-arch-riscv64` — expected: PASS (gated code invisible to the host).
Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf` — expected: success (possibly an unused warning on `mem::init` until Task 10 wires it; acceptable between commits).

- [ ] **Step 5: Commit**

```powershell
git add arch/riscv64/src/mem
git commit -m "feat(arch): page-table walker and mem::init - W^X identity map, Sv39 enable"
```

---

### Task 9: W^X probe support in the trap dispatcher

**Files:**
- Modify: `arch/riscv64/src/trap.rs`

- [ ] **Step 1: Add the probe flag and public API**

Add near the top of the gated section of `arch/riscv64/src/trap.rs`
(e.g. right before `pub fn init()`):

```rust
#[cfg(target_arch = "riscv64")]
use core::sync::atomic::{AtomicBool, Ordering};

/// One-shot flag for the deliberate W^X probe (kmain's write to
/// .rodata — 2b's analogue of 2a's ebreak). Set by [`expect_wx_fault`],
/// consumed by the dispatcher when the expected store page fault
/// arrives.
#[cfg(target_arch = "riscv64")]
static EXPECTING_WX_FAULT: AtomicBool = AtomicBool::new(false);

/// Arm the W^X probe: the next store page fault is expected and will be
/// reported and skipped instead of panicking.
#[cfg(target_arch = "riscv64")]
pub fn expect_wx_fault() {
    EXPECTING_WX_FAULT.store(true, Ordering::Release);
}

/// True while an armed probe has not yet faulted. kmain asserts this is
/// false after the probe write — if the MMU let the write through, W^X
/// is broken and the boot must fail loudly.
#[cfg(target_arch = "riscv64")]
pub fn wx_fault_pending() -> bool {
    EXPECTING_WX_FAULT.load(Ordering::Acquire)
}
```

- [ ] **Step 2: Teach the dispatcher to recover the probe**

Replace the `Cause::StorePageFault` arm in `trap_handler`:

```rust
        Cause::StorePageFault => {
            if EXPECTING_WX_FAULT.swap(false, Ordering::AcqRel) {
                crate::println!("trap: W^X store fault at {:#x} (probe)", frame.stval);
                // Like the breakpoint: skip the faulting store so
                // execution resumes after the probe.
                frame.sepc += instruction_len_at(frame.sepc);
            } else {
                fatal("store page fault", frame);
            }
        }
```

- [ ] **Step 3: Update the stale SAFETY comment in `instruction_len_at`**

Replace the comment inside `instruction_len_at`:

```rust
    // SAFETY: addr is the sepc of a just-executed instruction, so it
    // points into .text — identity-mapped R-X once paging is on (and
    // physically addressed before), so the read is always legal.
```

- [ ] **Step 4: Host tests and cross-build**

Run: `cargo test -p kernel-arch-riscv64` — expected: PASS.
Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf` — expected: success.

- [ ] **Step 5: Commit**

```powershell
git add arch/riscv64/src/trap.rs
git commit -m "feat(arch): W^X probe recovery in the trap dispatcher"
```

---

### Task 10: Wire kmain — smoke test goes green

**Files:**
- Modify: `kernel/src/main.rs`

- [ ] **Step 1: Update kmain and add the two probe functions**

In `kernel/src/main.rs`, update the import and `kmain`, and add
`wx_probe` and `frame_roundtrip` inside `mod bare`:

```rust
    use kernel::GREETING;
    use kernel_arch_riscv64::{mem, println, timer, trap};
    use kernel_common::PROJECT_NAME;

    /// Rust entry, called from the boot assembly with the arguments
    /// OpenSBI gave us. Never returns: a kernel has nowhere to return to.
    #[no_mangle]
    extern "C" fn kmain(hartid: usize, _dtb: usize) -> ! {
        println!();
        println!("{GREETING} from {PROJECT_NAME} - Phase 2b (hart {hartid})");

        trap::init();
        // Deliberate breakpoint: proves the handler catches an exception
        // and execution RESUMES past it (the smoke test's
        // "survived breakpoint" line can only print if recovery worked).
        unsafe { core::arch::asm!("ebreak") };
        println!("survived breakpoint");

        mem::init();
        println!(
            "paging: sv39 on ({} of {} frames free)",
            mem::free_frames(),
            mem::total_frames()
        );
        wx_probe();
        frame_roundtrip();

        timer::start();
        println!("(kernel idles; heartbeat ~1/s; exit QEMU with Ctrl-A then X)");
        park()
    }

    /// 2b's deliberate fault (like 2a's ebreak): prove the MMU blocks
    /// writes to read-only memory. The store is inline asm so Rust never
    /// sees a write through a shared reference — that would be UB even
    /// though the store faults before retiring.
    fn wx_probe() {
        static RODATA_PROBE: u64 = 0x600D_C0DE;
        trap::expect_wx_fault();
        // SAFETY: the store targets .rodata, mapped R-- — it faults, the
        // trap handler consumes the probe flag and skips the instruction.
        unsafe {
            core::arch::asm!(
                "sd zero, 0({addr})",
                addr = in(reg) core::ptr::addr_of!(RODATA_PROBE) as usize,
                options(nostack),
            );
        }
        assert!(
            !trap::wx_fault_pending(),
            "W^X broken: rodata write did not fault"
        );
        // SAFETY: reading our own static; volatile so the check can't be
        // const-folded away.
        let value = unsafe { core::ptr::read_volatile(&RODATA_PROBE) };
        assert_eq!(value, 0x600D_C0DE, "W^X broken: rodata was modified");
        println!("wx: rodata write blocked");
    }

    /// Prove the allocator round-trips: alloc -> write -> free ->
    /// re-alloc returns the same (re-zeroed) frame.
    fn frame_roundtrip() {
        let first = mem::frame::alloc_zeroed().expect("frame alloc failed");
        let p = first.0 as *mut u64;
        // SAFETY: `first` is a 4 KiB frame we own, identity-mapped RW.
        unsafe {
            assert_eq!(p.read_volatile(), 0, "frame not zeroed on alloc");
            p.write_volatile(0x600D_F00D);
            assert_eq!(p.read_volatile(), 0x600D_F00D, "frame not writable");
        }
        mem::frame::free(first);
        let second = mem::frame::alloc_zeroed().expect("frame re-alloc failed");
        assert_eq!(first, second, "first-fit should recycle the freed frame");
        // SAFETY: same frame, still mapped RW.
        unsafe {
            assert_eq!(p.read_volatile(), 0, "recycled frame not re-zeroed");
        }
        mem::frame::free(second);
        println!("frames: alloc/free ok");
    }
```

Also update `main.rs`'s module doc header line from `Phase 2a` context if
present (the file header mentions phases generically — only change text
that names 2a).

- [ ] **Step 2: Run the full smoke test**

Run: `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS: 2a milestones plus paging, W^X, and frame round-trip all observed.` — exit code 0. If it fails, the serial log is printed; debug with the systematic-debugging skill before touching code.

- [ ] **Step 3: Run the host tests**

Run: `cargo test`
Expected: PASS (whole workspace).

- [ ] **Step 4: Commit**

```powershell
git add kernel/src/main.rs
git commit -m "feat: Phase 2b live - Sv39 paging, W^X probe, frame round-trip in kmain"
```

---

### Task 11: Documentation

**Files:**
- Create: `docs/learning/0004-memory-and-paging.md`
- Modify: `docs/learning/README.md`
- Modify: `docs/glossary.md`
- Modify: `docs/roadmap/roadmap.md`

- [ ] **Step 1: Write the learning note**

Create `docs/learning/0004-memory-and-paging.md`. The executor should
write this honestly from what the implementation actually taught —
the draft below is the skeleton to adapt (it is real content, not a
placeholder; extend it with anything that genuinely surprised during
implementation):

```markdown
# 0004 — Memory and paging (Phase 2b)

How the kernel took ownership of its own memory: handing out physical
frames, building Sv39 page tables, and turning on the MMU without
sawing off the branch it sits on.

## Physical vs. virtual: two address spaces, one RAM

Until now every address was *physical* — `0x8020_0000` named a real DRAM
location. Paging inserts a translation layer: code uses *virtual*
addresses, and the MMU walks a page table to find the physical frame
behind each 4 KiB page. We chose an **identity mapping** (VA = PA) for
the kernel, which sounds like it changes nothing — but the point isn't
*where* things live, it's the **permission bits** that come with each
mapping. Identity-mapped W^X is real protection.

## The frame allocator: a bitmap, on purpose

One bit per 4 KiB frame; 128 MiB needs a 4 KiB bitmap. The xv6-style
free-list is the classic choice (each free frame stores a pointer to the
next), but its data structure lives *inside* freed memory — impossible
to unit-test on the host and silently corrupted by a double-free. The
bitmap's core is pure integer math: it tests on the host like the trap
decoder, and a double-free hits a loud assert instead of corrupting the
list. The O(n) scan is irrelevant at 32k frames.

## Sv39 in one paragraph

A virtual address's bits 38:12 split into three 9-bit indices (VPN2/1/0),
one per table level; each table is itself exactly one 4 KiB frame holding
512 PTEs. A PTE holds the next level's (or the final page's) physical
frame number plus flag bits — Valid, Read, Write, eXecute, and friends.
`satp` holds the root table's frame number and the mode (8 = Sv39);
`sfence.vma` flushes the TLB, the cache of recent translations.

## Turning it on without crashing

The scary moment: `csrw satp` changes how the *next instruction fetch*
is addressed. Identity mapping is what makes it safe — the PC is valid
in both worlds. The order of operations matters everywhere here: trap
handler first (a paging bug should fault loudly, not hang), then build
the complete table, then fence-write-fence. Two other traps avoided:
the A/D bits are pre-set because hardware is allowed to fault instead of
setting them; and OpenSBI's region stays *unmapped* — the kernel can't
even read firmware memory now, and SBI calls still work because `ecall`
jumps to M-mode where `satp` doesn't apply.

## W^X, proven, not assumed

`.text` is executable but not writable; `.rodata` is only readable; data
is writable but not executable. The boot now *proves* it: kmain arms a
one-shot flag and deliberately stores to `.rodata`. The MMU raises a
store page fault, the handler recognizes the armed probe, skips the
store (reusing 2a's instruction-length logic), and boot continues. If
the store *succeeds*, kmain panics — broken W^X fails the smoke test.
A subtlety: the store is inline asm, because a Rust-level write through
a shared reference is UB even if it never retires.

## The guard page

Paging also paid off a 2a debt: the boot stack now has an unmapped page
below it, so an overflow store-faults instead of silently corrupting
.bss. Honest caveat: the fault itself pushes a trap frame onto the
overflowed stack, which faults again — the recursion marches `sp`
through the guard and corrupts a few hundred bytes below it before the
panic lands. Still a loud, attributable crash instead of a silent one;
the real fix is 2c's dedicated trap stack.
```

- [ ] **Step 2: Index the note**

In `docs/learning/README.md`, append to the Notes list:

```markdown
- [0004 — Memory and paging (Phase 2b)](0004-memory-and-paging.md)
```

- [ ] **Step 3: Add glossary entries**

Append to `docs/glossary.md` (after the 2a entries, keeping the
one-bullet-per-term format):

```markdown
- **Physical address** — a real location in RAM chips. Before Phase 2b, the only kind of address the kernel had.
- **Virtual address** — the address code actually uses once the MMU is on; translated to a physical address through page tables on every access.
- **Page / frame** — the same 4 KiB unit seen from two sides: a *page* is virtual, a *frame* is the physical RAM behind it.
- **MMU (Memory Management Unit)** — CPU hardware that translates virtual to physical addresses and enforces per-page permissions, faulting on violations.
- **Page table** — the tree the MMU walks to translate addresses. In Sv39 it has three levels; each table is one 4 KiB frame of 512 entries.
- **PTE (Page Table Entry)** — one slot in a page table: the next level's (or final frame's) number plus permission flags (Valid/Read/Write/eXecute…).
- **Sv39** — RISC-V's three-level paging scheme: 39-bit virtual addresses, 4 KiB pages. What Phase 2b enables.
- **`satp`** — the CSR that holds the root page table's frame number and translation mode; writing it turns paging on.
- **TLB (Translation Lookaside Buffer)** — the MMU's cache of recent translations; must be flushed (`sfence.vma`) when mappings change.
- **`sfence.vma`** — the RISC-V instruction that flushes the TLB so stale translations can't survive a page-table change.
- **Identity mapping** — mapping each virtual address to the identical physical address. Changes nothing about *where* things are — and everything about *what's allowed*, via permission bits.
- **W^X (write XOR execute)** — the policy that no memory is both writable and executable: code can't be overwritten, data can't be run. Phase 2b's payoff.
- **Guard page** — a deliberately unmapped page (here: below the stack) so an overflow faults immediately instead of corrupting the neighbor.
- **Frame allocator** — the kernel component that owns physical RAM and hands out/reclaims 4 KiB frames; ours tracks them in a bitmap.
```

- [ ] **Step 4: Mark 2b done in the roadmap**

In `docs/roadmap/roadmap.md`, change the 2b heading (use the actual
completion date):

```markdown
### Phase 2b — Memory management  *(done — 2026-06-10)*
```

- [ ] **Step 5: Verify references and commit**

Run: `./tools/check-references.ps1` — expected: `Reference check OK`.
Run: `cargo test` — expected: PASS.

```powershell
git add docs/learning docs/glossary.md docs/roadmap/roadmap.md
git commit -m "docs: memory/paging learning note, glossary terms, roadmap 2b done"
```

---

### Task 12: Final verification

- [ ] **Step 1: Full host test suite**

Run: `cargo test`
Expected: PASS, zero failures.

- [ ] **Step 2: Cross-build clean**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: success, no warnings about unused mem items (everything is wired by now).

- [ ] **Step 3: Smoke test**

Run: `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS` — all seven patterns in one boot.

- [ ] **Step 4: Reference check**

Run: `./tools/check-references.ps1`
Expected: `Reference check OK`.

No commit — this task only verifies. If anything fails, fix it with the
systematic-debugging skill before declaring the phase done.
