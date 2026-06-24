# Phase 6a: block storage (virtio-blk) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline, per the user's preference for this project) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A user-space virtio-blk driver component reads and writes disk sectors over a virtqueue (interrupt-driven), proven by a self-test round-trip.

**Architecture:** A U-mode `blk` component owns the virtio-blk device (MMIO + an identity-mapped DMA frame), does the virtio handshake + a 3-descriptor request (header → data → status), blocks on its IRQ via `wait_irq`, and self-tests a write/read round-trip; a kernel consumer prints the result.

**Tech Stack:** Rust `no_std`, RISC-V (riscv64gc), QEMU (`-drive` + `virtio-blk-device`). Host tests: `cargo test -p kernel-arch-riscv64`. Bare: `cargo build -p kernel --target riscv64gc-unknown-none-elf`.

**Spec:** `docs/superpowers/specs/2026-06-23-phase-6a-block-storage-design.md`

**SPIKE-VERIFIED facts:** DeviceID 2; handshake identical to virtio-rng (accept only `VIRTIO_F_VERSION_1`). 3-desc chain: desc0=header(16B,`NEXT`,next=1), desc1=data(512B,`NEXT`[+`WRITE` if read],next=2), desc2=status(1B,`WRITE`,next=0). Header `{type:u32@0 (1=write/0=read), reserved:u32@4, sector:u64@8}`, status 0=OK. DMA offsets desc@0/avail@128/used@160/hdr@256/data@512/status@1024 (one 4 KiB frame). Writes persist; probe by DeviceID (slot-independent).

**Existing-code facts:** `virtio.rs` has the v2 register/status/feature constants, `VQ_*` layout (`VQ_DESC_OFF`=0/`VQ_AVAIL_OFF`=128/`VQ_USED_OFF`=160, `VQ_SIZE`=8), `VIRTQ_DESC_F_WRITE`=2, and `find_rng(bases) -> Option<usize>` (gated, called once in `kmain`: `let rng_base = unsafe { virtio::find_rng(&machine.virtio_mmio[..machine.virtio_mmio_count]) };`). `main.rs` has `#[inline(always)]` helpers `mmio_w`/`mmio_r`/`dma_w64`/`dma_w32`/`dma_w16`/`dma_r64`/`dma_fence`, the `entropy_component` (handshake + queue + a draw loop with `sys_wait_irq`/`INTERRUPT_STATUS`/`INTERRUPT_ACK`), `sys_send4`/`sys_wait_irq`/`sys_exit`, `recv_message`, and the entropy wiring (`build_virtio_space`, grant `Endpoint`+`Interrupt`, `set_launch_args`, `plic::init`/`set_priority`/`enable`, `csr::sie_enable_external`). Endpoints: `EP0`=0, `CRASH_EP`=1, `ENTROPY_EP`=2, `DEFER_EP`=3. `MAX_TASKS`=16; current cast is 13 tasks.

---

## Task 1: blk constants + a generalized device probe

**Files:**
- Modify: `arch/riscv64/src/virtio.rs`
- Modify: `kernel/src/main.rs` (the `find_rng` call site)

- [ ] **Step 1: Write the failing test**

In `arch/riscv64/src/virtio.rs`, add to `tests`:

```rust
    #[test]
    fn blk_dma_layout_is_ordered_within_the_frame() {
        assert!(BLK_HDR_OFF >= VQ_USED_OFF + 4 + 8 * VQ_SIZE as usize + 2, "header after the used ring");
        assert!(BLK_DATA_OFF >= BLK_HDR_OFF + 16, "data after the 16-byte header");
        assert!(BLK_STATUS_OFF >= BLK_DATA_OFF + BLK_SECTOR_SIZE, "status after the 512-byte data");
        assert!(BLK_STATUS_OFF + 1 <= 4096, "status byte fits in the 4 KiB frame");
        assert_eq!(DEVICE_ID_BLK, 2);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p kernel-arch-riscv64 virtio::`
Expected: FAIL — the blk constants do not exist.

- [ ] **Step 3: Add the blk constants and generalize the probe**

In `arch/riscv64/src/virtio.rs`, add after `VIRTQ_DESC_F_WRITE`:

