# virtio-rng Entropy Component Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline, per the user's preference for this project) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A user-space `entropy` component drives the QEMU virtio-rng device for real hardware entropy and delivers a 32-byte seed over IPC to a kernel consumer that seeds ML-KEM — retiring Phase 3c's fixed seed.

**Architecture:** The kernel discovers the RNG (probes the virtio-mmio slots for DeviceID 4), allocates a zeroed identity-mapped DMA frame, maps the MMIO + DMA pages RW-U into the component, and passes their bases as launch-register arguments (generalizing 5b's `a0`-generation). The component does the entire modern virtio-mmio v2 handshake + one split virtqueue in inline asm (no kernel `.text`/`.rodata`), draws twice, and IPCs each 32-byte draw to a kernel `pqc` task that compares them (live-source check) and runs the ML-KEM round-trip on the seed.

**Tech Stack:** Rust `no_std`, RISC-V (riscv64gc), QEMU (`-global virtio-mmio.force-legacy=false -device virtio-rng-device`). Host tests: `cargo test -p kernel-arch-riscv64`. Bare: `cargo build -p kernel --target riscv64gc-unknown-none-elf`.

**Spec:** `docs/superpowers/specs/2026-06-20-virtio-rng-entropy-component-design.md`

**SPIKE-VERIFIED facts (kernel-side bring-up run under this QEMU 11.0):**
- RNG is at virtio-mmio slot **`0x1000_8000`** (QEMU attaches the first `-device` to the highest slot). The 8 slots are `0x1000_1000`..`0x1000_8000`, 0x1000 apart.
- QEMU virtio-mmio defaults to **legacy (Version 1)**; modern (Version 2) requires `-global virtio-mmio.force-legacy=false`. We target **modern v2**.
- Verified register offsets/sequence: Magic `0x000`="virt"(`0x74726976`), Version `0x004`, DeviceID `0x008`(=4), DeviceFeatures `0x010`/sel `0x014`, DriverFeatures `0x020`/sel `0x024`, QueueSel `0x030`, QueueNumMax `0x034`(=1024), QueueNum `0x038`, QueueReady `0x044`, QueueNotify `0x050`, Status `0x070`, QueueDesc `0x080/0x084`, QueueDriver `0x090/0x094`, QueueDevice `0x0a0/0x0a4`. Status bits ACK=1, DRIVER=2, DRIVER_OK=4, FEATURES_OK=8 (read-back `0xb` after FEATURES_OK). Feature `VIRTIO_F_VERSION_1` = bit 32 (sel=1, bit 0). Descriptor flag WRITE=2.
- DMA layout used (one zeroed 4 KiB frame, QSIZE=8): desc@0, avail@128, used@160, buf@256. The device filled the buffer with **0 poll spins**; entropy differs every run.

**Existing-code facts:**
- `dt::parse` already buffers per-node `node_reg`/`compatible` and commits at `END_NODE` (UART, RTC). The fixture `arch/riscv64/tests/fixtures/qemu-virt.dtb` contains 8 `virtio,mmio` nodes.
- `mem::build_user_space(stack, device)` maps `.user_text` R-X-U, the stack RW-U, and one `device` region **R-U**.
- `forge_user_context(..., generation)` sets `s[3]=generation`; `user_trampoline` does `mv a0, s3`. `Context.s` is `[usize; 12]` = `s0..s11`.
- IPC: `ipc_send`/`ipc_recv` operate on a `TrapFrame`; `Message { badge, data:[usize;3] }`; `find_blocked(s, ep, role)`, `block_current`, `cap_lookup`, `IpcRole`, `EndpointId`, `TaskState` are in scope in `sched.rs`. Reserved endpoints so far: `EP0`=0 (RTC), `sched::CRASH_EP`=1 (5b).
- `MAX_TASKS = 8`; `Scheduler::new` initializes an 8-element `[None; ..]`. Current cast: rtc(0) client(1) rogue(2) healer(3) transient(4) flaky(5) idle(6).
- `kmain` calls `pqc_demo()` (fixed seed `[0x3c;32]`) early, before the scheduler.

**Cast after this plan (MAX_TASKS → 10):** rtc(0) · client(1) · rogue(2) · healer(3) · transient(4) · flaky(5) · **pqc(6)** · **entropy(7)** · idle(8). The consumer (slot 6) recv-blocks before the component (slot 7) sends.

---

## Task 1: device-tree discovery of the virtio-mmio slots

**Files:**
- Modify: `arch/riscv64/src/dt.rs`

- [ ] **Step 1: Extend the fixture test (failing)**

In `arch/riscv64/src/dt.rs`, add to the `parses_qemu_virt` test (after the `rtc_base` assertion):

```rust
        assert_eq!(mi.virtio_mmio_count, 8, "QEMU virt has 8 virtio-mmio slots");
        assert!(mi.virtio_mmio[..mi.virtio_mmio_count].contains(&0x1000_1000), "lowest slot");
        assert!(mi.virtio_mmio[..mi.virtio_mmio_count].contains(&0x1000_8000), "highest slot (where -device attaches)");
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p kernel-arch-riscv64 dt::`
Expected: FAIL — `MachineInfo` has no `virtio_mmio`/`virtio_mmio_count` fields.

- [ ] **Step 3: Add the fields and collect the bases**

In `arch/riscv64/src/dt.rs`, add to `MachineInfo` (after `rtc_base`):

```rust
    pub rtc_base: usize,
    /// Bases of the `virtio,mmio` transport slots (QEMU `virt` exposes 8).
    pub virtio_mmio: [usize; 8],
    pub virtio_mmio_count: usize,
```

In `parse`, add the per-node flag and accumulator near the other node state:

```rust
    let mut node_is_rtc = false;
    let mut rtc: Option<usize> = None;
    let mut node_is_virtio = false;
    let mut virtio_mmio = [0usize; 8];
    let mut virtio_count = 0usize;
```

In the `FDT_BEGIN_NODE` arm, reset it with the others:

```rust
                node_is_rtc = false;
                node_is_virtio = false;
```

In the `FDT_END_NODE` arm, commit it (after the RTC commit):

```rust
                if node_is_virtio {
                    if let Some(b) = node_reg {
                        if virtio_count < virtio_mmio.len() {
                            virtio_mmio[virtio_count] = b;
                            virtio_count += 1;
                        }
                    }
                }
```

In the `FDT_PROP` arm, detect the compatible (after the goldfish match):

```rust
                if pname == b"compatible" && val.windows(11).any(|w| w == b"virtio,mmio") {
                    node_is_virtio = true;
                }
```

Add the fields to the returned struct:

```rust
        rtc_base: rtc?,
        virtio_mmio,
        virtio_mmio_count: virtio_count,
    })
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p kernel-arch-riscv64 dt::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/dt.rs
git commit -m "feat(dt): discover the virtio-mmio transport slots"
```

---

## Task 2: the virtio module — constants, RNG match, and the probe

**Files:**
- Create: `arch/riscv64/src/virtio.rs`
- Modify: `arch/riscv64/src/lib.rs`

- [ ] **Step 1: Create `virtio.rs` with the verified constants, helpers, and tests**

Create `arch/riscv64/src/virtio.rs`:

```rust
//! virtio-mmio (modern, Version 2) constants and the RNG probe — the kernel
//! side of the user-space entropy driver.
//!
//! The kernel only *discovers* the RNG (probes the transport slots for the
//! entropy DeviceID) and hands the component its mapped MMIO + DMA. The
//! component does the handshake/virtqueue itself (in U-mode inline asm), so it
//! references these `const`s directly (each folds to an immediate — no call
//! into kernel `.text`, no `.rodata` read). All values are spike-verified
//! against QEMU's `virtio-rng-device` on the `virt` machine.

// --- virtio-mmio register offsets (modern, Version 2) ---
pub const MAGIC: usize = 0x000;
pub const VERSION: usize = 0x004;
pub const DEVICE_ID: usize = 0x008;
pub const DEVICE_FEATURES: usize = 0x010;
pub const DEVICE_FEATURES_SEL: usize = 0x014;
pub const DRIVER_FEATURES: usize = 0x020;
pub const DRIVER_FEATURES_SEL: usize = 0x024;
pub const QUEUE_SEL: usize = 0x030;
pub const QUEUE_NUM: usize = 0x038;
pub const QUEUE_READY: usize = 0x044;
pub const QUEUE_NOTIFY: usize = 0x050;
pub const STATUS: usize = 0x070;
pub const QUEUE_DESC_LOW: usize = 0x080;
pub const QUEUE_DESC_HIGH: usize = 0x084;
pub const QUEUE_DRIVER_LOW: usize = 0x090;
pub const QUEUE_DRIVER_HIGH: usize = 0x094;
pub const QUEUE_DEVICE_LOW: usize = 0x0a0;
pub const QUEUE_DEVICE_HIGH: usize = 0x0a4;

// --- magic / device id / status bits / feature / descriptor flag ---
pub const MAGIC_VALUE: u32 = 0x7472_6976; // "virt"
pub const DEVICE_ID_RNG: u32 = 4;
pub const STATUS_ACK: u32 = 1;
pub const STATUS_DRIVER: u32 = 2;
pub const STATUS_DRIVER_OK: u32 = 4;
pub const STATUS_FEATURES_OK: u32 = 8;
/// `VIRTIO_F_VERSION_1` is feature bit 32 — i.e. bit 0 of the high word
/// (DeviceFeaturesSel = 1).
pub const F_VERSION_1_HI: u32 = 1;
pub const VIRTQ_DESC_F_WRITE: u16 = 2;

// --- split-virtqueue + DMA layout within one zeroed 4 KiB frame (QSIZE=8) ---
pub const VQ_SIZE: u32 = 8;
pub const VQ_DESC_OFF: usize = 0;
pub const VQ_AVAIL_OFF: usize = 128;
pub const VQ_USED_OFF: usize = 160;
pub const VQ_BUF_OFF: usize = 256;

/// Is this DeviceID the entropy source?
pub fn is_rng(device_id: u32) -> bool {
    device_id == DEVICE_ID_RNG
}

/// Probe `bases` (the discovered virtio-mmio slots) for the RNG: the first
/// slot whose Magic is "virt" and DeviceID is 4. Called once in early boot
/// (MMU off), like reading the device tree.
///
/// # Safety
/// Each non-zero base must address a valid virtio-mmio register page.
#[cfg(target_arch = "riscv64")]
pub unsafe fn find_rng(bases: &[usize]) -> Option<usize> {
    for &b in bases {
        if b != 0
            && core::ptr::read_volatile((b + MAGIC) as *const u32) == MAGIC_VALUE
            && core::ptr::read_volatile((b + DEVICE_ID) as *const u32) == DEVICE_ID_RNG
        {
            return Some(b);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_rng_matches_only_the_entropy_id() {
        assert!(is_rng(4));
        assert!(!is_rng(0)); // empty slot
        assert!(!is_rng(2)); // block device
    }

    #[test]
    fn dma_layout_is_aligned_and_non_overlapping() {
        // desc table is QSIZE*16 bytes; avail must follow it.
        assert!(VQ_AVAIL_OFF >= VQ_DESC_OFF + VQ_SIZE as usize * 16);
        // used ring (16-aligned) must follow the avail ring
        // (flags 2 + idx 2 + ring 2*QSIZE + used_event 2).
        assert_eq!(VQ_USED_OFF % 16, 0, "used ring 16-aligned");
        assert!(VQ_USED_OFF >= VQ_AVAIL_OFF + 4 + 2 * VQ_SIZE as usize + 2);
        // buffer must follow the used ring (flags 2 + idx 2 + ring 8*QSIZE + 2)
        // and stay within the 4 KiB DMA frame.
        assert!(VQ_BUF_OFF >= VQ_USED_OFF + 4 + 8 * VQ_SIZE as usize + 2);
        assert!(VQ_BUF_OFF + 32 <= 4096, "32-byte buffer fits in the frame");
    }
}
```

- [ ] **Step 2: Declare the module in `lib.rs`**

In `arch/riscv64/src/lib.rs`, add after the `heal` module declaration:

```rust
/// virtio-mmio constants + the RNG probe (the kernel side of the user-space
/// entropy driver). Pure constants/helpers host-tested; the gated probe reads
/// device registers.
pub mod virtio;
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p kernel-arch-riscv64 virtio::`
Expected: PASS — `is_rng_matches_only_the_entropy_id`, `dma_layout_is_aligned_and_non_overlapping`.
Run: `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf`
Expected: SUCCESS.

- [ ] **Step 4: Commit**

```bash
git add arch/riscv64/src/virtio.rs arch/riscv64/src/lib.rs
git commit -m "feat(virtio): virtio-mmio v2 constants + RNG probe (host-tested)"
```

---

## Task 3: map the MMIO + DMA pages into the component (RW-U)

**Files:**
- Modify: `arch/riscv64/src/mem/mod.rs`

- [ ] **Step 1: Add `build_virtio_space`**

In `arch/riscv64/src/mem/mod.rs`, add after `build_user_space`:

```rust
/// Like [`build_user_space`], but for a virtio component: maps the device's
/// MMIO register page **and** a DMA region **RW-U, identity** (VA=PA) into the
/// component, in addition to `.user_text` (R-X-U) and the `stack` (RW-U). The
/// device DMAs to the physical addresses the component writes into
/// descriptors, so VA must equal PA — both regions are identity-mapped, and
/// mapped only here (the component's exclusive device capability). All three
/// region tuples are half-open `(start, end)`, page-aligned.
#[cfg(target_arch = "riscv64")]
pub fn build_virtio_space(
    stack: (usize, usize),
    mmio: (usize, usize),
    dma: (usize, usize),
) -> usize {
    use paging::{PTE_R, PTE_U, PTE_W, PTE_X};
    // SAFETY: as in build_user_space — fresh zeroed root, valid page-aligned
    // ranges, built on the master satp. MMIO + DMA are RW-U (the device
    // registers are written; the DMA rings are read/written by both sides).
    unsafe {
        let root = frame::alloc_zeroed()
            .expect("no frame for virtio component root page table")
            .0 as *mut paging::PageTable;
        map_kernel_sections(root);
        paging::map_range(root, sym!(__user_text_start), sym!(__user_text_end), PTE_R | PTE_X | PTE_U);
        paging::map_range(root, stack.0, stack.1, PTE_R | PTE_W | PTE_U);
        paging::map_range(root, mmio.0, mmio.1, PTE_R | PTE_W | PTE_U);
        paging::map_range(root, dma.0, dma.1, PTE_R | PTE_W | PTE_U);
        paging::make_satp(root as usize)
    }
}
```

- [ ] **Step 2: Verify it compiles (bare)**

Run: `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf`
Expected: SUCCESS.

- [ ] **Step 3: Commit**

```bash
git add arch/riscv64/src/mem/mod.rs
git commit -m "feat(mem): build_virtio_space - map MMIO + DMA pages RW-U (identity) into a component"
```

---

## Task 4: kernel IPC for a kernel task, launch-register args, bigger run queue

**Files:**
- Modify: `arch/riscv64/src/sched.rs`

- [ ] **Step 1: Pass launch args `a1`/`a2` (`mv a1,s4; mv a2,s5`)**

In `arch/riscv64/src/sched.rs`, in the `user_trampoline` global_asm, add the two moves right after the existing `mv a0, s3` line (before `sret`):

```rust
    mv a0, s3               # a0 = launch generation (forge put it in s3)
    mv a1, s4               # a1 = launch arg 1 (set_launch_args put it in s4)
    mv a2, s5               # a2 = launch arg 2 (set_launch_args put it in s5)
    sret                    # -> U-mode at the entry point
```

- [ ] **Step 2: Bump `MAX_TASKS` to 10 (constant + initializer)**

In `arch/riscv64/src/sched.rs`, update the constant and doc:

```rust
/// Maximum concurrent tasks: the demo runs nine (rtc, client, rogue, healer,
/// transient, flaky, pqc, entropy, idle) plus one slot of headroom.
pub const MAX_TASKS: usize = 10;
```

And the `Scheduler::new` initializer:

```rust
        Self { tasks: [None, None, None, None, None, None, None, None, None, None], current: 0 }
```

- [ ] **Step 3: Add `set_launch_args`**

In `arch/riscv64/src/sched.rs`, add near `set_crash_badge`:

```rust
/// Set the U-mode launch-register arguments of the task in scheduler `slot`:
/// `a1`/`a2` at first run (stored in the forged context's `s4`/`s5`; the
/// trampoline moves them into `a1`/`a2`). Used to hand a driver its device
/// addresses. (Not re-applied on a 5b restart — restartable launch args are
/// future work; the entropy component is not a restart patient.)
#[cfg(target_arch = "riscv64")]
pub fn set_launch_args(slot: usize, a1: usize, a2: usize) {
    SCHED.with(|s| {
        let t = s.tasks[slot].as_mut().expect("set_launch_args: empty task slot");
        t.context.s[4] = a1;
        t.context.s[5] = a2;
    });
}
```

- [ ] **Step 4: Add `recv_message` (a kernel task's IPC receive)**

In `arch/riscv64/src/sched.rs`, add after `ipc_recv` (it mirrors the recv rendezvous but returns the `Message` directly — kernel tasks have no `TrapFrame` to write):

```rust
/// Receive a [`Message`] on the endpoint named by capability `cap_idx`, for a
/// **kernel** (S-mode) task (which cannot use the U-mode `ecall` IPC path).
/// Blocks until a sender arrives, then returns its message. The sender uses
/// the ordinary `ipc_send`; delivery is identical regardless of the receiver's
/// privilege. Panics if the caller lacks the capability (a kernel-config bug).
#[cfg(target_arch = "riscv64")]
pub fn recv_message(cap_idx: usize) -> crate::task::Message {
    enum K {
        Got(crate::task::Message),
        Block(EndpointId),
    }
    let step = SCHED.with(|s| {
        let cur = s.current;
        let ep = cap_lookup(&s.tasks[cur].as_ref().unwrap().caps, cap_idx)
            .expect("recv_message: caller lacks the endpoint capability");
        match find_blocked(s, ep, IpcRole::Send) {
            Some(si) => {
                let msg = s.tasks[si].as_ref().unwrap().message;
                s.tasks[si].as_mut().unwrap().state = TaskState::Ready;
                K::Got(msg)
            }
            None => K::Block(ep),
        }
    });
    match step {
        K::Got(msg) => msg,
        K::Block(ep) => {
            block_current(ep, IpcRole::Recv);
            SCHED.with(|s| s.tasks[s.current].as_ref().unwrap().message)
        }
    }
}
```

- [ ] **Step 5: Verify host tests + bare build**

Run: `cargo test -p kernel-arch-riscv64`
Expected: PASS (gated code excluded on host; existing suite green).
Run: `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf`
Expected: SUCCESS.

- [ ] **Step 6: Commit**

```bash
git add arch/riscv64/src/sched.rs
git commit -m "feat(sched): kernel-task recv_message, launch-register args (a1/a2), MAX_TASKS=10"
```

---

## Task 5: extend the smoke test for entropy-seeded PQC (red)

**Files:**
- Modify: `tools/test-qemu.ps1`

- [ ] **Step 1: Add the QEMU flags for modern virtio-rng**

In `tools/test-qemu.ps1`, in the `Start-Process qemu-system-riscv64 ... -ArgumentList @(...)` array, add (after the `"-m", "192M",` entry):

```powershell
    "-global", "virtio-mmio.force-legacy=false",
    "-device", "virtio-rng-device",
```

- [ ] **Step 2: Swap the PQC pattern and add the entropy pattern**

In `tools/test-qemu.ps1`, replace the existing PQC line in `$mustMatch`:

```powershell
    "pqc: ML-KEM-768 round-trip ok",
```

with:

```powershell
    "entropy: virtio-rng live \(two draws differ\)",
    "pqc: ML-KEM-768 round-trip ok \(entropy-seeded\)",
```

Update the header comment + PASS message to mention the entropy component (a user-space virtio-rng driver provides real entropy that seeds ML-KEM, replacing the fixed seed).

- [ ] **Step 3: Run the smoke test — expect RED**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: FAIL — the two new patterns are absent (no entropy component yet); the early fixed-seed `pqc_demo` still prints the old `(shared secret agreed)` line, which no longer matches.

- [ ] **Step 4: Commit (red test pinned)**

```bash
git add tools/test-qemu.ps1
git commit -m "test: smoke test asserts entropy-seeded ML-KEM via virtio-rng (red)"
```

---

## Task 6: the entropy component + PQC consumer + wiring (green)

**Files:**
- Modify: `kernel/src/main.rs`

- [ ] **Step 1: Import `Message` and `virtio`**

In `kernel/src/main.rs`, update the arch-crate import:

```rust
    use kernel_arch_riscv64::{cap::Capability, console, dt, mem, println, sched, task::Message, timer, trap, virtio};
```

- [ ] **Step 2: Remove the early fixed-seed PQC demo**

In `kernel/src/main.rs`, delete the `pqc_demo();` call in `kmain` (it sits after `frame_roundtrip();`), and delete the whole `fn pqc_demo() { ... }` definition. (ML-KEM now runs entropy-seeded in the `pqc` consumer task.)

- [ ] **Step 3: Add endpoint/cap constants and the stacks**

In `kernel/src/main.rs`, after the `CRASH_CAP` constant, add:

```rust
    /// The endpoint the entropy component delivers seeds on, and the cap slot
    /// it/​the consumer hold it in.
    const ENTROPY_EP: usize = 2;
    const ENTROPY_CAP: usize = 0;
```

Add kernel stacks (after `KS_FLAKY`):

```rust
    static mut KS_PQC: KStack = [0; TASK_STACK];
    static mut KS_ENTROPY: KStack = [0; TASK_STACK];
```

Add the entropy component's user stack (after `US_FLAKY`):

```rust
    #[link_section = ".user_data"]
    static mut US_ENTROPY: UStack = UStack([0; USER_STACK_SIZE]);
```

- [ ] **Step 4: Add the `send4` syscall wrapper and the MMIO/DMA inline-asm helpers**

In `kernel/src/main.rs`, after `sys_restart`, add the 4-word send and the driver primitives (all `#[inline(always)]`, so they inline into `.user_text` — no call into kernel `.text`):

```rust
    /// send syscall (a7 = 4) carrying a full 32-byte payload: a0 = cap, a1 =
    /// badge (= word 0), a2..a4 = words 1..3. Returns a0.
    ///
    /// # Safety
    /// Always sound; the kernel validates the capability and may block us.
    #[inline(always)]
    unsafe fn sys_send4(cap: usize, w0: usize, w1: usize, w2: usize, w3: usize) -> usize {
        let ret;
        core::arch::asm!(
            "ecall",
            in("a7") 4usize,
            inout("a0") cap => ret,
            in("a1") w0,
            in("a2") w1,
            in("a3") w2,
            in("a4") w3,
            options(nostack),
        );
        ret
    }

    /// MMIO register write (32-bit). # Safety: `base+off` must be a mapped
    /// device register.
    #[inline(always)]
    unsafe fn mmio_w(base: usize, off: usize, v: u32) {
        core::arch::asm!("sw {v}, 0({a})", v = in(reg) v, a = in(reg) base + off, options(nostack));
    }
    /// MMIO register read (32-bit).
    #[inline(always)]
    unsafe fn mmio_r(base: usize, off: usize) -> u32 {
        let v;
        core::arch::asm!("lw {v}, 0({a})", v = out(reg) v, a = in(reg) base + off, options(nostack));
        v
    }
    /// DMA stores (the rings/descriptor live in a mapped DMA page).
    #[inline(always)]
    unsafe fn dma_w64(addr: usize, v: u64) {
        core::arch::asm!("sd {v}, 0({a})", v = in(reg) v, a = in(reg) addr, options(nostack));
    }
    #[inline(always)]
    unsafe fn dma_w32(addr: usize, v: u32) {
        core::arch::asm!("sw {v}, 0({a})", v = in(reg) v, a = in(reg) addr, options(nostack));
    }
    #[inline(always)]
    unsafe fn dma_w16(addr: usize, v: u16) {
        core::arch::asm!("sh {v}, 0({a})", v = in(reg) v, a = in(reg) addr, options(nostack));
    }
    /// DMA reads.
    #[inline(always)]
    unsafe fn dma_r64(addr: usize) -> u64 {
        let v;
        core::arch::asm!("ld {v}, 0({a})", v = out(reg) v, a = in(reg) addr, options(nostack));
        v
    }
    #[inline(always)]
    unsafe fn dma_r16(addr: usize) -> u16 {
        let v;
        core::arch::asm!("lhu {v}, 0({a})", v = out(reg) v, a = in(reg) addr, options(nostack));
        v
    }
    #[inline(always)]
    unsafe fn dma_fence() {
        core::arch::asm!("fence", options(nostack));
    }
```

- [ ] **Step 5: Add the entropy component (the user-space virtio-rng driver)**

In `kernel/src/main.rs`, add after `flaky_task` (the verified protocol, in inline asm; offsets/bits from `virtio::*`):

```rust
    /// The entropy component (user-space virtio-rng driver). The kernel maps
    /// the device's MMIO page and a zeroed DMA frame RW-U (identity) into this
    /// task and passes their bases at launch (a1 = mmio, a2 = dma). It drives
    /// the modern virtio-mmio v2 handshake + one split virtqueue entirely in
    /// inline asm (no kernel `.text`/`.rodata`), draws 32 random bytes twice,
    /// and sends each draw (32 bytes = badge + 3 words) to the `pqc` consumer.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn entropy_component() -> ! {
        let mmio: usize;
        let dma: usize;
        // SAFETY: read the launch args the kernel placed in a1/a2.
        unsafe {
            core::arch::asm!("mv {m}, a1", "mv {d}, a2",
                m = out(reg) mmio, d = out(reg) dma,
                options(nomem, nostack, preserves_flags));
        }
        let desc = dma + virtio::VQ_DESC_OFF;
        let avail = dma + virtio::VQ_AVAIL_OFF;
        let used = dma + virtio::VQ_USED_OFF;
        let buf = dma + virtio::VQ_BUF_OFF;
        // SAFETY: mmio + dma are mapped RW-U into this task; the sequence is
        // the spike-verified modern virtio-mmio v2 bring-up.
        unsafe {
            // Status handshake.
            mmio_w(mmio, virtio::STATUS, 0);
            mmio_w(mmio, virtio::STATUS, virtio::STATUS_ACK);
            mmio_w(mmio, virtio::STATUS, virtio::STATUS_ACK | virtio::STATUS_DRIVER);
            // Feature negotiation: accept only VIRTIO_F_VERSION_1 (bit 32).
            mmio_w(mmio, virtio::DEVICE_FEATURES_SEL, 1);
            let fhi = mmio_r(mmio, virtio::DEVICE_FEATURES);
            mmio_w(mmio, virtio::DRIVER_FEATURES_SEL, 1);
            mmio_w(mmio, virtio::DRIVER_FEATURES, fhi & virtio::F_VERSION_1_HI);
            mmio_w(mmio, virtio::DRIVER_FEATURES_SEL, 0);
            mmio_w(mmio, virtio::DRIVER_FEATURES, 0);
            mmio_w(mmio, virtio::STATUS,
                virtio::STATUS_ACK | virtio::STATUS_DRIVER | virtio::STATUS_FEATURES_OK);
            // Queue 0 setup (modern: independent ring addresses).
            mmio_w(mmio, virtio::QUEUE_SEL, 0);
            mmio_w(mmio, virtio::QUEUE_NUM, virtio::VQ_SIZE);
            mmio_w(mmio, virtio::QUEUE_DESC_LOW, desc as u32);
            mmio_w(mmio, virtio::QUEUE_DESC_HIGH, (desc >> 32) as u32);
            mmio_w(mmio, virtio::QUEUE_DRIVER_LOW, avail as u32);
            mmio_w(mmio, virtio::QUEUE_DRIVER_HIGH, (avail >> 32) as u32);
            mmio_w(mmio, virtio::QUEUE_DEVICE_LOW, used as u32);
            mmio_w(mmio, virtio::QUEUE_DEVICE_HIGH, (used >> 32) as u32);
            mmio_w(mmio, virtio::QUEUE_READY, 1);
            mmio_w(mmio, virtio::STATUS,
                virtio::STATUS_ACK | virtio::STATUS_DRIVER | virtio::STATUS_FEATURES_OK | virtio::STATUS_DRIVER_OK);
            // Descriptor 0: device writes 32 bytes into `buf`.
            dma_w64(desc, buf as u64);
            dma_w32(desc + 8, 32);
            dma_w16(desc + 12, virtio::VIRTQ_DESC_F_WRITE);
            dma_w16(desc + 14, 0);
            // Two draws.
            let mut idx: u16 = 0;
            let mut n = 0;
            while n < 2 {
                dma_w16(avail + 4, 0); // avail.ring[0] = descriptor 0
                dma_fence();
                idx += 1;
                dma_w16(avail + 2, idx); // avail.idx
                dma_fence();
                mmio_w(mmio, virtio::QUEUE_NOTIFY, 0);
                loop {
                    dma_fence();
                    if dma_r16(used + 2) == idx {
                        break;
                    }
                }
                let w0 = dma_r64(buf) as usize;
                let w1 = dma_r64(buf + 8) as usize;
                let w2 = dma_r64(buf + 16) as usize;
                let w3 = dma_r64(buf + 24) as usize;
                sys_send4(ENTROPY_CAP, w0, w1, w2, w3);
                n += 1;
            }
            sys_exit(0)
        }
    }
```

- [ ] **Step 6: Add the PQC consumer (a kernel task)**

In `kernel/src/main.rs`, add after `entropy_component` (kernel task: full privilege, may call `kernel_crypto`):

```rust
    /// Rebuild a 32-byte ML-KEM seed from an IPC message (badge + 3 data words,
    /// each a little-endian `u64`).
    fn seed_from_message(m: &Message) -> [u8; 32] {
        let words = [m.badge as u64, m.data[0] as u64, m.data[1] as u64, m.data[2] as u64];
        let mut out = [0u8; 32];
        for (i, w) in words.iter().enumerate() {
            out[i * 8..i * 8 + 8].copy_from_slice(&w.to_le_bytes());
        }
        out
    }

    /// The entropy consumer (kernel task): receives two 32-byte draws from the
    /// entropy component, confirms they differ (a live source), and runs the
    /// ML-KEM-768 round-trip seeded by real device entropy — retiring Phase
    /// 3c's fixed seed. Then idles cooperatively (kernel tasks never return).
    extern "C" fn pqc_consumer() -> ! {
        let m1 = sched::recv_message(ENTROPY_CAP);
        let m2 = sched::recv_message(ENTROPY_CAP);
        let seed1 = seed_from_message(&m1);
        let seed2 = seed_from_message(&m2);
        if seed1 != seed2 {
            println!("entropy: virtio-rng live (two draws differ)");
        } else {
            println!("entropy: WARNING virtio-rng two draws identical");
        }
        match kernel_crypto::ml_kem768_agree(seed1) {
            Some(_) => println!("pqc: ML-KEM-768 round-trip ok (entropy-seeded)"),
            None => println!("pqc: ML-KEM-768 FAIL (secrets disagreed)"),
        }
        loop {
            sched::yield_now();
            // SAFETY: wait for the next interrupt between yields.
            unsafe { core::arch::asm!("wfi") };
        }
    }
```

- [ ] **Step 7: Probe the RNG early (MMU off)**

In `kernel/src/main.rs` `kmain`, after the `dt:` print block and before `mem::init(...)`, add the probe (MMU still off, like reading the DTB):

```rust
        // Discover the virtio-rng device before paging is on (direct physical
        // MMIO reads), like the device-tree read above.
        // SAFETY: the discovered virtio-mmio bases address real register pages.
        let rng_base = unsafe { virtio::find_rng(&machine.virtio_mmio[..machine.virtio_mmio_count]) };
```

- [ ] **Step 8: Spawn the consumer + component**

In `kernel/src/main.rs` `kmain`, insert this just before the `idle` spawn (after the `flaky` block):

```rust
        // The entropy component (a user-space virtio-rng driver) + its kernel
        // consumer, if the device is present. The consumer (earlier slot)
        // recv-blocks before the component sends.
        if let Some(rng) = rng_base {
            let dma_pa = mem::frame::alloc_zeroed().expect("no DMA frame for entropy").0;
            let consumer = sched::spawn("pqc", pqc_consumer,
                core::ptr::addr_of!(KS_PQC) as usize + TASK_STACK);
            sched::grant_cap(consumer, ENTROPY_CAP, Capability::Endpoint(ENTROPY_EP));

            let eu = ustack(core::ptr::addr_of!(US_ENTROPY) as usize);
            let entropy = sched::spawn_user("entropy", entropy_component, eu.1,
                core::ptr::addr_of!(KS_ENTROPY) as usize + TASK_STACK,
                mem::build_virtio_space(eu, (rng, rng + 0x1000), (dma_pa, dma_pa + 0x1000)));
            sched::grant_cap(entropy, ENTROPY_CAP, Capability::Endpoint(ENTROPY_EP));
            sched::set_launch_args(entropy, rng, dma_pa);
        } else {
            println!("entropy: no virtio-rng device found");
        }
```

- [ ] **Step 9: Build the kernel**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: SUCCESS.

- [ ] **Step 10: Run the smoke test — expect GREEN**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS`, including `entropy: virtio-rng live (two draws differ)` and `pqc: ML-KEM-768 round-trip ok (entropy-seeded)`, with the RTC, healing (5a/5b), and all prior milestones still present.

Troubleshooting (diagnose, don't weaken the test):
- `entropy: no virtio-rng device found` → the smoke must pass `-global virtio-mmio.force-legacy=false -device virtio-rng-device` (Task 5); also confirm `find_rng` scans `machine.virtio_mmio[..count]`.
- Component faults (`sched: task 'entropy' killed by ...`) → a U-mode codegen slip: every MMIO/DMA access must go through the `#[inline(always)]` asm helpers (no `read_volatile`/struct literals); confirm the `a1`/`a2` read is the first thing in `entropy_component`.
- Hangs (used ring never advances) → check the `QUEUE_*` address writes use the DMA frame PA and that `force-legacy=false` is set (legacy ignores the modern queue registers).

- [ ] **Step 11: Commit**

```bash
git add kernel/src/main.rs
git commit -m "feat: virtio-rng entropy component - real entropy seeds ML-KEM, retiring the fixed seed"
```

---

## Task 7: docs — learning note, roadmap, glossary; final verification

**Files:**
- Create: `docs/learning/0016-virtio-rng-entropy.md`
- Modify: `docs/roadmap/roadmap.md`
- Modify: `docs/glossary.md`
- Modify: `docs/learning/README.md`

- [ ] **Step 1: Write a short learning note**

Create `docs/learning/0016-virtio-rng-entropy.md`:

```markdown
# 0016 — A virtio device in user space: the entropy component

**One-line:** a real virtio-rng driver runs as an unprivileged component and
feeds genuine hardware entropy into ML-KEM — retiring Phase 3c's fixed seed.

## What changed
- The kernel discovers the RNG (probes the virtio-mmio slots for DeviceID 4),
  allocates a zeroed identity-mapped DMA frame, maps the MMIO + DMA pages RW-U
  into the `entropy` component, and hands it their bases in launch registers
  (`a1`/`a2`, generalizing 5b's `a0`).
- The component does the whole modern virtio-mmio v2 bring-up itself, in
  inline asm: status/feature handshake, one split virtqueue, notify, poll the
  used ring. It draws 32 random bytes twice and sends each over IPC.
- A kernel `pqc` consumer task receives the two draws, confirms they differ (a
  live source), and runs ML-KEM-768 seeded by real entropy.

## The ideas worth keeping
1. **A virtqueue is just shared DMA memory + four rules.** A descriptor table
   (buffers), an available ring (driver → device), a used ring (device →
   driver), and a notify register; fences order the ring writes. Polling the
   used ring avoids needing interrupts.
2. **Identity mapping makes DMA trivial.** The device DMAs to *physical*
   addresses; with VA=PA the component writes the same value into a descriptor
   that it uses as a pointer. The kernel maps the DMA frame identity, RW-U,
   into the component only — the device is the component's capability.
3. **A complex driver still fits user space.** The kernel grants mapped MMIO +
   a DMA frame; the component does everything else unprivileged. The cost is
   the U-mode codegen wall — every access is inline asm, and constants come
   from `virtio::*` (folded to immediates, never a `.rodata` read).

## Why a spike first
virtio is unforgiving (wrong register/version/feature → silent no-op). A
throwaway kernel-side bring-up under QEMU verified the protocol, the offsets,
and that QEMU defaults to *legacy* virtio-mmio (modern needs
`-global virtio-mmio.force-legacy=false`) before any of it was committed.

## Proof
`entropy: virtio-rng live (two draws differ)` then `pqc: ML-KEM-768 round-trip
ok (entropy-seeded)` — real device entropy, varying every boot, now keys the
post-quantum round-trip.
```

- [ ] **Step 2: Update the roadmap**

In `docs/roadmap/roadmap.md`, under the "User-space components — realizing ADR 0007" section, add after the RTC component block:

```markdown
### Second component — virtio-rng entropy driver  *(done — 2026-06-20)*

- **Goal:** a more complex user-space driver — the QEMU virtio-rng device
  (virtqueue + DMA) — providing real hardware entropy that seeds ML-KEM,
  retiring Phase 3c's fixed seed.
- **You learn:** what a virtio device is (the virtio-mmio handshake, a split
  virtqueue as shared DMA memory), why identity mapping makes DMA trivial, and
  that even a complex driver fits the user-space-component model (see
  [learning note 0016](../learning/0016-virtio-rng-entropy.md)).
- **Done when:** `./tools/test-qemu.ps1` (with `-device virtio-rng-device`)
  shows the component draw two differing entropy samples and the ML-KEM-768
  round-trip succeed seeded by them. QEMU-only.
```

Also update the Phase 3c limitation note: in the "Limitations carried forward" text (it states crypto uses a FIXED non-secret seed), append: "— retired by the virtio-rng entropy component (the ML-KEM seed now comes from the device)."

- [ ] **Step 3: Add glossary entries**

In `docs/glossary.md`, add after the existing device/HAL cluster (only genuinely new terms):

```markdown
- **virtio** — a standard interface for *paravirtualized* devices (rng, block, net…): the guest talks to a simple, well-specified device the hypervisor provides, instead of emulating real hardware. Our entropy driver uses virtio-mmio (the memory-mapped transport).
- **Virtqueue** — the shared-memory channel between a virtio driver and device: a descriptor table (buffers) plus an *available* ring (driver→device) and a *used* ring (device→driver). The driver publishes a buffer and the device returns it filled.
- **DMA (Direct Memory Access)** — a device reading/writing system memory directly, at *physical* addresses, without the CPU copying each byte. The virtqueue and its buffers live in DMA memory shared with the device.
```

(If `MMIO` is not already defined, add: `- **MMIO (Memory-Mapped I/O)** — device registers exposed as memory addresses; the driver reads/writes them with ordinary loads/stores to a mapped page.`)

- [ ] **Step 4: Update the learning-notes index**

In `docs/learning/README.md`, add after the 0015 line:

```markdown
- [0016 — A virtio device in user space: the entropy component](0016-virtio-rng-entropy.md)
```

- [ ] **Step 5: Final verification (run all, paste results)**

Run: `cargo test -p kernel-arch-riscv64` → expect all green.
Run: `cargo build --workspace` → expect success.
Run (PowerShell): `./tools/check-references.ps1` → expect PASS (the new note + roadmap links resolve).
Run (PowerShell): `./tools/test-qemu.ps1` → expect `BOOT TEST PASS`.

- [ ] **Step 6: Commit**

```bash
git add docs/learning/0016-virtio-rng-entropy.md docs/roadmap/roadmap.md docs/glossary.md docs/learning/README.md
git commit -m "docs: virtio-rng entropy component - learning note, roadmap, glossary"
```

---

## Done-when checklist (maps to spec §1)

- [ ] **Live entropy** — smoke shows `entropy: virtio-rng live (two draws differ)`.
- [ ] **Entropy-seeded PQC** — smoke shows `pqc: ML-KEM-768 round-trip ok (entropy-seeded)`; the fixed-seed `pqc_demo` is gone.
- [ ] **Host tests** — `dt` collects 8 virtio-mmio bases; `virtio::is_rng` + the DMA-layout constants are validated.
- [ ] `check-references` clean; `cargo build --workspace` green; `BOOT TEST PASS`.
```
