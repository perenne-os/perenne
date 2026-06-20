# Phase 4a: Device-tree-driven hardware discovery — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline, per the user's preference for this project) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Parse the device tree (the `dtb` pointer OpenSBI passes to `kmain`) to discover RAM base/size and the timer frequency at boot, replacing the hardcoded QEMU `RAM_END` and `TIMEBASE_HZ`. QEMU-only; proven dynamic by booting with `-m 192M`.

**Architecture:** A small hand-rolled `no_std` FDT parser in `arch/riscv64/src/dt.rs` (`parse(&[u8]) -> Option<MachineInfo>`, host-tested against a committed real QEMU DTB; gated `from_ptr`). `mem::init(ram_end)` and `timer::init(timebase_hz)` replace the constants; `kmain` parses the DTB early and wires the values in.

**Tech Stack:** Rust `no_std`, RISC-V (riscv64gc), QEMU. Host tests: `cargo test -p kernel-arch-riscv64`. Bare build: `cargo build -p kernel --target riscv64gc-unknown-none-elf`.

**Spec:** `docs/superpowers/specs/2026-06-20-phase-4a-device-tree-discovery-design.md`

**Verified during planning:** the parser below was run against the real QEMU virt DTB and returns `ram_base = 0x8000_0000`, `ram_size = 128 MiB`, `timebase_hz = 10_000_000`; bad magic → `None`. The fixture `arch/riscv64/tests/fixtures/qemu-virt.dtb` is already captured (via `qemu-system-riscv64 -machine virt,dumpdtb=… -m 128M -display none`) and is currently untracked.

---

## Task 1: the FDT parser (`dt.rs`, host-tested) + fixture

**Files:**
- Create: `arch/riscv64/src/dt.rs`
- Modify: `arch/riscv64/src/lib.rs` (declare the module)
- Add: `arch/riscv64/tests/fixtures/qemu-virt.dtb` (already captured)

- [ ] **Step 1: Confirm the fixture exists (or re-capture it)**

Run: `ls arch/riscv64/tests/fixtures/qemu-virt.dtb`
Expected: the file exists (~5 KB). If missing, recreate it:
`qemu-system-riscv64 -machine virt,dumpdtb=arch/riscv64/tests/fixtures/qemu-virt.dtb -m 128M -display none`

- [ ] **Step 2: Create `arch/riscv64/src/dt.rs`**

```rust
//! Device tree (flattened, FDT) parsing.
//!
//! OpenSBI passes the address of the firmware's flattened device tree in
//! `a1` (the `dtb` argument to `kmain`). We read just what the kernel needs
//! to stop hardcoding QEMU's machine: RAM base/size (the `/memory` node's
//! `reg`) and the timer frequency (`timebase-frequency`). Pure parsing is
//! host-tested; `from_ptr` (gated) wraps a raw firmware pointer.

/// What the kernel learns from the device tree at boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MachineInfo {
    pub ram_base: usize,
    pub ram_size: usize,
    pub timebase_hz: u64,
}

const FDT_MAGIC: u32 = 0xd00d_feed;
const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_NOP: u32 = 4;
const FDT_END: u32 = 9;

/// Big-endian u32 at byte offset `off`, bounds-checked.
fn be_u32(buf: &[u8], off: usize) -> Option<u32> {
    let b = buf.get(off..off + 4)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// Length (excluding NUL) of the null-terminated string at `off`.
fn cstr_len(buf: &[u8], off: usize) -> Option<usize> {
    let mut i = off;
    while *buf.get(i)? != 0 {
        i += 1;
    }
    Some(i - off)
}

/// Parse an FDT blob for [`MachineInfo`]. Returns `None` on a bad magic, a
/// truncated/oversized field (every offset is bounds-checked, so a
/// malformed blob never reads out of bounds), or a missing value. The
/// `/memory` `reg` is decoded using the root's `#address-cells`/
/// `#size-cells` (default 2/2 per the FDT spec).
pub fn parse(dtb: &[u8]) -> Option<MachineInfo> {
    if be_u32(dtb, 0)? != FDT_MAGIC {
        return None;
    }
    let off_struct = be_u32(dtb, 8)? as usize;
    let off_strings = be_u32(dtb, 12)? as usize;

    let mut pos = off_struct;
    let mut depth: usize = 0;
    let mut is_mem = [false; 32]; // is_mem[d] = node at depth d is "memory*"
    let mut addr_cells: u32 = 2;
    let mut size_cells: u32 = 2;
    let mut ram: Option<(usize, usize)> = None;
    let mut timebase: Option<u64> = None;

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
                pos = (pos + name_len + 1 + 3) & !3; // past name + NUL, 4-pad
            }
            FDT_END_NODE => {
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
                if depth < is_mem.len() && is_mem[depth] && pname == b"reg" {
                    let (ac, sc) = (addr_cells as usize, size_cells as usize);
                    if len >= (ac + sc) * 4 {
                        let mut base: u64 = 0;
                        for i in 0..ac {
                            base = (base << 32) | be_u32(val, i * 4)? as u64;
                        }
                        let mut sz: u64 = 0;
                        for i in 0..sc {
                            sz = (sz << 32) | be_u32(val, (ac + i) * 4)? as u64;
                        }
                        ram = Some((base as usize, sz as usize));
                    }
                }
                pos = (val_off + len + 3) & !3; // past value, 4-pad
            }
            FDT_NOP => {}
            FDT_END => break,
            _ => return None,
        }
    }

    Some(MachineInfo {
        ram_base: ram?.0,
        ram_size: ram?.1,
        timebase_hz: timebase?,
    })
}