```rust
pub const VIRTQ_DESC_F_NEXT: u16 = 1;
pub const DEVICE_ID_BLK: u32 = 2;
pub const BLK_T_IN: u32 = 0; // read (device writes the data buffer)
pub const BLK_T_OUT: u32 = 1; // write (device reads the data buffer)
pub const BLK_SECTOR_SIZE: usize = 512;
/// virtio-blk request buffers within the DMA frame (after the rings).
pub const BLK_HDR_OFF: usize = 256; // 16-byte header
pub const BLK_DATA_OFF: usize = 512; // 512-byte sector data
pub const BLK_STATUS_OFF: usize = 1024; // 1-byte status
```

Replace `find_rng` with a generalized `find_device`:

```rust
/// Probe `bases` (the discovered virtio-mmio slots) for the first device with
/// the given DeviceID (e.g. 4 = rng, 2 = block). Called in early boot (MMU
/// off), like reading the device tree.
///
/// # Safety
/// Each non-zero base must address a valid virtio-mmio register page.
#[cfg(target_arch = "riscv64")]
pub unsafe fn find_device(bases: &[usize], device_id: u32) -> Option<usize> {
    for &b in bases {
        if b != 0
            && core::ptr::read_volatile((b + MAGIC) as *const u32) == MAGIC_VALUE
            && core::ptr::read_volatile((b + DEVICE_ID) as *const u32) == device_id
        {
            return Some(b);
        }
    }
    None
}
```

(Delete the old `find_rng`.)

- [ ] **Step 4: Update the `kmain` call site**

In `kernel/src/main.rs`, change the RNG probe:

```rust
        let rng_base = unsafe { virtio::find_device(&machine.virtio_mmio[..machine.virtio_mmio_count], virtio::DEVICE_ID_RNG) };
```

- [ ] **Step 5: Run tests + bare build**

Run: `cargo test -p kernel-arch-riscv64 virtio::` → expect PASS.
Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf` → expect SUCCESS.

- [ ] **Step 6: Commit**

```bash
git add arch/riscv64/src/virtio.rs kernel/src/main.rs
git commit -m "feat(virtio): blk constants + generalized find_device probe"
```

---

## Task 2: update the smoke test for block storage (red)

**Files:**
- Modify: `tools/test-qemu.ps1`

- [ ] **Step 1: Create a scratch disk and add the QEMU args**

In `tools/test-qemu.ps1`, before the `Start-Process qemu-system-riscv64 …`, create a scratch raw disk image:

```powershell
$diskImg = Join-Path ([System.IO.Path]::GetTempPath()) "kernel-qemu-disk.img"
[System.IO.File]::WriteAllBytes($diskImg, (New-Object byte[] 65536))
```

In the `-ArgumentList @(...)` array (after the existing `-device virtio-rng-device,` line), add:

```powershell
    "-drive", "file=$diskImg,if=none,format=raw,id=blk0",
    "-device", "virtio-blk-device,drive=blk0",
```

Add to `$mustMatch` (after the entropy/pqc lines):

```powershell
    "blk: sector round-trip ok",
```

**Also** relax the existing PLIC assertion. With two interrupt-driven devices
(rng + blk) QEMU re-assigns the virtio-mmio slots, so the rng is no longer
necessarily IRQ 8, and the kernel's one-shot "first external interrupt" log may
name either driver on either IRQ. Replace the line:

```powershell
    "irq: external IRQ 8 woke 'entropy'",
```

with a device-agnostic one (any external IRQ waking any driver proves the PLIC
path; each driver delivering its data proves it is interrupt-driven):

```powershell
    "irq: external IRQ \d+ woke",
