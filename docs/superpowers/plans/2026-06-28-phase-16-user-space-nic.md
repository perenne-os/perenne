# Phase 16 — Move the NIC driver to a user-space component — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Relocate the Phase 15 virtio-net + ARP exchange out of the kernel into an unprivileged U-mode driver component (ADR 0007), joining `rng`/`blk`, with the ARP logic staying kernel-side in a small resolver client — same wire behavior, the kernel no longer touching the NIC registers.

**Architecture:** The `blk` model. A U-mode `net_component` (`.user_text`, inline asm) does only raw virtio-net mechanics (two-queue bring-up, RX pre-post, transmit-the-DMA-frame, block on the device IRQ) and serves a call/reply endpoint. A kernel `net_resolver` task builds the ARP request into the shared identity-mapped DMA page (host-tested `kernel_common::net`), calls the driver, parses the reply, and prints it.

**Tech Stack:** Rust `no_std`, RISC-V64, QEMU `virt`, virtio-mmio (modern/v2), existing capability/IPC + PLIC + `wait_irq` machinery.

## Global Constraints

- **Target/scope:** QEMU `virt` riscv64 only; no board. Same QEMU flags already in `tools/test-qemu.ps1` (`-netdev user,id=net0 -device virtio-net-device,netdev=net0`).
- **U-mode codegen rule:** U-mode component code lives in `#[link_section = ".user_text"]` and may NOT call kernel `.text`/`.rodata`. All MMIO/DMA is via `#[inline(always)]` inline-asm helpers (which fold into the caller's `.user_text`); constants fold to immediates. No `core::ptr::read_volatile` in U-mode (may become a call).
- **No new pure logic:** `kernel_common::net` (ARP build/parse) is reused unchanged and stays host-tested. There is no new unit-testable logic; the integration proof is the boot smoke test. `cargo test` must stay green throughout.
- **Wire behavior is byte-identical to Phase 15:** src MAC `52:54:00:12:34:56`, src IP `10.0.2.15`, target/gateway `10.0.2.2`, frames are a 12-byte zeroed `virtio_net_hdr` + ARP, DMA layout offsets unchanged.
- **Commit identity:** project default (no Claude co-author; signing automated).
- **Done-when (whole phase):** `./tools/test-qemu.ps1` shows `net: resolved 10.0.2.2 -> 52:55:0a:00:02:02`, produced by the U-mode driver (a `net` task in the scheduler; the kernel calls it), with `cargo test` green.

## File structure

- `kernel/src/main.rs` — all changes: new consts/statics, the `net_component` (U-mode) and `net_resolver` (kernel) fns, the `kmain` spawn block; removal of the in-kernel `net_resolve_gateway` and its boot-time call.
- `arch/riscv64/src/sched.rs` — `MAX_TASKS 25 → 27`.
- `arch/riscv64/src/mem/mod.rs` — remove `map_device` if it becomes unused (Task 4 verifies).
- Docs (Task 6): `docs/learning/0034-*.md`, `docs/roadmap/roadmap.md`, `docs/glossary.md`, the spec's revision note.

---

### Task 1: Constants, statics, and task-budget bump

**Files:**
- Modify: `kernel/src/main.rs` (const/static block ~lines 444–557)
- Modify: `arch/riscv64/src/sched.rs:28`

**Interfaces:**
- Produces: `NET_EP: usize = 9`, `NET_EP_CAP: usize = 0`, `NET_IRQ_CAP: usize = 1`, `NET_REPLY_SLOT: usize = 2`, `static mut NET_DMA_PA: usize`, `static mut KS_NET`, `static mut KS_NETRES`, `static mut US_NET`. Consumed by Tasks 2–4.

- [ ] **Step 1: Bump MAX_TASKS**

In `arch/riscv64/src/sched.rs:28`, change:
```rust
pub const MAX_TASKS: usize = 25;
```
to:
```rust
pub const MAX_TASKS: usize = 27; // +2 for the net driver + net resolver (Phase 16)
```

- [ ] **Step 2: Add the net consts**

In `kernel/src/main.rs`, after the `BLK_WRITE_FLAG` block (~line 457), add:
```rust
    /// The net service endpoint (the kernel `net_resolver` calls it; the U-mode
    /// `net` driver recvs). Mirrors the blk cap layout exactly.
    const NET_EP: usize = 9;
    /// net cap slots: 0 = the service endpoint, 1 = its Interrupt cap, 2 = the
    /// one-shot Reply cap the driver's recv mints per call.
    const NET_EP_CAP: usize = 0;
    const NET_IRQ_CAP: usize = 1;
    const NET_REPLY_SLOT: usize = 2;
```

- [ ] **Step 3: Add the DMA-pointer static**

After `static mut BLK_DMA_PA: usize = 0;` (~line 472), add:
```rust
    /// Physical address of the net DMA frame (identity-mapped); the resolver
    /// builds the ARP request into `NET_DMA_PA + 0xC00 + 12` and reads the reply
    /// from `NET_DMA_PA + 0x400 + 12`. Set by `kmain`.
    static mut NET_DMA_PA: usize = 0;
```

- [ ] **Step 4: Add the kernel stacks**

After `static mut KS_BLK: KStack = [0; TASK_STACK];` (~line 501), add:
```rust
    static mut KS_NET: KStack = [0; TASK_STACK];
    static mut KS_NETRES: KStack = [0; TASK_STACK];
```

- [ ] **Step 5: Add the U-mode stack**

After the `US_BLK` block (~line 555), add:
```rust
    #[link_section = ".user_data"]
    static mut US_NET: UStack = UStack([0; USER_STACK_SIZE]);
```

- [ ] **Step 6: Verify it still builds (no behavior change yet)**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: builds (new consts/statics are `dead_code`-warned but compile; the MAX_TASKS bump is inert). Warnings about unused `NET_*` are acceptable until Task 4 wires them.

- [ ] **Step 7: Commit**

```bash
git add kernel/src/main.rs arch/riscv64/src/sched.rs
git commit -m "feat(net): Phase 16 scaffolding — net EP/cap consts, DMA + stacks, MAX_TASKS 25->27"
```

---

### Task 2: The U-mode `net` driver component

**Files:**
- Modify: `kernel/src/main.rs` (add two `#[inline(always)]` DMA-read helpers near the other `dma_r*` helpers ~line 899; add `net_component` near `blk_component` ~line 1567)

**Interfaces:**
- Consumes: `NET_EP_CAP`, `NET_IRQ_CAP`, `NET_REPLY_SLOT` (Task 1); `sys_recv`, `sys_reply`, `sys_wait_irq`, `mmio_w`, `mmio_r`, `dma_w16/32/64`, `dma_fence` (existing); `virtio::*` consts.
- Produces: `extern "C" fn net_component() -> !` (used by Task 4's spawn).

- [ ] **Step 1: Add `dma_r16` and `dma_r32` inline-asm helpers**

In `kernel/src/main.rs`, immediately after `dma_r8` (~line 899), add:
```rust
    #[inline(always)]
    unsafe fn dma_r16(addr: usize) -> u16 {
        let v;
        core::arch::asm!("lhu {v}, 0({a})", v = out(reg) v, a = in(reg) addr, options(nostack));
        v
    }
    #[inline(always)]
    unsafe fn dma_r32(addr: usize) -> u32 {
        let v;
        core::arch::asm!("lw {v}, 0({a})", v = out(reg) v, a = in(reg) addr, options(nostack));
        v
    }
```

- [ ] **Step 2: Add the `net_component` driver**

In `kernel/src/main.rs`, after `blk_component` (~line 1567), add:
```rust
    /// The user-space virtio-net driver (Phase 16, ADR 0007). It exclusively
    /// owns the NIC — the MMIO + DMA frame are mapped RW-U into THIS component
    /// only — and the kernel never touches the device. Raw mechanics only: the
    /// modern two-queue bring-up (RX=0, TX=1), one pre-posted RX buffer, then a
    /// call/reply loop: each call's badge is the TX frame length; it publishes
    /// the TX descriptor, notifies, blocks on the device IRQ until the RX used
    /// ring advances, and replies the received length (0 = none). The ARP frame
    /// is built/parsed by the kernel `net_resolver` client in the shared DMA
    /// page (a U-mode task can't call `kernel_common::net`). Only one exchange is
    /// served (one RX buffer posted) — all this phase needs.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn net_component() -> ! {
        let mmio: usize;
        let dma: usize;
        // SAFETY: read the launch args the kernel placed in a1/a2.
        unsafe {
            core::arch::asm!("mv {m}, a1", "mv {d}, a2",
                m = out(reg) mmio, d = out(reg) dma,
                options(nomem, nostack, preserves_flags));
        }
        // SAFETY: mmio + dma are mapped RW-U into this task; the sequence is the
        // spike-verified virtio-net bring-up, then a recv/transmit/wait/reply loop.
        unsafe {
            let rx_desc = dma + 0x000;
            let rx_avail = dma + 0x080;
            let rx_used = dma + 0x100;
            let tx_desc = dma + 0x200;
            let tx_avail = dma + 0x280;
            let tx_used = dma + 0x300;
            let rx_buf = dma + 0x400;
            let tx_buf = dma + 0xC00;

            // --- modern virtio-mmio bring-up, two queues (0=RX, 1=TX) ---
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
            for (q, d, a, u) in [(0u32, rx_desc, rx_avail, rx_used), (1, tx_desc, tx_avail, tx_used)] {
                mmio_w(mmio, virtio::QUEUE_SEL, q);
                mmio_w(mmio, virtio::QUEUE_NUM, virtio::VQ_SIZE);
                mmio_w(mmio, virtio::QUEUE_DESC_LOW, d as u32);
                mmio_w(mmio, virtio::QUEUE_DESC_HIGH, (d >> 32) as u32);
                mmio_w(mmio, virtio::QUEUE_DRIVER_LOW, a as u32);
                mmio_w(mmio, virtio::QUEUE_DRIVER_HIGH, (a >> 32) as u32);
                mmio_w(mmio, virtio::QUEUE_DEVICE_LOW, u as u32);
                mmio_w(mmio, virtio::QUEUE_DEVICE_HIGH, (u >> 32) as u32);
                mmio_w(mmio, virtio::QUEUE_READY, 1);
            }
            mmio_w(mmio, virtio::STATUS,
                virtio::STATUS_ACK | virtio::STATUS_DRIVER | virtio::STATUS_FEATURES_OK | virtio::STATUS_DRIVER_OK);

            // --- pre-post one RX buffer (device-writable) ---
            dma_w64(rx_desc, rx_buf as u64);
            dma_w32(rx_desc + 8, 2048);
            dma_w16(rx_desc + 12, virtio::VIRTQ_DESC_F_WRITE);
            dma_w16(rx_desc + 14, 0);
            dma_w16(rx_avail + 4, 0); // ring[0] -> desc 0
            dma_fence();
            dma_w16(rx_avail + 2, 1); // avail.idx
            dma_fence();

            let mut tx_idx: u16 = 0;
            loop {
                // badge = total TX length (12-byte virtio_net_hdr + ARP frame),
                // already written into tx_buf by the kernel resolver.
                let tx_len = sys_recv(NET_EP_CAP, NET_REPLY_SLOT);

                // publish the TX descriptor (device reads), notify queue 1.
                dma_w64(tx_desc, tx_buf as u64);
                dma_w32(tx_desc + 8, tx_len as u32);
                dma_w16(tx_desc + 12, 0);
                dma_w16(tx_desc + 14, 0);
                dma_w16(tx_avail + 4 + (tx_idx as usize % virtio::VQ_SIZE as usize) * 2, 0);
                dma_fence();
                dma_w16(tx_avail + 2, tx_idx + 1);
                dma_fence();
                mmio_w(mmio, virtio::QUEUE_NOTIFY, 1);
                tx_idx = tx_idx.wrapping_add(1);

                // Block on the device IRQ until the RX used ring advances. A
                // TX-completion IRQ may wake us first; ack and re-wait. Bounded so
                // a genuine no-reply replies 0.
                let mut rx_len: usize = 0;
                for _ in 0..16 {
                    sys_wait_irq(NET_IRQ_CAP);
                    let is = mmio_r(mmio, virtio::INTERRUPT_STATUS);
                    if is != 0 {
                        mmio_w(mmio, virtio::INTERRUPT_ACK, is);
                    }
                    if dma_r16(rx_used + 2) != 0 {
                        // used-ring element 0: id(u32 @ +4), len(u32 @ +8).
                        rx_len = dma_r32(rx_used + 8) as usize;
                        break;
                    }
                }
                sys_reply(NET_REPLY_SLOT, rx_len);
            }
        }
    }
```

- [ ] **Step 3: Verify it builds**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: builds (`net_component` is unused until Task 4 → `dead_code` warning OK).

- [ ] **Step 4: Commit**

```bash
git add kernel/src/main.rs
git commit -m "feat(net): user-space virtio-net driver component (two-queue, IRQ-driven)"
```

---

### Task 3: The kernel `net_resolver` client

**Files:**
- Modify: `kernel/src/main.rs` (add `net_resolver` near the other kernel tasks, e.g. after `net_component`)

**Interfaces:**
- Consumes: `NET_DMA_PA`, `NET_EP_CAP` (Tasks 1); `sched::call_message(cap_idx, badge) -> usize` (existing); `kernel_common::net::{build_request, parse_reply, ARP_FRAME_LEN}` (existing); `println!`; `sched::yield_now()`.
- Produces: `extern "C" fn net_resolver() -> !` (used by Task 4's spawn).

- [ ] **Step 1: Add the resolver task**

In `kernel/src/main.rs`, after `net_component`, add:
```rust
    /// The kernel client for the user-space `net` driver (Phase 16). It builds
    /// the ARP request into the shared identity-mapped DMA page (host-tested
    /// `kernel_common::net`), `call`s the driver to transmit + receive, then
    /// parses the reply and reports the gateway MAC. The ARP logic stays
    /// kernel-side and pure — exactly as `fs` stayed kernel-side over `blk`.
    extern "C" fn net_resolver() -> ! {
        use kernel_common::net;
        // SAFETY: NET_DMA_PA is the kernel-allocated, identity-mapped DMA frame;
        // single hart owns it. The driver shares the same physical frame RW-U.
        unsafe {
            let dma = NET_DMA_PA;
            let tx_frame = dma + 0xC00 + 12; // after the 12-byte virtio_net_hdr
            let rx_frame = dma + 0x400 + 12;
            let src_mac = [0x52u8, 0x54, 0x00, 0x12, 0x34, 0x56];
            let txf = core::slice::from_raw_parts_mut(tx_frame as *mut u8, net::ARP_FRAME_LEN);
            let arp_len = net::build_request(&src_mac, [10, 0, 2, 15], [10, 0, 2, 2], txf);

            // Call the driver: badge = total TX length (header + ARP). Blocks
            // until the driver replies the received length (0 = no reply).
            let rx_len = sched::call_message(NET_EP_CAP, 12 + arp_len);

            let mut resolved = false;
            if rx_len != 0 {
                let rxf = core::slice::from_raw_parts(rx_frame as *const u8, net::ARP_FRAME_LEN);
                if let Some(mac) = net::parse_reply(rxf, [10, 0, 2, 2]) {
                    println!(
                        "net: resolved 10.0.2.2 -> {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
                    );
                    resolved = true;
                }
            }
            if !resolved {
                println!("net: no ARP reply");
            }
        }
        // Done: idle like the other one-shot kernel tasks.
        loop {
            sched::yield_now();
            // SAFETY: wait for the next interrupt between yields.
            unsafe { core::arch::asm!("wfi") };
        }
    }
```

- [ ] **Step 2: Verify it builds**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: builds (`net_resolver` unused until Task 4 → `dead_code` warning OK).

- [ ] **Step 3: Commit**

```bash
git add kernel/src/main.rs
git commit -m "feat(net): kernel net_resolver — builds ARP, calls the driver, prints the gateway MAC"
```

---

### Task 4: Wire `kmain` — remove the in-kernel driver, spawn the component + resolver

**Files:**
- Modify: `kernel/src/main.rs` (remove the boot-time net block ~lines 64–70; remove `net_resolve_gateway` fn ~lines 933–1027; add the spawn block in the task-spawning region, e.g. after the `blk` block ~line 317)
- Modify: `arch/riscv64/src/mem/mod.rs` (remove `map_device` if unused after this task)

**Interfaces:**
- Consumes: everything from Tasks 1–3; `mem::build_virtio_space`, `mem::frame::alloc_zeroed`, `sched::spawn_user`, `sched::spawn`, `sched::grant_cap`, `sched::set_launch_args`, `virtio::irq_for_base`, `plic::*`, `csr::sie_enable_external`, `Capability::{Endpoint, Interrupt}` (all existing, used identically by the blk block).

- [ ] **Step 1: Remove the boot-time in-kernel net exchange**

In `kernel/src/main.rs`, delete the Phase 15 block (~lines 64–70):
```rust
        // Phase 15: bring up virtio-net and resolve the gateway (10.0.2.2) by
        // ARP — the OS's first network exchange.
        if let Some(net) = net_base {
            unsafe { net_resolve_gateway(net) };
        } else {
            println!("net: no virtio-net device found");
        }
```
(Keep the `let net_base = …` discovery at ~line 61 — it's used by the new spawn block. Keep the surrounding `mem::init` / paging lines.)

- [ ] **Step 2: Remove the in-kernel `net_resolve_gateway`**

Delete the whole `unsafe fn net_resolve_gateway(mmio: usize) { … }` (~lines 933–1027, the doc-comment through its closing brace). Its logic now lives in `net_component` + `net_resolver`.

- [ ] **Step 3: Add the spawn block**

In `kernel/src/main.rs`, after the `blk` `if let Some(blk) = blk_base { … }` block closes (~line 317), add:
```rust
        // Phase 16 — the NIC as a user-space component (ADR 0007). The U-mode
        // `net` driver owns the virtio-net MMIO + DMA (mapped RW-U into it only);
        // the kernel `net_resolver` calls it to ARP-resolve the gateway. Spawn the
        // driver first (lower slot) so it recv-blocks before the resolver calls.
        if let Some(net) = net_base {
            let dma_pa = mem::frame::alloc_zeroed().expect("no DMA frame for net").0;
            // SAFETY: set once at boot before the resolver runs; single hart.
            unsafe { NET_DMA_PA = dma_pa; }

            let netu = ustack(core::ptr::addr_of!(US_NET) as usize);
            let netdev = sched::spawn_user("net", net_component, netu.1,
                core::ptr::addr_of!(KS_NET) as usize + TASK_STACK,
                mem::build_virtio_space(netu, (net, net + 0x1000), (dma_pa, dma_pa + 0x1000)));
            sched::grant_cap(netdev, NET_EP_CAP, Capability::Endpoint(NET_EP));

            let n = machine.virtio_mmio_count;
            let net_irq = virtio::irq_for_base(&machine.virtio_mmio[..n], &machine.virtio_mmio_irq[..n], net)
                .expect("net has no IRQ in the device tree");
            sched::grant_cap(netdev, NET_IRQ_CAP, Capability::Interrupt(net_irq));
            sched::set_launch_args(netdev, net, dma_pa);
            // PLIC setup (idempotent if the rng/blk paths already did init + sie).
            plic::init(machine.plic_base);
            plic::set_priority(net_irq, 1);
            plic::enable(net_irq);
            // SAFETY: the trap handler and the PLIC are set up to service it.
            unsafe { csr::sie_enable_external() };

            // The resolver is the net driver's client: grant it the service cap.
            let netres = sched::spawn("netres", net_resolver,
                core::ptr::addr_of!(KS_NETRES) as usize + TASK_STACK);
            sched::grant_cap(netres, NET_EP_CAP, Capability::Endpoint(NET_EP));
        } else {
            println!("net: no virtio-net device found");
        }
```

- [ ] **Step 4: Check whether `mem::map_device` is now unused**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf 2>&1 | grep -i "map_device\|never used"`
- If `map_device` is reported unused, delete the `pub fn map_device(base: usize) { … }` (and its doc-comment) from `arch/riscv64/src/mem/mod.rs:200–213`.
- If something else still calls it, leave it (note who in the commit message).

- [ ] **Step 5: Build the kernel**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: builds clean — the `dead_code` warnings from Tasks 1–3 are gone (everything is wired), no unused `net_component`/`net_resolver`/`NET_*`.

- [ ] **Step 6: Commit**

```bash
git add kernel/src/main.rs arch/riscv64/src/mem/mod.rs
git commit -m "feat(net): wire the user-space NIC — spawn net driver + resolver, retire in-kernel net_resolve_gateway"
```

---

### Task 5: Verify — host tests green + boot smoke proves the U-mode driver

**Files:** none (verification only)

- [ ] **Step 1: Host tests**

Run: `cargo test`
Expected: PASS, including `kernel_common::net` ARP tests (build→parse round-trip, reply→MAC, `None` for non-ARP/wrong ethertype/wrong oper/wrong target). No test was changed; this confirms no regression.

- [ ] **Step 2: Boot smoke**

Run: `./tools/test-qemu.ps1`
Expected: the run asserts (already in the script, line ~143) and finds:
```
net: resolved 10.0.2.2 -> 52:55:0a:00:02:02
```
plus a `net` task present in the scheduler output. The line is now produced by the **U-mode driver** via the kernel resolver — the kernel never touched the NIC registers.

- [ ] **Step 3: If the smoke fails, debug before proceeding**

Likely culprits and checks (do NOT loosen the assertion):
- No `net:` line at all → confirm `net_base` discovered (the `else` prints `net: no virtio-net device found`); confirm the spawn block compiled in.
- `net: no ARP reply` → the driver's IRQ wait isn't seeing the RX completion. Confirm `net_irq` matched, PLIC `enable(net_irq)` ran, and the RX used-ring read (`dma_r16(rx_used+2)`) targets the right offset (`0x100 + 2`). Confirm the bound (16 `wait_irq` iterations) is enough; SLIRP replies in microseconds, so one TX-completion + one RX-completion wake is typical.
- Hang (no scheduler progress) → verify the driver was spawned at a lower slot than the resolver (it must recv-block first) and that `MAX_TASKS` is 27.

- [ ] **Step 4: No commit** (verification only; code already committed in Tasks 1–4)

---

### Task 6: Documentation

**Files:**
- Create: `docs/learning/0034-user-space-nic.md`
- Modify: `docs/roadmap/roadmap.md` (turn the "Phase 16+ — Breadth" head into a completed "Phase 16" entry; keep a "Phase 17+ — Breadth" tail for the remaining long tail)
- Modify: `docs/glossary.md` (note the NIC is now a user-space driver, if a net/virtio-net entry exists)
- Modify: `docs/superpowers/specs/2026-06-28-phase-16-user-space-nic-design.md` (add an "Implementation note" recording what shipped)

- [ ] **Step 1: Write learning note 0034**

Create `docs/learning/0034-user-space-nic.md` — keep it short (summary, not tutorial; per the project's learning-note convention). Cover: the `blk` model split (driver = raw mechanics in `.user_text`; ARP logic stays kernel-side in the resolver over a shared identity-mapped DMA page); why the U-mode driver can't call `kernel_common::net`; that the RX completion is a one-shot rising-edge IRQ the edge-PLIC delivers (so the driver blocks on `wait_irq` like blk, not polling); and that this retires the Phase 15 in-kernel deviation so every device the OS drives is now an unprivileged component.

- [ ] **Step 2: Update the roadmap**

Replace the "## Phase 16+ — Breadth" heading with a completed "## Phase 16 — The NIC becomes a user-space component *(done — 2026-06-28)*" entry (goal / you-learn / done-when, matching the Phase 15 style and the actual shipped behavior), and add a fresh "## Phase 17+ — Breadth" section carrying the remaining long tail (IP/UDP stack, DHCP/ping, encrypt traffic with the Phase 14 channel, U-mode crypto, epoch revocation + CDT, per-component crash ledgers, growable records, board boot 4c, more devices/HAL).

- [ ] **Step 3: Update the glossary**

If `docs/glossary.md` has a virtio-net / NIC / `net` entry, update it to say the driver runs as an unprivileged user-space component (ADR 0007) as of Phase 16. Add one if a parallel `blk`/`rng` entry pattern exists.

- [ ] **Step 4: Add the spec implementation note**

In the spec, add a short "## Implementation note (2026-06-28, during build)" recording that the phase shipped as designed (blk-model split, interrupt-driven driver, MAX_TASKS 25→27), and whether `map_device` was removed.

- [ ] **Step 5: Verify references**

Run: `pwsh tools/check-references.ps1`
Expected: PASS (every doc path/KB id cited resolves). Fix any dangling reference before committing.

- [ ] **Step 6: Commit**

```bash
git add docs/
git commit -m "docs: Phase 16 user-space NIC — learning note 0034, roadmap, glossary; spec impl note"
```

---

## Self-review notes

- **Spec coverage:** driver/client split (Tasks 2–3), interrupt-driven RX wait (Task 2 loop), shared identity-mapped DMA (Tasks 1/3/4), removal of `net_resolve_gateway` + `map_device` (Task 4), MAX_TASKS 25→27 (Task 1), unchanged smoke assertion (Task 5), docs (Task 6) — all mapped.
- **No new pure logic:** `kernel_common::net` reused unchanged; the integration proof is the boot smoke (Task 5), consistent with how every device-bring-up phase in this repo is verified. This is why the tasks are build+smoke gated rather than red-green unit tests.
- **Type/name consistency:** `NET_EP=9`, `NET_EP_CAP=0`, `NET_IRQ_CAP=1`, `NET_REPLY_SLOT=2`, `NET_DMA_PA`, `net_component`, `net_resolver`, `KS_NET`/`KS_NETRES`/`US_NET` used identically across Tasks 1–4. DMA offsets (RX 0x000/0x080/0x100, TX 0x200/0x280/0x300, bufs 0x400/0xC00) match the spec table and Task 2/3 code. `sched::call_message(NET_EP_CAP, 12+arp_len)` ↔ driver `sys_recv(NET_EP_CAP, …)`/`sys_reply(NET_REPLY_SLOT, rx_len)` form one consistent call/reply contract.
```