/// Parse the device tree at physical pointer `ptr` (the firmware `dtb`
/// argument). Reads the header's `totalsize` to bound the blob. Panics if
/// the device tree is invalid or missing the values we need — QEMU always
/// supplies a valid one, so this is a loud safety net.
///
/// # Safety
/// `ptr` must address a valid FDT blob; called once in early boot with the
/// MMU off, before the frame allocator touches its memory.
#[cfg(target_arch = "riscv64")]
pub unsafe fn from_ptr(ptr: usize) -> MachineInfo {
    // Header prefix: magic @0, totalsize @4 (both big-endian u32).
    let header = unsafe { core::slice::from_raw_parts(ptr as *const u8, 8) };
    assert_eq!(be_u32(header, 0), Some(FDT_MAGIC), "dtb: bad magic");
    let totalsize = be_u32(header, 4).expect("dtb: short header") as usize;
    let blob = unsafe { core::slice::from_raw_parts(ptr as *const u8, totalsize) };
    parse(blob).expect("device tree invalid or missing memory/timebase")
}

#[cfg(test)]
mod tests {
    use super::*;
    const DTB: &[u8] = include_bytes!("../tests/fixtures/qemu-virt.dtb");

    #[test]
    fn parses_qemu_virt() {
        let mi = parse(DTB).expect("should parse");
        assert_eq!(mi.ram_base, 0x8000_0000, "ram base");
        assert_eq!(mi.ram_size, 128 * 1024 * 1024, "ram size 128 MiB");
        assert_eq!(mi.timebase_hz, 10_000_000, "timebase 10 MHz");
    }

    #[test]
    fn rejects_bad_magic() {
        assert_eq!(parse(&[0u8; 16]), None);
    }

    #[test]
    fn rejects_truncated_blob() {
        // A valid header but the struct block cut short -> None, not a panic.
        assert_eq!(parse(&DTB[..64]), None);
    }
}
```

- [ ] **Step 3: Declare the module in `lib.rs`**

In `arch/riscv64/src/lib.rs`, add after the `cap` module declaration:

```rust
/// Device tree (FDT) parsing: discover RAM and the timer frequency from the
/// firmware-provided blob (pure parsing host-tested; `from_ptr` gated).
pub mod dt;
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p kernel-arch-riscv64 dt::`
Expected: PASS — `parses_qemu_virt`, `rejects_bad_magic`, `rejects_truncated_blob`. Then `cargo test -p kernel-arch-riscv64` → all green, and `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf` → SUCCESS.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/dt.rs arch/riscv64/src/lib.rs arch/riscv64/tests/fixtures/qemu-virt.dtb
git commit -m "feat(dt): hand-rolled FDT parser for RAM + timebase (host-tested)"
```

---

## Task 2: parameterize `mem::init` and `timer` on the discovered values

**Files:**
- Modify: `arch/riscv64/src/mem/mod.rs`
- Modify: `arch/riscv64/src/timer.rs`