```

Update the header comment + PASS message to note a user-space virtio-blk driver
reads and writes a disk sector (a write/read round-trip), interrupt-driven.

- [ ] **Step 2: Run the smoke test — expect RED**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: FAIL — `blk: sector round-trip ok` is absent (no blk component yet). The disk and device are present but unused.

- [ ] **Step 3: Commit (red test pinned)**

```bash
git add tools/test-qemu.ps1
git commit -m "test: smoke test asserts a virtio-blk sector round-trip (red)"
```

---

## Task 3: the blk driver component + wiring (green)

**Files:**
- Modify: `kernel/src/main.rs`

- [ ] **Step 1: Add byte-wide DMA helpers**

In `kernel/src/main.rs`, next to the other `dma_*` helpers, add:

```rust
    #[inline(always)]
    unsafe fn dma_w8(addr: usize, v: u8) {
        core::arch::asm!("sb {v}, 0({a})", v = in(reg) v, a = in(reg) addr, options(nostack));
    }
    #[inline(always)]
    unsafe fn dma_r8(addr: usize) -> u8 {
        let v;
        core::arch::asm!("lbu {v}, 0({a})", v = out(reg) v, a = in(reg) addr, options(nostack));
        v
    }
```

- [ ] **Step 2: Add a shared virtio queue-0 init helper and the blk request**

In `kernel/src/main.rs`, add (near the other `#[inline(always)]` helpers):

```rust
    /// The virtio-mmio v2 status handshake + queue-0 setup (identical for every
    /// modern virtio device). `dma` is the identity-mapped frame holding the
    /// rings. Spike-verified against virtio-rng and virtio-blk.
    #[inline(always)]
    unsafe fn virtio_queue_init(mmio: usize, dma: usize) {
        let desc = dma + virtio::VQ_DESC_OFF;
        let avail = dma + virtio::VQ_AVAIL_OFF;
        let used = dma + virtio::VQ_USED_OFF;
        mmio_w(mmio, virtio::STATUS, 0);
        mmio_w(mmio, virtio::STATUS, virtio::STATUS_ACK);
        mmio_w(mmio, virtio::STATUS, virtio::STATUS_ACK | virtio::STATUS_DRIVER);
        mmio_w(mmio, virtio::DEVICE_FEATURES_SEL, 1);
        let fhi = mmio_r(mmio, virtio::DEVICE_FEATURES);
        mmio_w(mmio, virtio::DRIVER_FEATURES_SEL, 1);
        mmio_w(mmio, virtio::DRIVER_FEATURES, fhi & virtio::F_VERSION_1_HI);
        mmio_w(mmio, virtio::DRIVER_FEATURES_SEL, 0);
        mmio_w(mmio, virtio::DRIVER_FEATURES, 0);
        mmio_w(mmio, virtio::STATUS,
            virtio::STATUS_ACK | virtio::STATUS_DRIVER | virtio::STATUS_FEATURES_OK);
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
    }

    /// Issue one virtio-blk request for sector 0 (`write` = T_OUT else T_IN),
    /// publishing the 3-descriptor chain and blocking on the device IRQ.
    /// Returns the device status byte (0 = OK). `avail_idx` is the current
    /// available-ring index.
    #[inline(always)]
    unsafe fn blk_req(mmio: usize, dma: usize, write: bool, avail_idx: u16) -> u8 {
        let desc = dma + virtio::VQ_DESC_OFF;
        let avail = dma + virtio::VQ_AVAIL_OFF;
        let hdr = dma + virtio::BLK_HDR_OFF;
        let data = dma + virtio::BLK_DATA_OFF;
        let status = dma + virtio::BLK_STATUS_OFF;
        // request header
        dma_w32(hdr, if write { virtio::BLK_T_OUT } else { virtio::BLK_T_IN });
        dma_w32(hdr + 4, 0);
        dma_w64(hdr + 8, 0); // sector 0
        dma_w8(status, 0xff); // sentinel
        // desc 0: header (device reads)
        dma_w64(desc, hdr as u64);
        dma_w32(desc + 8, 16);
        dma_w16(desc + 12, virtio::VIRTQ_DESC_F_NEXT);
        dma_w16(desc + 14, 1);
        // desc 1: data (device WRITEs it on a read)
        dma_w64(desc + 16, data as u64);
        dma_w32(desc + 24, virtio::BLK_SECTOR_SIZE as u32);
        let data_flags = virtio::VIRTQ_DESC_F_NEXT
            | if write { 0 } else { virtio::VIRTQ_DESC_F_WRITE };
        dma_w16(desc + 28, data_flags);
        dma_w16(desc + 30, 2);
        // desc 2: status (device WRITEs)
        dma_w64(desc + 32, status as u64);
        dma_w32(desc + 40, 1);
        dma_w16(desc + 44, virtio::VIRTQ_DESC_F_WRITE);
        dma_w16(desc + 46, 0);
        // publish the head descriptor, notify, and wait for the interrupt
        dma_w16(avail + 4 + (avail_idx as usize % virtio::VQ_SIZE as usize) * 2, 0);
        dma_fence();
        dma_w16(avail + 2, avail_idx + 1);
        dma_fence();
        mmio_w(mmio, virtio::QUEUE_NOTIFY, 0);
        sys_wait_irq(BLK_IRQ_CAP);
        let is = mmio_r(mmio, virtio::INTERRUPT_STATUS);
        mmio_w(mmio, virtio::INTERRUPT_ACK, is);
        dma_r8(status)
    }
```

- [ ] **Step 3: Add the `BLK_*` constants and the stacks**

In `kernel/src/main.rs`, after the `DEFER_EP`/`DEFER_CAP` constants, add:

```rust
    /// The endpoint the blk component reports its self-test result on; the cap
    /// slot it (and the consumer) hold it in; and the slot of its Interrupt cap.
    const BLK_EP: usize = 4;
    const BLK_REPORT_CAP: usize = 0;
    const BLK_IRQ_CAP: usize = 1;
```

Add a kernel stack for the consumer (after `KS_DCLIENTB`):

```rust
    static mut KS_BLK: KStack = [0; TASK_STACK];
    static mut KS_BLKC: KStack = [0; TASK_STACK];
```

Add the component's user stack (after `US_DCLIENTB`):

```rust
    #[link_section = ".user_data"]
    static mut US_BLK: UStack = UStack([0; USER_STACK_SIZE]);
```

- [ ] **Step 4: Add the blk component and the result consumer**

In `kernel/src/main.rs`, add after the `dclient_b_task` (or near the other U-mode tasks):

```rust
    /// The blk component (user-space virtio-blk driver). The kernel maps the
    /// device's MMIO + a zeroed DMA frame into it (a1 = mmio, a2 = dma) and
    /// grants an Interrupt cap for `wait_irq`. It self-tests a sector
    /// round-trip: fill the data buffer with a pattern, WRITE sector 0, zero the
    /// buffer, READ sector 0, and verify the pattern was restored. Reports the
    /// result with a one-way send to the blk consumer.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn blk_component() -> ! {
        let mmio: usize;
        let dma: usize;
        // SAFETY: read the launch args the kernel placed in a1/a2.
        unsafe {
            core::arch::asm!("mv {m}, a1", "mv {d}, a2",
                m = out(reg) mmio, d = out(reg) dma,
                options(nomem, nostack, preserves_flags));
        }
        let data = dma + virtio::BLK_DATA_OFF;
        // SAFETY: mmio + dma are mapped RW-U into this task; the sequence is the
        // spike-verified virtio-blk bring-up.
        unsafe {
            virtio_queue_init(mmio, dma);
            // Fill the data buffer with a known pattern and WRITE sector 0.
            let mut i = 0;
            while i < virtio::BLK_SECTOR_SIZE {
                dma_w8(data + i, (i & 0xff) as u8);
                i += 1;
            }
            let wst = blk_req(mmio, dma, true, 0);
            // Zero the buffer, then READ sector 0 back into it.
            i = 0;
            while i < virtio::BLK_SECTOR_SIZE {
                dma_w8(data + i, 0);
                i += 1;
            }
            let rst = blk_req(mmio, dma, false, 1);
            // Verify the pattern was restored (so both write and read hit disk).
            let mut ok = wst == 0 && rst == 0;
            i = 0;
            while ok && i < virtio::BLK_SECTOR_SIZE {
                if dma_r8(data + i) != (i & 0xff) as u8 {
                    ok = false;
                }
                i += 1;
            }
            sys_send4(BLK_REPORT_CAP, if ok { 1 } else { 0 }, 0, 0, 0);
            sys_exit(0)
        }
    }

    /// The blk result consumer (kernel task): receives the blk component's
    /// self-test result and prints it. Then idles (kernel tasks never return).
    extern "C" fn blk_consumer() -> ! {
        let m = sched::recv_message(BLK_REPORT_CAP);
        if m.badge == 1 {
            println!("blk: sector round-trip ok");
        } else {
            println!("blk: sector round-trip FAILED");
        }
        loop {
            sched::yield_now();
            // SAFETY: wait for the next interrupt between yields.
            unsafe { core::arch::asm!("wfi") };
        }
    }
```