- [ ] **Step 1: `mem::init` takes `ram_end`**

In `arch/riscv64/src/mem/mod.rs`, remove the `RAM_END` constant:

```rust
/// End of RAM on QEMU virt with `-m 128M` (pinned by the run/test
/// scripts). **QEMU-specific constant** ...
#[cfg(target_arch = "riscv64")]
const RAM_END: usize = 0x8800_0000;
```

Change `init` to take `ram_end` and use it for the free-RAM region (the doc comment's first line should note the source is now the device tree):

```rust
#[cfg(target_arch = "riscv64")]
pub fn init(ram_end: usize) {
    use paging::{PTE_G, PTE_R, PTE_W};

    // SAFETY: all sym! calls read linker-script symbol addresses ... The
    // MMU is still off, so writes land in the physical addresses we own.
    unsafe {
        let free_ram = (sym!(__kernel_end), ram_end);
        frame::ALLOCATOR.with(|a| a.init(free_ram.0, free_ram.1));
        // ... rest of init unchanged ...
```

(Only the signature and the `free_ram` end change; the `KERNEL_SATP` save, `map_kernel_sections`, free-RAM map, and `satp_write` are unchanged.)

- [ ] **Step 2: `timer::init` sets a dynamic tick interval**

In `arch/riscv64/src/timer.rs`, remove the two constants:

```rust
const TIMEBASE_HZ: u64 = 10_000_000;
const TICK_INTERVAL: u64 = TIMEBASE_HZ;
```

Add a stored interval and an `init`, and read it in `arm_next`:

```rust
/// Ticks between heartbeats (= the timebase frequency = 1 second), learned
/// from the device tree by [`init`]. Zero until `init` runs — call `init`
/// before `start`.
static TICK_INTERVAL: AtomicU64 = AtomicU64::new(0);

/// Record the platform timer frequency (from the device tree) as the
/// one-second tick interval. Call once, before [`start`].
pub fn init(timebase_hz: u64) {
    TICK_INTERVAL.store(timebase_hz, Ordering::Relaxed);
}
```

Change `arm_next` to read the static:

```rust
fn arm_next() {
    // ... existing comment ...
    let interval = TICK_INTERVAL.load(Ordering::Relaxed);
    sbi::set_timer(csr::time() + interval);
}
```

(`TICKS`, `start`, and `on_tick` are otherwise unchanged. `AtomicU64`/`Ordering` are already imported.)

- [ ] **Step 3: Verify the arch crate builds + host tests**

Run: `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf`
Expected: SUCCESS. (The kernel *binary* will not build yet — `kmain` still calls `mem::init()` with no arg and never calls `timer::init`; fixed in Task 4. Build only the arch crate here.)

Run: `cargo test -p kernel-arch-riscv64`
Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add arch/riscv64/src/mem/mod.rs arch/riscv64/src/timer.rs
git commit -m "refactor(mem,timer): take ram_end / timebase from the caller (drop QEMU constants)"
```

---

## Task 3: update the smoke test for dynamic discovery (red)

**Files:**
- Modify: `tools/test-qemu.ps1`

- [ ] **Step 1: Boot with a non-default RAM size and assert the discovery**

In `tools/test-qemu.ps1`, change the QEMU memory argument from `"-m", "128M"` to:

```powershell
    "-m", "192M",
```

Add a pattern to the `$mustMatch` array (just before `"tick: 2(?!\d)"`):

```powershell
    "dt: 192 MiB RAM",
```

Update the header comment and PASS message to mention the 4a milestone, e.g. add: "and the Phase 4a milestone — the kernel discovers RAM (192 MiB, proving it read the device tree rather than a hardcoded 128) and the timer frequency from the device tree."

- [ ] **Step 2: Run the smoke test — expect RED**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: FAIL — `kmain` doesn't parse the DTB yet, so `dt: 192 MiB RAM` is missing (and, if it boots, the kernel still hardcodes 128 MiB which would mis-map under `-m 192M` — both reasons it must be wired up in Task 4).

- [ ] **Step 3: Commit (red test pinned)**

```bash
git add tools/test-qemu.ps1
git commit -m "test: smoke test proves dynamic RAM discovery via -m 192M (red)"
```

---

## Task 4: wire discovery into `kmain` (green)

**Files:**
- Modify: `kernel/src/main.rs`

- [ ] **Step 1: Use the `dtb` argument; parse and wire it in**

In `kernel/src/main.rs`, in `kmain` (the `bare` module), rename the unused
`_dtb` parameter to `dtb` and restructure the early boot. The current shape
is roughly:

```rust
    extern "C" fn kmain(hartid: usize, _dtb: usize) -> ! {
        println!();
        println!("{GREETING} from {PROJECT_NAME} - Phase 3b-iii (hart {hartid})");

        trap::init();
        unsafe { core::arch::asm!("ebreak") };
        println!("survived breakpoint");

        mem::init();
        println!("paging: sv39 on ({} of {} frames free)", mem::free_frames(), mem::total_frames());
        wx_probe();
        frame_roundtrip();
        pqc_demo();
        // ... spawn block ... timer::start(); sched::enter()
```

Change it to:

```rust
    extern "C" fn kmain(hartid: usize, dtb: usize) -> ! {
        println!();
        println!("{GREETING} from {PROJECT_NAME} - Phase 4a (hart {hartid})");

        trap::init();
        unsafe { core::arch::asm!("ebreak") };
        println!("survived breakpoint");

        // Phase 4a: learn the machine from the device tree instead of
        // hardcoding QEMU's. SAFETY: `dtb` is the firmware-provided FDT
        // pointer; the MMU is still off, so the physical read is valid.
        let machine = unsafe { dt::from_ptr(dtb) };
        println!(
            "dt: {} MiB RAM @ {:#x}, timebase {} Hz",
            machine.ram_size >> 20,
            machine.ram_base,
            machine.timebase_hz
        );

        mem::init(machine.ram_base + machine.ram_size);
        println!("paging: sv39 on ({} of {} frames free)", mem::free_frames(), mem::total_frames());
        wx_probe();
        frame_roundtrip();
        pqc_demo();
        // ... spawn block unchanged ...
        timer::init(machine.timebase_hz);
        timer::start();
        println!("(scheduler starting; heartbeat ~1/s; exit QEMU with Ctrl-A then X)");
        sched::enter()
```

Add `dt` to the `use` line:

```rust
    use kernel_arch_riscv64::{cap::Capability, dt, mem, println, sched, timer, trap};
```

Place `timer::init(machine.timebase_hz);` immediately before the existing
`timer::start();` (keep the spawn block between `pqc_demo()` and
`timer::init` exactly as it is).

- [ ] **Step 2: Build the kernel binary**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: SUCCESS.

- [ ] **Step 3: Run the smoke test — expect GREEN**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS` including `dt: 192 MiB RAM` (the kernel read the DTB; under `-m 192M` it reports and maps 192 MiB). All prior milestones still pass over the discovered RAM/clock. If `dt:` shows a wrong size or the boot faults, the parser or the `mem::init`/`timer::init` wiring is off — diagnose, don't weaken the test.

- [ ] **Step 4: Commit**

```bash
git add kernel/src/main.rs
git commit -m "feat: Phase 4a live - kmain discovers RAM + timebase from the device tree"
```

---

## Task 5: docs — short learning note, roadmap (Phase 4 decomposed), glossary; final verification

**Files:**
- Create: `docs/learning/0011-device-tree.md`
- Modify: `docs/roadmap/roadmap.md`
- Modify: `docs/glossary.md`
- Modify: `docs/learning/README.md`

- [ ] **Step 1: Write a short learning note**

Create `docs/learning/0011-device-tree.md`:

```markdown
# 0011 — Discovering hardware from the device tree (Phase 4a)

**One-line:** the kernel learns its RAM size and timer frequency from the
device tree the firmware passes it, instead of hardcoding QEMU's numbers.

## What changed
- OpenSBI passes the address of a *flattened device tree* (FDT) in `a1` —
  the `dtb` argument `kmain` always received and ignored. A small hand-rolled
  `no_std` parser (`arch/riscv64/src/dt.rs`) reads the `/memory` node's `reg`
  (RAM base+size) and `timebase-frequency`.
- `mem::init` now takes the discovered `ram_end`; `timer::init` takes the
  discovered frequency. The hardcoded `RAM_END` (0x8800_0000) and
  `TIMEBASE_HZ` (10 MHz) constants are gone.

## The key idea
The FDT is a big-endian binary: a header, then a token stream
(BEGIN_NODE + name / PROP + len + name-offset + value / END_NODE / END) plus
a strings block. Walking it for two values is a few dozen lines — and it's
why the kernel can run on hardware that isn't QEMU.

## Proof
Host tests parse a captured real QEMU DTB (128 MiB, 0x8000_0000, 10 MHz) and
reject a bad/truncated blob. The smoke test boots QEMU with `-m 192M` and the
kernel reports **192 MiB** — proving it read the device tree, not a constant.

## Still QEMU-only
This needs no board. Next (Phase 4b) is discovering the UART from the DTB and
driving real serial — the step that actually boots on a physical RISC-V board.
```

- [ ] **Step 2: Update the roadmap (Phase 4 decomposed, 4a done)**

In `docs/roadmap/roadmap.md`, replace the `## Phase 4 — Real hardware` block
with:

```markdown
## Phase 4 — Real hardware

Decomposed (2026-06-20), continuing the RISC-V path (ADR 0003: one
architecture well before a second). The hardware-agnostic groundwork is
done first in QEMU; physical-board bring-up follows once a board is in hand.

### Phase 4a — Device-tree-driven discovery  *(done — 2026-06-20)*

- **Goal:** read RAM base/size and the timer frequency from the firmware's
  device tree instead of hardcoding QEMU's values.
- **You learn:** the FDT binary format and why a portable kernel discovers
  its machine from firmware (see [learning note 0011](../learning/0011-device-tree.md)).
- **Done when:** `./tools/test-qemu.ps1` (booting `-m 192M`) shows the kernel
  discover 192 MiB of RAM and the timebase from the device tree, with the
  parser host-tested against a real QEMU DTB. QEMU-only; no board.

### Phase 4b — Real UART + board bring-up

- **Goal:** discover the console UART from the device tree and drive real
  serial; boot on a physical RISC-V board (needs buying one).
- **Done when:** the kernel boots and prints on real hardware.

(If the RISC-V board route stalls, the owned x86-64 laptop remains a
separate, larger option — a new `arch/x86_64` with its own boot/discovery.)
```

- [ ] **Step 3: Add glossary entries**

In `docs/glossary.md`, add entries (in the file's format) for: **device tree
/ FDT** (a firmware-provided data structure describing the hardware — CPUs,
memory, devices — that a portable kernel reads at boot instead of hardcoding;
"flattened device tree" is its compact binary form, passed by address in a
register), **`timebase-frequency`** (the device-tree property giving the rate
of the `time` counter, used to derive timer deadlines), and **memory node**
(the device-tree node whose `reg` gives RAM base + size). Reuse existing
paging/timer terms.

- [ ] **Step 4: Update the learning-notes index**

In `docs/learning/README.md`, add under the notes list:

```markdown
- [0011 — Discovering hardware from the device tree (Phase 4a)](0011-device-tree.md)
```

- [ ] **Step 5: Final verification (run all, paste results)**

Run: `cargo test -p kernel-arch-riscv64` → expect all green (incl. the `dt::` tests).
Run: `cargo build --workspace` → expect success.
Run (PowerShell): `./tools/check-references.ps1` → expect PASS (fix any of YOUR broken references).
Run (PowerShell): `./tools/test-qemu.ps1` → expect `BOOT TEST PASS`.

- [ ] **Step 6: Commit**

```bash
git add docs/learning/0011-device-tree.md docs/roadmap/roadmap.md docs/glossary.md docs/learning/README.md
git commit -m "docs: Phase 4a learning note, roadmap (Phase 4 decomposed), glossary terms"
```

---

## Done-when checklist (maps to spec §1)

- [ ] Dynamic RAM discovery — smoke pattern `dt: 192 MiB RAM` under `-m 192M` (not the old hardcoded 128); paging/frames operate over it.
- [ ] Dynamic timebase — the `dt:` line reports the discovered frequency; ≥ 2 ticks run off it.
- [ ] Host tests — `parse()` on the real QEMU DTB fixture (128 MiB, 0x8000_0000, 10 MHz); bad-magic and truncated blobs → `None`.
- [ ] `check-references` clean; `cargo build --workspace` green; `BOOT TEST PASS`.
```