- [ ] **Step 5: Discover the device early and wire it in `kmain`**

In `kernel/src/main.rs`, next to the RNG probe (early, MMU off), add the blk probe:

```rust
        let blk_base = unsafe { virtio::find_device(&machine.virtio_mmio[..machine.virtio_mmio_count], virtio::DEVICE_ID_BLK) };
```

Then, in `kmain` just before the `idle` spawn, add the blk wiring:

```rust
        // Phase 6a — block storage: a user-space virtio-blk driver. The consumer
        // (a kernel task) recv-blocks for the self-test result before the
        // component reports.
        if let Some(blk) = blk_base {
            let dma_pa = mem::frame::alloc_zeroed().expect("no DMA frame for blk").0;
            let blk_cons = sched::spawn("blkc", blk_consumer,
                core::ptr::addr_of!(KS_BLKC) as usize + TASK_STACK);
            sched::grant_cap(blk_cons, BLK_REPORT_CAP, Capability::Endpoint(BLK_EP));

            let bu = ustack(core::ptr::addr_of!(US_BLK) as usize);
            let blkdev = sched::spawn_user("blk", blk_component, bu.1,
                core::ptr::addr_of!(KS_BLK) as usize + TASK_STACK,
                mem::build_virtio_space(bu, (blk, blk + 0x1000), (dma_pa, dma_pa + 0x1000)));
            sched::grant_cap(blkdev, BLK_REPORT_CAP, Capability::Endpoint(BLK_EP));

            let n = machine.virtio_mmio_count;
            let blk_irq = virtio::irq_for_base(&machine.virtio_mmio[..n], &machine.virtio_mmio_irq[..n], blk)
                .expect("blk has no IRQ in the device tree");
            sched::grant_cap(blkdev, BLK_IRQ_CAP, Capability::Interrupt(blk_irq));
            sched::set_launch_args(blkdev, blk, dma_pa);
            // PLIC setup (idempotent if the rng path already did init + sie).
            plic::init(machine.plic_base);
            plic::set_priority(blk_irq, 1);
            plic::enable(blk_irq);
            // SAFETY: the trap handler and the PLIC are set up to service it.
            unsafe { csr::sie_enable_external() };
        } else {
            println!("blk: no virtio-blk device found");
        }
```

- [ ] **Step 6: Build the kernel**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: SUCCESS.

- [ ] **Step 7: Run the smoke test — expect GREEN**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS`, including `blk: sector round-trip ok`, with every other milestone (the rng entropy path still works — both components find their device by DeviceID).

Troubleshooting (diagnose, don't weaken the test):
- `blk: sector round-trip FAILED` → a status byte ≠ 0 or the read-back mismatched: confirm the data descriptor sets `WRITE` only on the read (`blk_req`'s `data_flags`), and the header `type` is `T_OUT` for write / `T_IN` for read.
- Hang (no `blk:` line) → the IRQ isn't delivered: confirm `plic::enable(blk_irq)` ran and `blk_irq` came from `irq_for_base` (the blk device's slot), and the component holds the Interrupt cap at `BLK_IRQ_CAP`.
- `blk: no virtio-blk device found` → the smoke must pass `-drive … -device virtio-blk-device,drive=blk0` (Task 2).

- [ ] **Step 8: Commit**

```bash
git add kernel/src/main.rs
git commit -m "feat: Phase 6a - user-space virtio-blk driver, interrupt-driven sector round-trip"
```

---

## Task 4: docs — learning note, roadmap, glossary; final verification

**Files:**
- Create: `docs/learning/0022-block-storage.md`
- Modify: `docs/roadmap/roadmap.md`
- Modify: `docs/glossary.md`
- Modify: `docs/learning/README.md`

- [ ] **Step 1: Write a short learning note**

Create `docs/learning/0022-block-storage.md`:

```markdown
# 0022 — Block storage (virtio-blk)

**One-line:** the OS can read and write disk sectors — a user-space virtio-blk
driver, the foundation for a filesystem and a real, on-disk knowledge base.

## What changed
- A `blk` U-mode component drives the QEMU virtio-blk device (its MMIO + a DMA
  frame mapped only into it), reusing the virtio handshake, the split
  virtqueue, and `wait_irq` from the entropy/PLIC work.
- `find_rng` generalized to `find_device(bases, id)` — both the rng (id 4) and
  the block device (id 2) are found by DeviceID, so they coexist on any slots.
- The component self-tests a sector round-trip (write a pattern, read it back,
  verify) and reports; a kernel consumer prints `blk: sector round-trip ok`.

## The idea worth keeping
**A block request is a *chain* of descriptors, not one buffer.** Where the rng
used a single device-writable descriptor, a block I/O is three linked
descriptors: a header `{type, reserved, sector}` the device reads, a 512-byte
data buffer, and a 1-byte status the device writes. The data descriptor's
`WRITE` flag flips with direction — set for a read (the device fills it), clear
for a write (the device reads it). The same virtqueue notify/`wait_irq`
machinery carries it.

## Why this is step one of a real knowledge base
The self-healer's knowledge base is still a compiled-in stub (`KB-0005`).
Phase 6 makes it real: 6a is the disk, 6b a filesystem over it, 6c the healer
reading its actual `knowledge-base/` from disk. Because the DMA frame is
identity-mapped, the kernel filesystem (6b) will read sector data straight from
that physical page — no extra plumbing.

## Proof
`blk: sector round-trip ok` — a pattern written to sector 0 and read back
intact, interrupt-driven, through an unprivileged driver.
```

- [ ] **Step 2: Update the roadmap**

In `docs/roadmap/roadmap.md`, change the `### Phase 6a — Block storage (virtio-blk)` heading to mark it done and append a learning-note link:

```markdown
### Phase 6a — Block storage (virtio-blk)  *(done — 2026-06-23)*
```

And append to that sub-section's "Done when" a pointer:

```markdown
  (see [learning note 0022](../learning/0022-block-storage.md)).
```

- [ ] **Step 3: Add glossary entries**

In `docs/glossary.md`, add after the virtio/virtqueue/DMA cluster:

```markdown
- **Block device** — a storage device addressed in fixed-size *sectors* (here 512 bytes) rather than as a byte stream; you read or write whole sectors by number. The disk is a block device (virtio-blk).
- **virtio-blk** — the virtio block-device interface. Each request is a 3-descriptor chain (a header naming the operation + sector, a data buffer, and a status byte) submitted through the virtqueue.
```

- [ ] **Step 4: Update the learning-notes index**

In `docs/learning/README.md`, add after the 0021 line:

```markdown
- [0022 — Block storage (virtio-blk)](0022-block-storage.md)
```

- [ ] **Step 5: Final verification (run all, paste results)**

Run: `cargo test -p kernel-arch-riscv64` → expect all green.
Run: `cargo build --workspace` → expect success.
Run (PowerShell): `./tools/check-references.ps1` → expect PASS.
Run (PowerShell): `./tools/test-qemu.ps1` → expect `BOOT TEST PASS`.

- [ ] **Step 6: Commit**

```bash
git add docs/learning/0022-block-storage.md docs/roadmap/roadmap.md docs/glossary.md docs/learning/README.md
git commit -m "docs: Phase 6a block storage - learning note, roadmap, glossary"
```

---

## Done-when checklist (maps to spec §1)

- [ ] **A sector round-trip** — smoke shows `blk: sector round-trip ok`, with the rng/entropy path and every other milestone still present.
- [ ] **Host tests** — the blk DMA-layout offsets are ordered and within the frame; `DEVICE_ID_BLK`=2.
- [ ] `check-references` clean; `cargo build --workspace` green; `BOOT TEST PASS`.
```
